//! カーネルログ (§38)。
//!
//! レベルは TRACE / DEBUG / INFO / WARN / ERROR / FATAL の 6 段階。
//! 出力先は COM1 シリアルと、直近のログを保持するリングバッファ ([`LOG_RING_SIZE`] バイト)。
//! リングバッファはシリアルが無い実機でも残り、Kernel Panic 画面に表示される。
//!
//! ロガーの状態は [`IrqSpinLock`] で保護する (割り込みハンドラからもログを出せる)。
//! 割り込み・例外ハンドラの中でロックが取れない (= 割り込まれたコードがログ出力中) 場合は、
//! 待たずに COM1 へ直接書き出す (行が混ざることはあるが、デッドロックはしない)。
//!
//! 書式:
//!
//! ```text
//! [ INFO] boot: message
//! ```
//!
//! 使い方は `trace!` / `debug!` / `info!` / `warn!` / `error!` / `fatal!` マクロ。
//! マクロはテキストスコープで定義されるため、`main.rs` で他のモジュールより先に
//! `#[macro_use] mod log;` として宣言している。

use core::fmt::{self, Write};
use core::sync::atomic::{AtomicU8, Ordering};

use crate::drivers::serial::{self, SerialError, SerialPort};
use crate::sync::IrqSpinLock;

/// ログレベル。値が大きいほど重大。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum Level {
    /// 非常に詳細な追跡情報
    Trace = 0,
    /// 開発時のデバッグ情報
    Debug = 1,
    /// 通常の動作情報
    Info = 2,
    /// 回復可能な異常
    Warn = 3,
    /// 機能が失敗した
    Error = 4,
    /// 回復不能(停止する)
    Fatal = 5,
}

impl Level {
    /// 固定幅 5 文字のラベル。
    pub const fn label(self) -> &'static str {
        match self {
            Self::Trace => "TRACE",
            Self::Debug => "DEBUG",
            Self::Info => " INFO",
            Self::Warn => " WARN",
            Self::Error => "ERROR",
            Self::Fatal => "FATAL",
        }
    }

    const fn from_u8(v: u8) -> Self {
        match v {
            0 => Self::Trace,
            1 => Self::Debug,
            2 => Self::Info,
            3 => Self::Warn,
            4 => Self::Error,
            _ => Self::Fatal,
        }
    }
}

/// ビルドプロファイルごとの既定の最小レベル。
/// debug ビルドは TRACE まで、release ビルドは DEBUG 以上を出力する。
pub const DEFAULT_MIN_LEVEL: Level = if cfg!(debug_assertions) {
    Level::Trace
} else {
    Level::Debug
};

static MIN_LEVEL: AtomicU8 = AtomicU8::new(DEFAULT_MIN_LEVEL as u8);

/// 直近ログのリングバッファの大きさ。
pub const LOG_RING_SIZE: usize = 16 * 1024;

/// 直近のログを保持するリングバッファ (古いものから上書き)。
struct LogRing {
    buf: [u8; LOG_RING_SIZE],
    /// 書き込んだ総バイト数 (次の書き込み位置 = written % SIZE)
    written: u64,
}

impl LogRing {
    fn push(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.buf[(self.written % LOG_RING_SIZE as u64) as usize] = b;
            self.written += 1;
        }
    }

    /// 末尾から最大 `out.len()` バイトを、行の先頭から始まるように `out` へコピーする。
    fn copy_tail(&self, out: &mut [u8]) -> usize {
        let available = self.written.min(LOG_RING_SIZE as u64);
        let n = available.min(out.len() as u64);
        let start = self.written - n;
        for (i, slot) in out.iter_mut().take(n as usize).enumerate() {
            *slot = self.buf[((start + i as u64) % LOG_RING_SIZE as u64) as usize];
        }
        // 途中から始まった最初の行は捨てる
        let mut skip = 0;
        if self.written > n
            && let Some(nl) = out[..n as usize].iter().position(|&b| b == b'\n')
        {
            skip = nl + 1;
        }
        out.copy_within(skip..n as usize, 0);
        n as usize - skip
    }
}

struct Logger {
    serial: Option<SerialPort>,
    ring: LogRing,
}

/// シリアルとリングバッファの両方に書く。
struct Tee<'a>(&'a mut Logger);

impl Write for Tee<'_> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.0.ring.push(s.as_bytes());
        if let Some(port) = self.0.serial.as_mut() {
            port.write_str(s)?;
        }
        Ok(())
    }
}

static LOGGER: IrqSpinLock<Logger> = IrqSpinLock::new(Logger {
    serial: None,
    ring: LogRing {
        buf: [0; LOG_RING_SIZE],
        written: 0,
    },
});

/// ロガーを初期化する(COM1 を初期化して出力先にする)。
///
/// UART が存在しない場合はエラーを返す。その場合もリングバッファへの記録は続く。
pub fn init() -> Result<(), SerialError> {
    let port = serial::init_com1()?;
    LOGGER.lock().serial = Some(port);
    Ok(())
}

/// 出力する最小レベルを変更する。
pub fn set_min_level(level: Level) {
    MIN_LEVEL.store(level as u8, Ordering::Relaxed);
}

/// 現在の最小レベル。
pub fn min_level() -> Level {
    Level::from_u8(MIN_LEVEL.load(Ordering::Relaxed))
}

/// `level` のログが出力対象か。
#[inline]
pub fn enabled(level: Level) -> bool {
    level >= min_level()
}

/// `module_path!()` からクレート名を取り除く (`yumekawa_kernel::boot` -> `boot`)。
fn short_module(module: &str) -> &str {
    match module.split_once("::") {
        Some((_, rest)) => rest,
        None => "kernel",
    }
}

#[doc(hidden)]
pub fn write_record(level: Level, module: &str, args: fmt::Arguments<'_>) {
    let guard = if crate::interrupts::in_interrupt() {
        // ハンドラ内: 割り込まれたコードがロガーを保持している可能性があるので待たない
        LOGGER.try_lock()
    } else {
        Some(LOGGER.lock())
    };
    match guard {
        Some(mut logger) => {
            let _ = writeln!(
                Tee(&mut logger),
                "[{}] {}: {}",
                level.label(),
                short_module(module),
                args
            );
        }
        None => serial::emergency_write(format_args!(
            "[{}] {}: {} (logger busy; written directly)\n",
            level.label(),
            short_module(module),
            args
        )),
    }
}

/// 直近のログを `out` にコピーする (行の先頭から)。コピーしたバイト数を返す。
#[allow(dead_code)] // Phase 3 以降 (画面上のログ表示・Terminal) で使う想定
pub fn recent(out: &mut [u8]) -> usize {
    LOGGER.lock().ring.copy_tail(out)
}

/// パニック経路専用: ロックを奪って直近のログをコピーする。
///
/// # Safety
///
/// 割り込み無効で、ロガーを保持していたコードが二度と実行されないこと
/// (パニック処理中で CPU を停止する直前)。
pub unsafe fn recent_for_panic(out: &mut [u8]) -> usize {
    // SAFETY: 呼び出し元の契約により、保持者は再開しない。copy_tail は範囲チェック付きの
    // 読み出しだけなので、保持者が更新途中でも境界外アクセスは起きない。
    let logger = unsafe { LOGGER.force_lock() };
    logger.ring.copy_tail(out)
}

/// 指定レベルでログを出す。
macro_rules! klog {
    ($level:expr, $($arg:tt)+) => {{
        let level = $level;
        if $crate::log::enabled(level) {
            $crate::log::write_record(level, module_path!(), format_args!($($arg)+));
        }
    }};
}

/// TRACE レベルのログ。
macro_rules! trace { ($($arg:tt)+) => { klog!($crate::log::Level::Trace, $($arg)+) }; }
/// DEBUG レベルのログ。
macro_rules! debug { ($($arg:tt)+) => { klog!($crate::log::Level::Debug, $($arg)+) }; }
/// INFO レベルのログ。
macro_rules! info { ($($arg:tt)+) => { klog!($crate::log::Level::Info, $($arg)+) }; }
/// WARN レベルのログ。
macro_rules! warn { ($($arg:tt)+) => { klog!($crate::log::Level::Warn, $($arg)+) }; }
/// ERROR レベルのログ。
macro_rules! error { ($($arg:tt)+) => { klog!($crate::log::Level::Error, $($arg)+) }; }
/// FATAL レベルのログ。
macro_rules! fatal { ($($arg:tt)+) => { klog!($crate::log::Level::Fatal, $($arg)+) }; }

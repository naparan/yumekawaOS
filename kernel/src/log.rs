//! カーネルログ (§38)。
//!
//! レベルは TRACE / DEBUG / INFO / WARN / ERROR / FATAL の 6 段階。
//! 出力先は COM1 シリアル。書式:
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
use crate::sync::SpinLock;

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

static SINK: SpinLock<Option<SerialPort>> = SpinLock::new(None);

/// ロガーを初期化する(COM1 を初期化して出力先にする)。
///
/// UART が存在しない場合はエラーを返し、以降のログは破棄される。
pub fn init() -> Result<(), SerialError> {
    let port = serial::init_com1()?;
    *SINK.lock() = Some(port);
    Ok(())
}

/// 出力する最小レベルを変更する。
#[allow(dead_code)] // Phase 1 のカーネル本体からは未使用 (Phase 2 以降でコマンドライン等から設定する想定)
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
    let mut sink = SINK.lock();
    if let Some(port) = sink.as_mut() {
        let _ = writeln!(
            port,
            "[{}] {}: {}",
            level.label(),
            short_module(module),
            args
        );
    }
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

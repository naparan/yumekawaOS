//! Kernel Panic (§9.2, §30.2)。
//!
//! 回復不能な状態 (Rust の `panic!`、回復できない CPU 例外) で呼ばれ、
//!
//! 1. 割り込みを無効化する
//! 2. シリアル (COM1) へ、ロックを使わず直接、種別・メッセージ・場所・エラーコード・
//!    レジスタ・CPU 情報を出力する
//! 3. 画面に Kernel Panic 画面 (直近のカーネルログ付き) を描く
//! 4. `system halted.` を出力して CPU を停止する
//!
//! ## 再パニック・デッドロック対策
//!
//! - パニック中の再パニック (画面描画中のフォルト等) は [`PANICKING`] で検出し、
//!   短いメッセージをシリアルへ直接出して即座に停止する (無限再帰しない)。
//! - ロガー・画面のロックは保持者を無視して奪う (`force_lock`)。保持者は割り込まれた
//!   コードか自分自身で、どちらも二度と再開しないため安全 (単一 CPU)。
//! - ヒープは使わない (固定長バッファのみ)。メモリ統計は `try_lock` で取れた場合だけ表示する。

use core::fmt::{self, Write};
use core::panic::PanicInfo;
use core::sync::atomic::{AtomicBool, Ordering};

use crate::arch;
use crate::arch::x86_64::cpu::CpuIdentity;
use crate::arch::x86_64::probe;
use crate::arch::x86_64::trap::TrapFrame;
use crate::drivers::serial;
use crate::fmtbuf::StackString;
use crate::graphics::display;
use crate::graphics::panic_screen::{self, PanicDetails};

static PANICKING: AtomicBool = AtomicBool::new(false);

/// 回復不能な CPU 例外の情報。
#[derive(Debug)]
pub struct ExceptionReport<'a> {
    /// ベクタ
    pub vector: u8,
    /// 例外名
    pub name: &'static str,
    /// 保存されたレジスタ
    pub frame: &'a TrapFrame,
    /// CR2 (#PF / #DF のとき)
    pub cr2: Option<u64>,
    /// 詳細
    pub detail: fmt::Arguments<'a>,
}

/// パニックの開始処理。2 回目以降なら `false` (呼び出し側は即停止する)。
fn enter() -> bool {
    arch::disable_interrupts();
    !PANICKING.swap(true, Ordering::SeqCst)
}

/// パニック処理中か。
pub fn is_panicking() -> bool {
    PANICKING.load(Ordering::Relaxed)
}

#[panic_handler]
fn panic(info: &PanicInfo<'_>) -> ! {
    if !enter() {
        serial::emergency_write(format_args!(
            "\n[FATAL] nested panic while handling a panic: {}\n[FATAL] system halted.\n",
            info.message()
        ));
        arch::halt_forever();
    }

    let mut location = StackString::<160>::new();
    match info.location() {
        Some(loc) => {
            let _ = write!(location, "{}:{}:{}", loc.file(), loc.line(), loc.column());
        }
        None => {
            let _ = write!(location, "<unknown location>");
        }
    }
    serial::emergency_write(format_args!(
        "\n[FATAL] KERNEL PANIC at {}: {}\n",
        location.as_str(),
        info.message()
    ));
    let mut message = StackString::<512>::new();
    let _ = write!(message, "{}", info.message());
    finish(PanicDetails {
        kind: "Rust panic",
        message: message.as_str(),
        location: Some(location.as_str()),
        error_code: None,
        rip: None,
        cr2: None,
        frame: None,
        cpu: "",
        status: "",
        log: &[],
        backtrace: &[],
    })
}

/// バックトレースの最大段数。
const BACKTRACE_DEPTH: usize = 10;

/// フレームポインタ (rbp) の連鎖を辿ってリターンアドレスを集める。
///
/// カーネルは `-C force-frame-pointers=yes` でビルドしているので、各フレームの
/// `[rbp]` が呼び出し元の rbp、`[rbp+8]` がリターンアドレスになる。rbp が壊れていても
/// 読み出しはプローブ (例外テーブルで回復) なので、パニック処理自体は落ちない。
fn backtrace(mut rbp: u64, out: &mut [u64; BACKTRACE_DEPTH]) -> usize {
    let mut n = 0;
    while n < BACKTRACE_DEPTH && rbp != 0 && rbp.is_multiple_of(8) && rbp >> 63 == 1 {
        let (Ok(ret), Ok(next)) = (probe::read_u64(rbp + 8), probe::read_u64(rbp)) else {
            break;
        };
        if ret == 0 {
            break;
        }
        out[n] = ret;
        n += 1;
        if next <= rbp {
            break;
        }
        rbp = next;
    }
    n
}

fn current_rbp() -> u64 {
    let rbp: u64;
    // SAFETY: rbp を読むだけ。
    unsafe {
        core::arch::asm!("mov {}, rbp", out(reg) rbp, options(nomem, nostack, preserves_flags))
    };
    rbp
}

/// 回復不能な CPU 例外による Kernel Panic。
pub fn exception_panic(report: &ExceptionReport<'_>) -> ! {
    if !enter() {
        serial::emergency_write(format_args!(
            "\n[FATAL] nested CPU exception {} (vector {}) at rip={:#x} while handling a panic\n[FATAL] system halted.\n",
            report.name, report.vector, report.frame.rip
        ));
        arch::halt_forever();
    }
    let f = report.frame;
    serial::emergency_write(format_args!(
        "\n[FATAL] KERNEL PANIC: CPU exception {} (vector {}): {}\n",
        report.name, report.vector, report.detail
    ));
    serial::emergency_write(format_args!(
        "[FATAL]   error code={:#x} rip={:#x} cr2={}\n[FATAL]   registers:\n{f:?}\n",
        f.error_code,
        f.rip,
        OptHex(report.cr2),
    ));
    let mut kind = StackString::<96>::new();
    let _ = write!(
        kind,
        "CPU exception {} (vector {})",
        report.name, report.vector
    );
    let mut message = StackString::<512>::new();
    let _ = write!(message, "{}", report.detail);
    finish(PanicDetails {
        kind: kind.as_str(),
        message: message.as_str(),
        location: None,
        error_code: Some(f.error_code),
        rip: Some(f.rip),
        cr2: report.cr2,
        frame: Some(f),
        cpu: "",
        status: "",
        log: &[],
        backtrace: &[],
    })
}

struct OptHex(Option<u64>);

impl fmt::Display for OptHex {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            Some(v) => write!(f, "{v:#x}"),
            None => f.write_str("-"),
        }
    }
}

/// 共通の後半: CPU 情報・状態をシリアルへ出し、画面を描いて停止する。
/// (`base` の cpu / status / log は無視して、ここで埋める)
fn finish(base: PanicDetails<'_>) -> ! {
    let cpu = CpuIdentity::read();
    let mut cpu_line = StackString::<160>::new();
    let _ = write!(
        cpu_line,
        "{} {} / apic id {}",
        cpu.vendor(),
        cpu.brand().unwrap_or("<no brand string>"),
        crate::drivers::apic::local_apic().map_or(u32::MAX, |l| l.id())
    );
    let mut status = StackString::<200>::new();
    let _ = write!(
        status,
        "uptime {} ms, ticks {}",
        crate::time::uptime_ms(),
        crate::time::ticks()
    );
    if let Some(fs) = crate::memory::frame::try_stats() {
        let _ = write!(
            status,
            ", frames free {}/{}",
            fs.free_frames, fs.total_frames
        );
    }
    if let Some(hs) = crate::memory::heap::try_stats() {
        let _ = write!(
            status,
            ", heap used {}/{} KiB",
            hs.used / 1024,
            hs.size / 1024
        );
    }
    serial::emergency_write(format_args!(
        "[FATAL]   cpu: {}\n[FATAL]   status: {}\n",
        cpu_line.as_str(),
        status.as_str()
    ));
    // バックトレース: 例外ならフォルトした RIP と、そのときの rbp から。
    // Rust のパニックならこの関数の rbp から (先頭の数段はパニック処理自身のフレーム)。
    let mut trace = [0u64; BACKTRACE_DEPTH];
    let n = match base.frame {
        Some(f) => {
            trace[0] = f.rip;
            let mut rest = [0u64; BACKTRACE_DEPTH];
            let m = backtrace(f.rbp, &mut rest);
            let m = m.min(BACKTRACE_DEPTH - 1);
            trace[1..=m].copy_from_slice(&rest[..m]);
            m + 1
        }
        None => backtrace(current_rbp(), &mut trace),
    };
    let mut trace_line = StackString::<256>::new();
    for addr in &trace[..n] {
        let _ = write!(trace_line, " {addr:#x}");
    }
    serial::emergency_write(format_args!(
        "[FATAL]   backtrace (return addresses, newest first):{}\n",
        trace_line.as_str()
    ));

    let mut log = [0u8; 4096];
    // SAFETY: enter() で割り込みを無効化済みで、この後 CPU を停止するため、
    // ロガーの保持者 (割り込まれたコード) が再開することはない。
    let log_len = unsafe { crate::log::recent_for_panic(&mut log) };
    let details = PanicDetails {
        cpu: cpu_line.as_str(),
        status: status.as_str(),
        log: &log[..log_len],
        backtrace: &trace[..n],
        ..base
    };

    // SAFETY: 上と同じ理由で、framebuffer の保持者は再開しない。単一 CPU。
    let mut display = unsafe { display::force_for_panic() };
    match display.as_mut() {
        Some(fb) => {
            panic_screen::draw(fb, &details);
            serial::emergency_write(format_args!("[FATAL] kernel panic screen drawn\n"));
        }
        None => serial::emergency_write(format_args!(
            "[FATAL] no display is available for the kernel panic screen\n"
        )),
    }
    serial::emergency_write(format_args!("[FATAL] system halted.\n"));
    arch::halt_forever()
}

//! CPU 例外 (ベクタ 0〜31) の処理 (§9.1, §9.2)。
//!
//! | 例外 | 処理 |
//! | --- | --- |
//! | #BP (int3) | ログ (RIP・レジスタ) を出して**復帰** (int3 の次の命令から継続) |
//! | NMI | ログを出して復帰 (IST) |
//! | #PF / #GP / #UD / #DE | 例外テーブル (プローブ) に該当すれば記録して復帰。そうでなければ Kernel Panic |
//! | #DF | Kernel Panic (IST)。CR2 / RSP がスタックのガードなら「スタックオーバーフロー」と表示 |
//! | #MC | Kernel Panic (IST) |
//! | その他 (#DE, #UD, ...) | Kernel Panic |
//!
//! Phase 2 ではユーザー空間が無いため、例外はすべてカーネル内で起きたものとして扱う。
//! 「該当タスクのみ停止」(§9.2) は Phase 5/7 でタスクを導入した時点で、
//! `frame.privilege_level() == 3` の例外をタスクの終了に振り分ける形で追加する。

use core::fmt;
use core::sync::atomic::{AtomicU64, Ordering};

use crate::arch::x86_64::idt::{exception_name, vector};
use crate::arch::x86_64::probe::{self, Fault};
use crate::arch::x86_64::registers;
use crate::arch::x86_64::trap::TrapFrame;
use crate::memory::stack;
use crate::panic::{self as kpanic, ExceptionReport};

static BREAKPOINTS: AtomicU64 = AtomicU64::new(0);
static RECOVERED_FAULTS: AtomicU64 = AtomicU64::new(0);
static NMIS: AtomicU64 = AtomicU64::new(0);

/// これまでに処理した #BP の回数。
pub fn breakpoint_count() -> u64 {
    BREAKPOINTS.load(Ordering::Relaxed)
}

/// 例外テーブルで回復したフォルトの回数。
pub fn recovered_fault_count() -> u64 {
    RECOVERED_FAULTS.load(Ordering::Relaxed)
}

/// #PF のエラーコードを人が読める形にする。
#[derive(Debug, Clone, Copy)]
pub struct PageFaultError(pub u64);

impl fmt::Display for PageFaultError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let e = self.0;
        write!(
            f,
            "{} {} in {} mode",
            if e & 1 != 0 {
                "protection violation"
            } else {
                "page not present"
            },
            if e & (1 << 4) != 0 {
                "on instruction fetch"
            } else if e & (1 << 1) != 0 {
                "on write"
            } else {
                "on read"
            },
            if e & (1 << 2) != 0 { "user" } else { "kernel" },
        )?;
        if e & (1 << 3) != 0 {
            f.write_str(", reserved bit set")?;
        }
        if e & (1 << 5) != 0 {
            f.write_str(", protection key")?;
        }
        if e & (1 << 6) != 0 {
            f.write_str(", shadow stack")?;
        }
        Ok(())
    }
}

/// CPU 例外を処理する。
pub(super) fn handle(frame: &mut TrapFrame) {
    let v = frame.vector as u8;
    match v {
        vector::BREAKPOINT => breakpoint(frame),
        vector::NMI => nmi(frame),
        vector::PAGE_FAULT => page_fault(frame),
        vector::GENERAL_PROTECTION => general_protection(frame),
        vector::DOUBLE_FAULT => double_fault(frame),
        vector::INVALID_OPCODE => {
            if !try_recover(frame, 0) {
                fatal(
                    frame,
                    None,
                    format_args!("invalid opcode at rip={:#x}", frame.rip),
                );
            }
        }
        vector::DIVIDE_ERROR => {
            if !try_recover(frame, 0) {
                fatal(
                    frame,
                    None,
                    format_args!(
                        "divide error (division by zero or quotient overflow) at rip={:#x}",
                        frame.rip
                    ),
                );
            }
        }
        _ => fatal(frame, None, format_args!("unrecoverable CPU exception")),
    }
}

fn breakpoint(frame: &mut TrapFrame) {
    let n = BREAKPOINTS.fetch_add(1, Ordering::Relaxed) + 1;
    // RIP は int3 の次の命令を指しているので、そのまま戻れば継続する
    info!(
        "#BP breakpoint at rip={:#x} (int3 at {:#x}), cs={:#x}, rsp={:#x}, count={n}; resuming",
        frame.rip,
        frame.rip.wrapping_sub(1),
        frame.cs,
        frame.rsp
    );
    debug!("#BP registers:\n{frame:?}");
}

fn nmi(frame: &mut TrapFrame) {
    let n = NMIS.fetch_add(1, Ordering::Relaxed) + 1;
    // NMI は他の処理 (ロガー保持中を含む) の途中でも来るので、ロックを使わずに出力する
    crate::drivers::serial::emergency_write(format_args!(
        "[ WARN] exceptions: NMI #{n} received at rip={:#x}; resuming\n",
        frame.rip
    ));
}

/// 例外テーブル・実行プローブで回復できれば回復して `true`。
fn try_recover(frame: &mut TrapFrame, cr2: u64) -> bool {
    let fault = Fault {
        vector: frame.vector as u8,
        error_code: frame.error_code,
        address: cr2,
        rip: frame.rip,
    };
    if frame.privilege_level() != 0 {
        return false;
    }
    if let Some(fixup) = probe::search_exception_table(frame.rip) {
        probe::record_fault(fault);
        RECOVERED_FAULTS.fetch_add(1, Ordering::Relaxed);
        if kpanic::is_panicking() {
            // パニック中のバックトレース採取 (壊れたフレームポインタ) では黙って戻る
            frame.rip = fixup;
            return true;
        }
        info!(
            "{} recovered via exception table: rip={:#x} -> fixup {fixup:#x}, error={:#x}, cr2={cr2:#x}",
            exception_name(fault.vector),
            frame.rip,
            frame.error_code
        );
        debug!("registers at the recovered fault:\n{frame:?}");
        frame.rip = fixup;
        return true;
    }
    let target = probe::EXEC_PROBE_TARGET.load(Ordering::SeqCst);
    let instruction_fetch = frame.error_code & (1 << 4) != 0;
    if fault.vector == vector::PAGE_FAULT && instruction_fetch && target != 0 && frame.rip == target
    {
        // 実行プローブ: `call target` の戻りアドレスはスタックの先頭にある
        let return_address = frame.rsp as *const u64;
        // SAFETY: フォルトは yk_probe_exec の `call rdi` 直後の命令フェッチで起きており、
        // RSP は yk_probe_exec が積んだ戻りアドレスを指している (カーネルスタック上で有効)。
        let ret = unsafe { return_address.read() };
        probe::record_fault(fault);
        RECOVERED_FAULTS.fetch_add(1, Ordering::Relaxed);
        info!(
            "{} on instruction fetch at {target:#x} recovered by the exec probe (error={:#x}); returning to {ret:#x}",
            exception_name(fault.vector),
            frame.error_code
        );
        frame.rip = ret;
        frame.rsp += 8;
        frame.rax = 1;
        return true;
    }
    false
}

fn page_fault(frame: &mut TrapFrame) {
    let cr2 = registers::read_cr2();
    if try_recover(frame, cr2) {
        return;
    }
    if let Some(stack) = stack::find_guard(cr2) {
        fatal(
            frame,
            Some(cr2),
            format_args!(
                "kernel stack overflow: guard page of stack '{}' hit (stack {:#x}-{:#x}), {}",
                stack.name,
                stack.bottom,
                stack.top,
                PageFaultError(frame.error_code)
            ),
        );
    }
    fatal(
        frame,
        Some(cr2),
        format_args!(
            "page fault at {cr2:#x}: {}",
            PageFaultError(frame.error_code)
        ),
    );
}

fn general_protection(frame: &mut TrapFrame) {
    if try_recover(frame, 0) {
        return;
    }
    let e = frame.error_code;
    if e == 0 {
        fatal(
            frame,
            None,
            format_args!(
                "general protection fault (error code 0: e.g. non-canonical address or privileged instruction)"
            ),
        );
    }
    fatal(
        frame,
        None,
        format_args!(
            "general protection fault: selector index {} ({}{})",
            (e >> 3) & 0x1fff,
            match (e >> 1) & 3 {
                0 => "GDT",
                2 => "LDT",
                _ => "IDT",
            },
            if e & 1 != 0 { ", external" } else { "" }
        ),
    );
}

fn double_fault(frame: &mut TrapFrame) {
    let cr2 = registers::read_cr2();
    // 典型例: スタックがガードに達して #PF の例外フレームを積めず #DF になった
    let guard = stack::find_guard(cr2).or_else(|| stack::find_guard(frame.rsp.wrapping_sub(8)));
    match guard {
        Some(stack) => fatal(
            frame,
            Some(cr2),
            format_args!(
                "kernel stack overflow detected: stack '{}' ({:#x}-{:#x}) ran into its guard page (cr2={cr2:#x}, rsp={:#x}); the page fault could not be delivered, so a double fault was raised on the IST stack",
                stack.name, stack.bottom, stack.top, frame.rsp
            ),
        ),
        None => fatal(
            frame,
            Some(cr2),
            format_args!("double fault (cr2={cr2:#x})"),
        ),
    }
}

/// 回復不能な例外: Kernel Panic。
fn fatal(frame: &TrapFrame, cr2: Option<u64>, detail: fmt::Arguments<'_>) -> ! {
    let v = frame.vector as u8;
    kpanic::exception_panic(&ExceptionReport {
        vector: v,
        name: exception_name(v),
        frame,
        cr2,
        detail,
    })
}

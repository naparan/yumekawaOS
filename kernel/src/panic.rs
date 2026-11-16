//! パニックハンドラ。
//!
//! 割り込みを無効化し、パニックの場所とメッセージを COM1 へ直接出力して停止する。
//! (ロガーのロックを保持したままパニックした場合でも出力できるよう、ロックを経由しない。)
//! Kernel Panic 画面 (§30.2) は GUI 基盤ができる Phase 4 以降で追加する。

use core::panic::PanicInfo;
use core::sync::atomic::{AtomicBool, Ordering};

use crate::arch;
use crate::drivers::serial;

static PANICKING: AtomicBool = AtomicBool::new(false);

#[panic_handler]
fn panic(info: &PanicInfo<'_>) -> ! {
    arch::disable_interrupts();

    // パニック処理中の再パニックでは何もせず停止する
    if PANICKING.swap(true, Ordering::SeqCst) {
        arch::halt_forever();
    }

    match info.location() {
        Some(loc) => serial::emergency_write(format_args!(
            "\n[FATAL] KERNEL PANIC at {}:{}:{}: {}\n",
            loc.file(),
            loc.line(),
            loc.column(),
            info.message()
        )),
        None => serial::emergency_write(format_args!(
            "\n[FATAL] KERNEL PANIC at <unknown location>: {}\n",
            info.message()
        )),
    }
    serial::emergency_write(format_args!("[FATAL] system halted.\n"));

    arch::halt_forever()
}

//! PS/2 キーボード (IRQ1) の受信。
//!
//! Phase 2 の範囲: IRQ1 でスキャンコード (**set 1**: i8042 の変換後) を受け取り、
//! 割り込み安全なリングバッファ ([`SpscRing`]) に積む。取り出しは通常コンテキストから
//! [`pop_scancode`] で行う (Phase 2 ではアイドルループがログに出す)。
//!
//! Phase 3 では、この生のスキャンコード列を入力に、`0xE0` プレフィックス・make/break
//! (bit7) を解釈するデコーダとキーマップ・キーイベント API を上に重ねる。
//! ハンドラ側は変更不要の想定。

use core::sync::atomic::{AtomicU64, Ordering};

use super::{STATUS_AUX_DATA, read_data, read_status};
use crate::arch::x86_64::trap::TrapFrame;
use crate::sync::SpscRing;

/// 受信バッファの容量 (バイト)。
pub const BUFFER_SIZE: usize = 256;

static SCANCODES: SpscRing<BUFFER_SIZE> = SpscRing::new();
static RECEIVED: AtomicU64 = AtomicU64::new(0);
static AUX_DROPPED: AtomicU64 = AtomicU64::new(0);

/// IRQ1 のハンドラ (`interrupts::register(vectors::KEYBOARD, ...)` で登録する)。
pub fn interrupt_handler(_frame: &mut TrapFrame) {
    // 1 回の割り込みで溜まっている分を読む (上限付き)
    for _ in 0..16 {
        let status = read_status();
        if status & 0x01 == 0 {
            break;
        }
        let byte = read_data();
        if status & STATUS_AUX_DATA != 0 {
            // マウスのデータ (Phase 3 で IRQ12 側が扱う)。ここでは捨てて数える
            AUX_DROPPED.fetch_add(1, Ordering::Relaxed);
            continue;
        }
        RECEIVED.fetch_add(1, Ordering::Relaxed);
        SCANCODES.push(byte);
    }
}

/// 受信したスキャンコード (set 1) を 1 バイト取り出す。
pub fn pop_scancode() -> Option<u8> {
    SCANCODES.pop()
}

/// キーボードの統計。
#[derive(Debug, Clone, Copy)]
pub struct KeyboardStats {
    /// 受信したバイト数
    pub received: u64,
    /// バッファ溢れで捨てたバイト数
    pub dropped: u64,
    /// 誤って IRQ1 で読んだマウスのバイト数
    pub aux_dropped: u64,
}

/// 統計。
pub fn stats() -> KeyboardStats {
    KeyboardStats {
        received: RECEIVED.load(Ordering::Relaxed),
        dropped: SCANCODES.dropped(),
        aux_dropped: AUX_DROPPED.load(Ordering::Relaxed),
    }
}

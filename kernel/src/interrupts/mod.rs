//! 割り込み・例外の管理 (§9)。
//!
//! - `dispatch`: すべてのベクタの入口 (arch::x86_64::trap から呼ばれる)。
//! - `register`: デバイスドライバがベクタにハンドラを登録する。
//! - `exceptions`: CPU 例外 (0〜31) の処理。
//!
//! ## EOI (End Of Interrupt)
//!
//! Local APIC 経由の割り込みは、**ディスパッチャがハンドラを呼ぶ前に** EOI を送る
//! (Local APIC の ISR にそのベクタが立っているときだけ。ソフトウェア `int n` には送らない)。
//! ハンドラは割り込みゲートにより IF=0 で実行されるので、早めに EOI しても同じベクタが
//! ハンドラ中に再入することは無い。こうすることで、
//!
//! - ハンドラが EOI を送り忘れることがない
//! - Phase 5 でタイマハンドラからスケジューラがコンテキストを切り替えて、この
//!   ハンドラに戻らない場合でも EOI 済みになる
//!
//! レベルトリガの割り込みも、ハンドラが iretq 前にデバイスの要因を消せば再発しない。
//!
//! ## 割り込みハンドラの規約
//!
//! - 割り込み無効 (IF=0) のまま短時間で終えること。`sti` しない。
//! - ヒープ確保をしない (固定長の [`crate::sync::SpscRing`] 等を使う)。
//! - 共有状態は [`crate::sync::IrqSpinLock`] か atomics で扱う。
//! - ログは出してよい (ロガーは IrqSpinLock。通常コンテキストがロガーを保持中は
//!   IF=0 なので割り込みは入らない) が、頻繁な割り込みでは出さないこと。

pub mod exceptions;
pub mod vectors;

use core::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};

use crate::arch::x86_64::trap::TrapFrame;
use crate::drivers::{apic, pic};

/// 割り込みハンドラの型。
pub type Handler = fn(&mut TrapFrame);

const NO_HANDLER: usize = 0;

static HANDLERS: [AtomicUsize; 256] = [const { AtomicUsize::new(NO_HANDLER) }; 256];
static COUNTS: [AtomicU64; 256] = [const { AtomicU64::new(0) }; 256];
static NESTING: AtomicU32 = AtomicU32::new(0);
static SPURIOUS_LAPIC: AtomicU64 = AtomicU64::new(0);
static SPURIOUS_PIC: AtomicU64 = AtomicU64::new(0);
static UNHANDLED: AtomicU64 = AtomicU64::new(0);

/// 未登録ベクタのログを出す上限 (割り込みの嵐でログが溢れないように)。
const UNHANDLED_LOG_LIMIT: u64 = 16;

/// ハンドラ登録のエラー。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegisterError {
    /// CPU 例外・PIC・スプリアス用のベクタには登録できない
    ReservedVector(u8),
    /// 既に登録済み
    AlreadyRegistered(u8),
}

impl core::fmt::Display for RegisterError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::ReservedVector(v) => write!(f, "vector {v:#x} is reserved"),
            Self::AlreadyRegistered(v) => write!(f, "vector {v:#x} already has a handler"),
        }
    }
}

/// ベクタ `vector` (0x30〜0xFE) にハンドラを登録する。
pub fn register(vector: u8, handler: Handler) -> Result<(), RegisterError> {
    if vector < vectors::ISA_IRQ_BASE || vector == vectors::SPURIOUS {
        return Err(RegisterError::ReservedVector(vector));
    }
    HANDLERS[usize::from(vector)]
        .compare_exchange(
            NO_HANDLER,
            handler as usize,
            Ordering::AcqRel,
            Ordering::Acquire,
        )
        .map(|_| ())
        .map_err(|_| RegisterError::AlreadyRegistered(vector))
}

fn handler_for(vector: u8) -> Option<Handler> {
    let raw = HANDLERS[usize::from(vector)].load(Ordering::Acquire);
    if raw == NO_HANDLER {
        return None;
    }
    // SAFETY: HANDLERS に格納するのは `register` が `Handler` (fn(&mut TrapFrame)) を
    // usize へ変換した値だけなので、同じ型の関数ポインタへ戻すのは正しい。
    Some(unsafe { core::mem::transmute::<usize, Handler>(raw) })
}

/// 割り込み/例外ハンドラの中か。
pub fn in_interrupt() -> bool {
    NESTING.load(Ordering::Relaxed) > 0
}

/// ベクタごとの受信回数。
pub fn count(vector: u8) -> u64 {
    COUNTS[usize::from(vector)].load(Ordering::Relaxed)
}

/// 割り込みの統計。
#[derive(Debug, Clone, Copy, Default)]
pub struct InterruptStats {
    /// Local APIC のスプリアス
    pub spurious_lapic: u64,
    /// 8259 のスプリアス
    pub spurious_pic: u64,
    /// 未登録ベクタ
    pub unhandled: u64,
}

/// 統計。
pub fn stats() -> InterruptStats {
    InterruptStats {
        spurious_lapic: SPURIOUS_LAPIC.load(Ordering::Relaxed),
        spurious_pic: SPURIOUS_PIC.load(Ordering::Relaxed),
        unhandled: UNHANDLED.load(Ordering::Relaxed),
    }
}

/// すべての割り込み・例外の入口。
pub(crate) fn dispatch(frame: &mut TrapFrame) {
    let vector = frame.vector as u8;
    NESTING.fetch_add(1, Ordering::Relaxed);
    COUNTS[usize::from(vector)].fetch_add(1, Ordering::Relaxed);

    match vector {
        0..=31 => exceptions::handle(frame),
        vectors::PIC_MASTER_BASE..=0x2F => legacy_pic_interrupt(vector),
        vectors::SPURIOUS => {
            // Local APIC のスプリアス割り込みには EOI を送らない (SDM 10.9)
            SPURIOUS_LAPIC.fetch_add(1, Ordering::Relaxed);
        }
        _ => {
            apic::eoi_if_in_service(vector);
            match handler_for(vector) {
                Some(handler) => handler(frame),
                None => {
                    let n = UNHANDLED.fetch_add(1, Ordering::Relaxed);
                    if n < UNHANDLED_LOG_LIMIT {
                        warn!(
                            "unhandled interrupt vector {vector:#04x} at rip={:#x} (#{})",
                            frame.rip,
                            n + 1
                        );
                    }
                }
            }
        }
    }

    NESTING.fetch_sub(1, Ordering::Relaxed);
}

/// 8259 からの割り込み (全マスクしているので、通常はスプリアスだけ)。
fn legacy_pic_interrupt(vector: u8) {
    let irq = vector - vectors::PIC_MASTER_BASE;
    match irq {
        7 if !pic::in_service(7) => {
            // マスターのスプリアス: EOI 不要
            SPURIOUS_PIC.fetch_add(1, Ordering::Relaxed);
        }
        15 if !pic::in_service(15) => {
            // スレーブのスプリアス: マスターにだけ EOI (カスケードの IRQ2 分)
            SPURIOUS_PIC.fetch_add(1, Ordering::Relaxed);
            pic::eoi_master();
        }
        _ => {
            warn!("unexpected interrupt from the masked 8259 PIC (IRQ {irq}); sending EOI");
            if irq >= 8 {
                pic::eoi_slave();
            } else {
                pic::eoi_master();
            }
        }
    }
}

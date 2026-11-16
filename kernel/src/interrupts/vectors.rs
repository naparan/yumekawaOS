//! 割り込みベクタの割り当て。
//!
//! | ベクタ | 用途 |
//! | --- | --- |
//! | `0x00`〜`0x1F` | CPU 例外 |
//! | `0x20`〜`0x2F` | レガシー 8259 PIC (リマップ後・全マスク。スプリアスのみ届き得る) |
//! | `0x30`〜`0x3F` | ISA IRQ 0〜15 (I/O APIC 経由。`0x30 + IRQ`) |
//! | `0x40`〜`0xEF` | 予約 (将来の PCI / MSI 用) |
//! | `0xF0`〜`0xFC` | 予約 (将来の IPI: スケジューラ・TLB shootdown 等) |
//! | `0xFD` | Local APIC エラー |
//! | `0xFE` | Local APIC タイマ |
//! | `0xFF` | Local APIC スプリアス |
//!
//! ベクタの上位 4bit が優先度クラスになるため、タイマ等の時間に敏感なものを高い番号に置く。

/// マスター PIC のベクタオフセット (IRQ0〜7 → 0x20〜0x27)。
pub const PIC_MASTER_BASE: u8 = 0x20;
/// スレーブ PIC のベクタオフセット (IRQ8〜15 → 0x28〜0x2F)。
pub const PIC_SLAVE_BASE: u8 = 0x28;
/// I/O APIC 経由の ISA IRQ のベクタの先頭。
pub const ISA_IRQ_BASE: u8 = 0x30;
/// ISA IRQ 1: PS/2 キーボード。
pub const KEYBOARD: u8 = ISA_IRQ_BASE + 1;
/// ISA IRQ 12: PS/2 マウス (Phase 3 で使用)。
#[allow(dead_code)] // Phase 3 (マウス) で使用
pub const MOUSE: u8 = ISA_IRQ_BASE + 12;
/// Local APIC エラー割り込み。
pub const LAPIC_ERROR: u8 = 0xFD;
/// Local APIC タイマ。
pub const LAPIC_TIMER: u8 = 0xFE;
/// Local APIC スプリアス割り込み (下位 4bit が 1111 である必要がある)。
pub const SPURIOUS: u8 = 0xFF;

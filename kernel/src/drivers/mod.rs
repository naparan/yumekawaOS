//! デバイスドライバ。
//!
//! | モジュール | デバイス |
//! | --- | --- |
//! | [`serial`] | COM1 16550 UART (カーネルログ) |
//! | [`pic`] | レガシー 8259 PIC (リマップして全マスク) |
//! | [`apic`] | Local APIC / I/O APIC |
//! | [`pit`] | 8254 PIT チャネル 2 (タイマ校正の参照) |
//! | [`pm_timer`] | ACPI PM timer (タイマ校正の参照) |
//! | [`ps2`] | i8042 PS/2 コントローラとキーボード (IRQ1) |

pub mod apic;
pub mod pic;
pub mod pit;
pub mod pm_timer;
pub mod ps2;
pub mod serial;

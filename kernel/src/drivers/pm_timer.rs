//! ACPI PM timer (3.579545 MHz の自走カウンタ, 24bit または 32bit)。
//!
//! Local APIC タイマの周波数校正の参照クロックとして使う (第 1 候補)。
//! 周波数は ACPI 仕様で固定されており、I/O ポートを読むだけで割り込みも不要。

use crate::acpi::PmTimer;
use crate::arch::x86_64::port::Port32;

/// ACPI PM timer の周波数 (Hz)。
pub const FREQUENCY_HZ: u64 = 3_579_545;

/// PM timer。
#[derive(Debug, Clone, Copy)]
pub struct AcpiPmTimer {
    port: u16,
    mask: u32,
}

impl AcpiPmTimer {
    /// FADT の情報から作る。
    pub fn new(info: PmTimer) -> Self {
        Self {
            port: info.port,
            mask: if info.is_32bit { u32::MAX } else { 0x00FF_FFFF },
        }
    }

    /// カウンタのビット幅。
    pub fn bits(&self) -> u32 {
        32 - self.mask.leading_zeros()
    }

    /// I/O ポート。
    pub fn port(&self) -> u16 {
        self.port
    }

    /// 現在値。
    pub fn read(&self) -> u32 {
        // SAFETY: FADT が示す PM timer のポート (読み取り専用のカウンタ) で、読み取りに副作用は無い。
        let mut p = unsafe { Port32::new(self.port) };
        p.read() & self.mask
    }

    /// `a` から `b` までの経過カウント (1 回の折り返しまで考慮)。
    pub fn delta(&self, a: u32, b: u32) -> u32 {
        b.wrapping_sub(a) & self.mask
    }

    /// 動いているか (短時間に値が変化するか) を確認する。
    pub fn is_running(&self) -> bool {
        let a = self.read();
        (0..1_000_000).any(|_| self.read() != a)
    }
}

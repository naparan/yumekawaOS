//! x86 の I/O ポート (Port I/O) アクセス。
//!
//! `in` / `out` 命令の `unsafe` はここに閉じ込める。
//! [`Port`] の生成を `unsafe` にし、「そのポートを排他的に所有している」
//! ことを生成時の契約とすることで、読み書き自体は safe API として提供する。

use core::arch::asm;

/// 8bit 幅の I/O ポート。
#[derive(Debug)]
pub struct Port {
    port: u16,
}

impl Port {
    /// I/O ポートハンドルを作る。
    ///
    /// # Safety
    ///
    /// - `port` は実在するデバイスのレジスタを指し、8bit アクセスが許されること。
    /// - このポートへの読み書きがメモリ安全性を損なわないこと
    ///   (例: DMA を起動するレジスタではないこと)。
    /// - 同じポートを同時に操作する別のハンドルが存在しない、または
    ///   呼び出し側がその競合を許容できること。
    pub const unsafe fn new(port: u16) -> Self {
        Self { port }
    }

    /// ポートから 1 バイト読む。
    #[inline]
    pub fn read(&mut self) -> u8 {
        let value: u8;
        // SAFETY: `Port::new` の契約により、このポートは読み取り可能な
        // デバイスレジスタであり、読み取りはメモリ安全性に影響しない。
        unsafe {
            asm!("in al, dx", out("al") value, in("dx") self.port,
                 options(nomem, nostack, preserves_flags));
        }
        value
    }

    /// ポートへ 1 バイト書く。
    #[inline]
    pub fn write(&mut self, value: u8) {
        // SAFETY: `Port::new` の契約により、このポートへの書き込みは
        // メモリ安全性に影響しない。
        unsafe {
            asm!("out dx, al", in("dx") self.port, in("al") value,
                 options(nomem, nostack, preserves_flags));
        }
    }
}

/// 32bit 幅の I/O ポート (ACPI PM timer 等)。
#[derive(Debug)]
pub struct Port32 {
    port: u16,
}

impl Port32 {
    /// I/O ポートハンドルを作る。
    ///
    /// # Safety
    ///
    /// [`Port::new`] と同じ条件 (32bit アクセスが許されること)。
    pub const unsafe fn new(port: u16) -> Self {
        Self { port }
    }

    /// ポートから 4 バイト読む。
    #[inline]
    pub fn read(&mut self) -> u32 {
        let value: u32;
        // SAFETY: `Port32::new` の契約により、読み取りはメモリ安全性に影響しない。
        unsafe {
            asm!("in eax, dx", out("eax") value, in("dx") self.port,
                 options(nomem, nostack, preserves_flags));
        }
        value
    }
}

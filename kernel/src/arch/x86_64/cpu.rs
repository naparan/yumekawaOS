//! CPU 制御命令 (cli / hlt / cpuid) の safe ラッパー。

use core::arch::asm;
use core::arch::x86_64::__cpuid;

/// 割り込みを無効化する (`cli`)。
#[inline]
pub fn disable_interrupts() {
    // SAFETY: `cli` は IF フラグを落とすだけで、メモリ安全性に影響しない。
    // Ring 0 で実行されるカーネル内からのみ呼ばれる。
    unsafe {
        asm!("cli", options(nomem, nostack));
    }
}

/// RFLAGS.IF が立っているか。
#[inline]
pub fn interrupts_enabled() -> bool {
    let rflags: u64;
    // SAFETY: pushfq/pop は自スタック上の 8 バイトを一時的に使うだけで、
    // 他のメモリに触れない。
    unsafe {
        asm!("pushfq", "pop {}", out(reg) rflags, options(nomem, preserves_flags));
    }
    rflags & (1 << 9) != 0
}

/// 割り込みを無効化して CPU を永久に停止する。
///
/// NMI 等で `hlt` から復帰しても再度 `hlt` するようループする。
pub fn halt_forever() -> ! {
    loop {
        // SAFETY: `cli; hlt` は CPU を停止させるだけで、メモリ安全性に影響しない。
        unsafe {
            asm!("cli", "hlt", options(nomem, nostack));
        }
    }
}

/// CPUID から得た CPU の識別情報。
#[derive(Debug, Clone, Copy)]
pub struct CpuIdentity {
    vendor: [u8; 12],
    brand: [u8; 48],
    has_brand: bool,
}

impl CpuIdentity {
    /// CPUID 命令で CPU のベンダー文字列とブランド文字列を取得する。
    pub fn read() -> Self {
        // x86_64 の CPU は必ず CPUID をサポートする(Long Mode の前提)。
        let leaf0 = __cpuid(0);
        let mut vendor = [0u8; 12];
        vendor[0..4].copy_from_slice(&leaf0.ebx.to_le_bytes());
        vendor[4..8].copy_from_slice(&leaf0.edx.to_le_bytes());
        vendor[8..12].copy_from_slice(&leaf0.ecx.to_le_bytes());

        let mut brand = [0u8; 48];
        let max_ext = __cpuid(0x8000_0000).eax;
        let has_brand = max_ext >= 0x8000_0004;
        if has_brand {
            for (i, leaf) in (0x8000_0002u32..=0x8000_0004).enumerate() {
                let r = __cpuid(leaf);
                let off = i * 16;
                brand[off..off + 4].copy_from_slice(&r.eax.to_le_bytes());
                brand[off + 4..off + 8].copy_from_slice(&r.ebx.to_le_bytes());
                brand[off + 8..off + 12].copy_from_slice(&r.ecx.to_le_bytes());
                brand[off + 12..off + 16].copy_from_slice(&r.edx.to_le_bytes());
            }
        }
        Self {
            vendor,
            brand,
            has_brand,
        }
    }

    /// ベンダー文字列(例: "GenuineIntel", "AuthenticAMD")。
    pub fn vendor(&self) -> &str {
        core::str::from_utf8(&self.vendor).unwrap_or("<invalid>")
    }

    /// ブランド文字列(取得できない CPU では `None`)。
    pub fn brand(&self) -> Option<&str> {
        if !self.has_brand {
            return None;
        }
        let end = self
            .brand
            .iter()
            .position(|&b| b == 0)
            .unwrap_or(self.brand.len());
        core::str::from_utf8(&self.brand[..end]).ok().map(str::trim)
    }
}

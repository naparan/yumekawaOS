//! CPU 制御命令 (cli / sti / hlt / cpuid / rdtsc / int3) の safe ラッパー。

use core::arch::asm;
use core::arch::x86_64::{__cpuid, __cpuid_count};

/// 割り込みを無効化する (`cli`)。
#[inline]
pub fn disable_interrupts() {
    // SAFETY: `cli` は IF フラグを落とすだけで、メモリ安全性に影響しない。
    // Ring 0 で実行されるカーネル内からのみ呼ばれる。
    unsafe {
        asm!("cli", options(nomem, nostack));
    }
}

/// 割り込みを有効化する (`sti`)。
///
/// IDT・割り込みコントローラの初期化が済む前に呼んではならないが、
/// それはメモリ安全性ではなく初期化順序の問題なので safe 関数としている
/// (未初期化で割り込みが来ても Limine の IDT 無し → triple fault で停止するだけ)。
#[inline]
pub fn enable_interrupts() {
    // SAFETY: `sti` は IF フラグを立てるだけで、メモリ安全性に影響しない。
    // `nomem` を付けないことで、前後のメモリアクセスの並べ替えを防ぐ
    // (割り込みハンドラと共有する状態の読み書きを跨がせない)。
    unsafe {
        asm!("sti", options(nostack));
    }
}

/// 割り込みを有効化して、次の割り込みまで停止する (`sti; hlt`)。
///
/// `sti` の直後の 1 命令は割り込みが入らない (interrupt shadow) ため、
/// 「割り込み無効のまま仕事が無いことを確認 → この関数」で、確認と停止の間に来た
/// 割り込みを取りこぼさない。
#[inline]
pub fn enable_interrupts_and_halt() {
    // SAFETY: `sti; hlt` は IF を立てて割り込みまで停止するだけで、メモリ安全性に影響しない。
    unsafe {
        asm!("sti", "hlt", options(nostack));
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

/// 割り込みを無効化した状態で `f` を実行し、元の IF 状態へ戻す。
pub fn without_interrupts<R>(f: impl FnOnce() -> R) -> R {
    let was_enabled = interrupts_enabled();
    disable_interrupts();
    let result = f();
    if was_enabled {
        enable_interrupts();
    }
    result
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

/// ブレークポイント例外 (#BP, `int3`) を発生させる。
///
/// IDT の #BP ハンドラはログを出して復帰するため、呼び出し元はそのまま継続する。
#[inline]
pub fn breakpoint() {
    // SAFETY: `int3` は #BP を発生させる。カーネルの #BP ハンドラは
    // 全レジスタを保存・復元して `int3` の次の命令へ戻るため、呼び出し元の状態は保たれる。
    unsafe {
        asm!("int3", options(nostack));
    }
}

/// ソフトウェア割り込み `int VECTOR` を発生させる (ベクタ 32 以上のみ)。
///
/// CPU 例外のベクタ (0〜31) はエラーコードの有無がスタブと食い違い得るため禁止している。
#[inline]
pub fn software_interrupt<const VECTOR: u8>() {
    const {
        assert!(
            VECTOR >= 32,
            "software interrupts are only allowed for vectors >= 32"
        )
    };
    // SAFETY: ベクタ 32 以上のスタブはエラーコードの代わりに 0 を積むので、`int n` でも
    // 正しい TrapFrame になる。ディスパッチャは全レジスタを保存・復元して戻る。
    unsafe {
        asm!("int {v}", v = const VECTOR, options(nostack));
    }
}

/// タイムスタンプカウンタ (`rdtsc`)。
#[inline]
pub fn rdtsc() -> u64 {
    let lo: u32;
    let hi: u32;
    // SAFETY: `rdtsc` はカウンタを読むだけ (CR4.TSD=0 のため Ring 0 で常に実行可能)。
    unsafe {
        asm!("rdtsc", out("eax") lo, out("edx") hi, options(nomem, nostack, preserves_flags));
    }
    (u64::from(hi) << 32) | u64::from(lo)
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

/// Phase 2 で使う CPU 機能フラグ (CPUID)。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CpuFeatures {
    /// Family / Model / Stepping (表示用, CPUID.1:EAX)
    pub signature: u32,
    /// Local APIC (CPUID.1:EDX[9])
    pub apic: bool,
    /// x2APIC (CPUID.1:ECX[21])
    pub x2apic: bool,
    /// TSC (CPUID.1:EDX[4])
    pub tsc: bool,
    /// Invariant TSC (CPUID.80000007:EDX[8])
    pub invariant_tsc: bool,
    /// MSR 命令 (CPUID.1:EDX[5])
    pub msr: bool,
    /// Page Attribute Table (CPUID.1:EDX[16])
    pub pat: bool,
    /// Global pages / CR4.PGE (CPUID.1:EDX[13])
    pub pge: bool,
    /// No-Execute (CPUID.80000001:EDX[20])
    pub nx: bool,
    /// 1GiB ページ (CPUID.80000001:EDX[26])
    pub page1gb: bool,
    /// SMEP (CPUID.(7,0):EBX[7])
    pub smep: bool,
    /// SMAP (CPUID.(7,0):EBX[20])
    pub smap: bool,
    /// UMIP (CPUID.(7,0):ECX[2])
    pub umip: bool,
    /// 物理アドレス幅 (CPUID.80000008:EAX[7:0])
    pub phys_addr_bits: u8,
    /// 線形アドレス幅 (CPUID.80000008:EAX[15:8])
    pub virt_addr_bits: u8,
    /// 初期 APIC ID (CPUID.1:EBX[31:24])
    pub initial_apic_id: u8,
}

impl CpuFeatures {
    /// CPUID で機能を調べる。
    pub fn detect() -> Self {
        let max_basic = __cpuid(0).eax;
        let max_ext = __cpuid(0x8000_0000).eax;
        let leaf1 = __cpuid(1);
        let mut f = Self {
            signature: leaf1.eax,
            apic: leaf1.edx & (1 << 9) != 0,
            x2apic: leaf1.ecx & (1 << 21) != 0,
            tsc: leaf1.edx & (1 << 4) != 0,
            msr: leaf1.edx & (1 << 5) != 0,
            pat: leaf1.edx & (1 << 16) != 0,
            pge: leaf1.edx & (1 << 13) != 0,
            initial_apic_id: (leaf1.ebx >> 24) as u8,
            phys_addr_bits: 36,
            virt_addr_bits: 48,
            ..Self::default()
        };
        if max_basic >= 7 {
            let leaf7 = __cpuid_count(7, 0);
            f.smep = leaf7.ebx & (1 << 7) != 0;
            f.smap = leaf7.ebx & (1 << 20) != 0;
            f.umip = leaf7.ecx & (1 << 2) != 0;
        }
        if max_ext >= 0x8000_0001 {
            let ext1 = __cpuid(0x8000_0001);
            f.nx = ext1.edx & (1 << 20) != 0;
            f.page1gb = ext1.edx & (1 << 26) != 0;
        }
        if max_ext >= 0x8000_0007 {
            f.invariant_tsc = __cpuid(0x8000_0007).edx & (1 << 8) != 0;
        }
        if max_ext >= 0x8000_0008 {
            let ext8 = __cpuid(0x8000_0008).eax;
            f.phys_addr_bits = (ext8 & 0xff) as u8;
            f.virt_addr_bits = ((ext8 >> 8) & 0xff) as u8;
        }
        f
    }
}

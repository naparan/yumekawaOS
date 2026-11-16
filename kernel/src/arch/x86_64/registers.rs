//! 制御レジスタ (CR0/CR2/CR3/CR4)・MSR・TLB 操作。
//!
//! 読み出しは safe、メモリ安全性に影響し得る書き込みは `unsafe fn` とする。

use core::arch::asm;

/// CR0.WP: Ring 0 でも読み取り専用ページへの書き込みを禁止する。
pub const CR0_WP: u64 = 1 << 16;
/// CR4.PGE: グローバルページ。
pub const CR4_PGE: u64 = 1 << 7;
/// CR4.UMIP: ユーザーモードでの sgdt/sidt 等を禁止。
pub const CR4_UMIP: u64 = 1 << 11;
/// CR4.SMEP: ユーザーページ上のコードを Ring 0 で実行させない。
pub const CR4_SMEP: u64 = 1 << 20;
/// CR4.SMAP: ユーザーページへの Ring 0 からのデータアクセスを (RFLAGS.AC=0 のとき) 禁止。
pub const CR4_SMAP: u64 = 1 << 21;

/// IA32_APIC_BASE MSR。
pub const MSR_APIC_BASE: u32 = 0x1B;
/// IA32_PAT MSR。
pub const MSR_PAT: u32 = 0x277;
/// IA32_EFER MSR。
pub const MSR_EFER: u32 = 0xC000_0080;
/// EFER.NXE: ページテーブルの NX ビットを有効化。
pub const EFER_NXE: u64 = 1 << 11;

/// CR0 を読む。
#[inline]
pub fn read_cr0() -> u64 {
    let v: u64;
    // SAFETY: CR0 の読み出しは副作用が無い (Ring 0)。
    unsafe { asm!("mov {}, cr0", out(reg) v, options(nomem, nostack, preserves_flags)) };
    v
}

/// CR0 を書く。
///
/// # Safety
///
/// PG/PE 等を落とすなど、実行中のコードの前提を壊す値を書かないこと。
#[inline]
pub unsafe fn write_cr0(v: u64) {
    // SAFETY: 呼び出し元の契約により、安全な値である。
    unsafe { asm!("mov cr0, {}", in(reg) v, options(nostack, preserves_flags)) };
}

/// CR2 (直近のページフォルトの線形アドレス) を読む。
#[inline]
pub fn read_cr2() -> u64 {
    let v: u64;
    // SAFETY: CR2 の読み出しは副作用が無い。
    unsafe { asm!("mov {}, cr2", out(reg) v, options(nomem, nostack, preserves_flags)) };
    v
}

/// CR3 (現在のトップレベルページテーブルの物理アドレス + フラグ) を読む。
#[inline]
pub fn read_cr3() -> u64 {
    let v: u64;
    // SAFETY: CR3 の読み出しは副作用が無い。
    unsafe { asm!("mov {}, cr3", out(reg) v, options(nomem, nostack, preserves_flags)) };
    v
}

/// CR3 を書く (アドレス空間の切り替え。非グローバルな TLB エントリは破棄される)。
///
/// # Safety
///
/// `v` は有効な PML4 の物理アドレスで、そのページテーブルは現在実行中のコード・スタック・
/// 以降アクセスするすべてのデータを正しくマップしていること。
#[inline]
pub unsafe fn write_cr3(v: u64) {
    // SAFETY: 呼び出し元の契約により、新しいページテーブルは実行継続に必要な領域をマップしている。
    unsafe { asm!("mov cr3, {}", in(reg) v, options(nostack, preserves_flags)) };
}

/// CR4 を読む。
#[inline]
pub fn read_cr4() -> u64 {
    let v: u64;
    // SAFETY: CR4 の読み出しは副作用が無い。
    unsafe { asm!("mov {}, cr4", out(reg) v, options(nomem, nostack, preserves_flags)) };
    v
}

/// CR4 を書く。
///
/// # Safety
///
/// CPU がサポートする機能ビットだけを立てること (未サポートのビットは #GP)。
/// PAE 等、実行中の前提を壊すビットを落とさないこと。
#[inline]
pub unsafe fn write_cr4(v: u64) {
    // SAFETY: 呼び出し元の契約により、サポートされた機能ビットの組み合わせである。
    unsafe { asm!("mov cr4, {}", in(reg) v, options(nostack, preserves_flags)) };
}

/// MSR を読む。
///
/// # Safety
///
/// `msr` はこの CPU に実在する MSR であること (存在しない MSR は #GP)。
#[inline]
pub unsafe fn rdmsr(msr: u32) -> u64 {
    let lo: u32;
    let hi: u32;
    // SAFETY: 呼び出し元の契約により、実在する MSR である。
    unsafe {
        asm!("rdmsr", in("ecx") msr, out("eax") lo, out("edx") hi,
             options(nomem, nostack, preserves_flags));
    }
    (u64::from(hi) << 32) | u64::from(lo)
}

/// MSR を書く。
///
/// # Safety
///
/// `msr` は実在する MSR で、`value` はそのメモリ安全性を損なわない値であること。
#[inline]
pub unsafe fn wrmsr(msr: u32, value: u64) {
    let lo = value as u32;
    let hi = (value >> 32) as u32;
    // SAFETY: 呼び出し元の契約により、実在する MSR への安全な書き込みである。
    unsafe {
        asm!("wrmsr", in("ecx") msr, in("eax") lo, in("edx") hi,
             options(nostack, preserves_flags));
    }
}

/// 1 ページ分の TLB エントリを破棄する (`invlpg`)。
#[inline]
pub fn invlpg(virt: u64) {
    // SAFETY: `invlpg` は TLB を無効化するだけで、どのアドレスに対してもメモリ安全性に影響しない
    // (次回アクセス時にページテーブルが再度参照されるだけ)。
    unsafe { asm!("invlpg [{}]", in(reg) virt, options(nostack, preserves_flags)) };
}

/// キャッシュを書き戻して無効化する (`wbinvd`)。PAT 変更時の手順で使う。
#[inline]
pub fn wbinvd() {
    // SAFETY: `wbinvd` はキャッシュの内容をメモリへ書き戻して無効化するだけで、
    // メモリの内容 (プログラムから見える値) は変わらない。
    unsafe { asm!("wbinvd", options(nostack, preserves_flags)) };
}

/// RFLAGS.AC を立てる (`stac`)。SMAP 有効時にユーザーページへのアクセスを一時的に許可する。
///
/// # Safety
///
/// CPU が SMAP をサポートしていること (未サポートだと #UD)。
/// 対になる [`clac`] を必ず実行すること (許可したままにすると SMAP の保護が無くなる)。
#[inline]
#[allow(dead_code)] // Phase 7 (copy_from_user) で使用。現状のプローブは asm 内で直接 stac/clac する
pub unsafe fn stac() {
    // SAFETY: 呼び出し元の契約により SMAP 対応 CPU であり、AC を立てるだけである。
    unsafe { asm!("stac", options(nostack)) };
}

/// RFLAGS.AC を落とす (`clac`)。
///
/// # Safety
///
/// CPU が SMAP をサポートしていること (未サポートだと #UD)。
#[inline]
#[allow(dead_code)] // Phase 7 で使用
pub unsafe fn clac() {
    // SAFETY: 呼び出し元の契約により SMAP 対応 CPU であり、AC を落とすだけである。
    unsafe { asm!("clac", options(nostack)) };
}

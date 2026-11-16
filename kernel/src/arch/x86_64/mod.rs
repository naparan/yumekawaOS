//! x86_64 固有の低レイヤー処理 (CPU 命令 / Port I/O / GDT / IDT / 割り込み入口)。

pub mod cpu;
pub mod gdt;
pub mod idt;
pub mod port;
pub mod probe;
pub mod registers;
pub mod trap;

/// CPU の保護機能の有効化結果。
#[derive(Debug, Clone, Copy, Default)]
pub struct ProtectionFeatures {
    /// CR0.WP
    pub write_protect: bool,
    /// EFER.NXE
    pub nx: bool,
    /// CR4.PGE
    pub global_pages: bool,
    /// CR4.SMEP
    pub smep: bool,
    /// CR4.SMAP
    pub smap: bool,
    /// CR4.UMIP
    pub umip: bool,
}

/// CR0.WP / EFER.NXE / CR4.PGE / SMEP / SMAP / UMIP を、CPU が対応していれば有効化する。
///
/// - WP: Ring 0 でも読み取り専用ページへ書けなくする (W^X をカーネル自身にも効かせる)。
/// - SMEP: ユーザーページのコードを Ring 0 で実行させない。
/// - SMAP: ユーザーページへの Ring 0 からのアクセスを、`stac`〜`clac` の間以外で禁止する。
///   (Phase 7 の `copy_from_user` は `stac`/`clac` で囲む。docs/architecture.md 参照)
/// - UMIP: ユーザーモードの sgdt/sidt 等を禁止する。
///
/// Phase 2 時点でカーネルはユーザーページを一切マップしていないため、SMEP/SMAP を
/// 有効にしても既存の動作には影響しない。
pub fn enable_protection(features: &cpu::CpuFeatures) -> ProtectionFeatures {
    let mut result = ProtectionFeatures::default();

    let cr0 = registers::read_cr0();
    // SAFETY: WP を立てるだけ。カーネルは読み取り専用ページへ書き込まない (W^X)。
    unsafe { registers::write_cr0(cr0 | registers::CR0_WP) };
    result.write_protect = registers::read_cr0() & registers::CR0_WP != 0;

    if features.nx && features.msr {
        // SAFETY: CPUID で NX と MSR のサポートを確認済みなので EFER は存在し、NXE を立てられる。
        let efer = unsafe { registers::rdmsr(registers::MSR_EFER) };
        // SAFETY: 同上。NXE を立てるだけで、他のビット (LME/LMA/SCE) は保つ。
        unsafe { registers::wrmsr(registers::MSR_EFER, efer | registers::EFER_NXE) };
        // SAFETY: 同上。
        result.nx = unsafe { registers::rdmsr(registers::MSR_EFER) } & registers::EFER_NXE != 0;
    }

    let mut cr4 = registers::read_cr4();
    if features.pge {
        cr4 |= registers::CR4_PGE;
    }
    if features.smep {
        cr4 |= registers::CR4_SMEP;
    }
    if features.smap {
        cr4 |= registers::CR4_SMAP;
    }
    if features.umip {
        cr4 |= registers::CR4_UMIP;
    }
    // SAFETY: CPUID で確認した機能ビットだけを追加で立てる。既存のビット (PAE 等) は保つ。
    unsafe { registers::write_cr4(cr4) };
    let cr4 = registers::read_cr4();
    result.global_pages = cr4 & registers::CR4_PGE != 0;
    result.smep = cr4 & registers::CR4_SMEP != 0;
    result.smap = cr4 & registers::CR4_SMAP != 0;
    result.umip = cr4 & registers::CR4_UMIP != 0;
    result
}

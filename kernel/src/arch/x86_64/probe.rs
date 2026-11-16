//! フォルトしても復帰できるメモリアクセス (「プローブ」) と例外テーブル。
//!
//! Linux の exception table と同じ仕組み: フォルトし得る命令のアドレスと、フォルト時の
//! 復帰先 (fixup) の組を `.yk_extable` セクションに置いておき、#PF / #GP ハンドラは
//! 例外を起こした RIP が表にあれば RIP を fixup へ書き換えて復帰する。
//!
//! 用途:
//! - Phase 2: 例外・メモリ保護のセルフテスト (unmap 後のアクセス、W^X、GPF など)
//! - Phase 7: `copy_from_user` / `copy_to_user` (ユーザーが渡した不正なポインタで
//!   カーネルを落とさない) — `stac`/`clac` 付きの版をここに追加する想定。
//!
//! 実行プローブ ([`exec`]) は、フォルトした RIP が「呼び出し先」になるため表では扱えない。
//! 代わりに対象アドレスを [`EXEC_PROBE_TARGET`] に登録し、#PF ハンドラが
//! 「そのアドレスでの命令フェッチのフォルト」を見つけたら呼び出し元へ戻す。

use core::arch::global_asm;
use core::sync::atomic::{AtomicU64, Ordering};

use crate::sync::IrqSpinLock;

/// フォルトの記録 (プローブが失敗したときに返す)。
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Fault {
    /// 例外ベクタ (14 = #PF, 13 = #GP)
    pub vector: u8,
    /// エラーコード
    pub error_code: u64,
    /// #PF のときの CR2 (フォルトアドレス)。それ以外は 0
    pub address: u64,
    /// フォルトした命令
    pub rip: u64,
}

impl core::fmt::Debug for Fault {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "Fault {{ vector: {}, error_code: {:#x}, address: {:#x}, rip: {:#x} }}",
            self.vector, self.error_code, self.address, self.rip
        )
    }
}

/// 直近に回復したフォルト。プローブ関数が結果を組み立てるのに使う。
static LAST_FAULT: IrqSpinLock<Option<Fault>> = IrqSpinLock::new(None);

/// 実行プローブの対象アドレス (0 = 無効)。
pub static EXEC_PROBE_TARGET: AtomicU64 = AtomicU64::new(0);

#[repr(C)]
struct ProbeResult {
    value: u64,
    faulted: u64,
}

// プローブ関数本体。フォルトし得る命令 (.Lyk_probe_*_insn) と復帰先 (.Lyk_probe_*_fixup) を
// .yk_extable に登録する。ラベル名は 0/1 だけの数字ラベルを避ける (Intel 構文で 2 進数と誤認される)。
global_asm!(
    ".section .text.yk_probe, \"ax\", @progbits",
    // ProbeResult yk_probe_read_u8(u64 addr)
    ".global yk_probe_read_u8",
    "yk_probe_read_u8:",
    ".Lyk_probe_read_insn:",
    "    movzx eax, byte ptr [rdi]",
    "    xor edx, edx",
    "    ret",
    ".Lyk_probe_read_fixup:",
    "    xor eax, eax",
    "    mov edx, 1",
    "    ret",
    // ProbeResult yk_probe_write_u8(u64 addr, u64 value)
    ".global yk_probe_write_u8",
    "yk_probe_write_u8:",
    ".Lyk_probe_write_insn:",
    "    mov byte ptr [rdi], sil",
    "    xor eax, eax",
    "    xor edx, edx",
    "    ret",
    ".Lyk_probe_write_fixup:",
    "    xor eax, eax",
    "    mov edx, 1",
    "    ret",
    // ProbeResult yk_probe_read_u8_user(u64 addr) — SMAP 対応 CPU 専用 (stac/clac)
    ".global yk_probe_read_u8_user",
    "yk_probe_read_u8_user:",
    "    stac",
    ".Lyk_probe_read_user_insn:",
    "    movzx eax, byte ptr [rdi]",
    "    clac",
    "    xor edx, edx",
    "    ret",
    ".Lyk_probe_read_user_fixup:",
    "    clac",
    "    xor eax, eax",
    "    mov edx, 1",
    "    ret",
    // ProbeResult yk_probe_read_u64(u64 addr)
    ".global yk_probe_read_u64",
    "yk_probe_read_u64:",
    ".Lyk_probe_read64_insn:",
    "    mov rax, qword ptr [rdi]",
    "    xor edx, edx",
    "    ret",
    ".Lyk_probe_read64_fixup:",
    "    xor eax, eax",
    "    mov edx, 1",
    "    ret",
    // ProbeResult yk_probe_ud2(void): #UD を起こす
    ".global yk_probe_ud2",
    "yk_probe_ud2:",
    ".Lyk_probe_ud2_insn:",
    "    ud2",
    "    xor eax, eax",
    "    xor edx, edx",
    "    ret",
    ".Lyk_probe_ud2_fixup:",
    "    xor eax, eax",
    "    mov edx, 1",
    "    ret",
    // ProbeResult yk_probe_divide(u64 dividend, u64 divisor): 64bit 除算 (divisor=0 で #DE)
    ".global yk_probe_divide",
    "yk_probe_divide:",
    "    mov rax, rdi",
    "    xor edx, edx",
    ".Lyk_probe_div_insn:",
    "    div rsi",
    "    xor edx, edx",
    "    ret",
    ".Lyk_probe_div_fixup:",
    "    xor eax, eax",
    "    mov edx, 1",
    "    ret",
    // u64 yk_probe_exec(u64 target): target を call する。戻れば 0、フォルトなら
    // ハンドラが rax=1 にして呼び出し元 (call の次) へ戻す。
    ".global yk_probe_exec",
    "yk_probe_exec:",
    "    xor eax, eax",
    "    call rdi",
    "    ret",
    ".section .yk_extable, \"a\", @progbits",
    ".p2align 3",
    "    .quad .Lyk_probe_read_insn, .Lyk_probe_read_fixup",
    "    .quad .Lyk_probe_write_insn, .Lyk_probe_write_fixup",
    "    .quad .Lyk_probe_read_user_insn, .Lyk_probe_read_user_fixup",
    "    .quad .Lyk_probe_read64_insn, .Lyk_probe_read64_fixup",
    "    .quad .Lyk_probe_ud2_insn, .Lyk_probe_ud2_fixup",
    "    .quad .Lyk_probe_div_insn, .Lyk_probe_div_fixup",
);

unsafe extern "C" {
    fn yk_probe_read_u8(addr: u64) -> ProbeResult;
    fn yk_probe_write_u8(addr: u64, value: u64) -> ProbeResult;
    fn yk_probe_read_u8_user(addr: u64) -> ProbeResult;
    fn yk_probe_read_u64(addr: u64) -> ProbeResult;
    fn yk_probe_ud2() -> ProbeResult;
    fn yk_probe_divide(dividend: u64, divisor: u64) -> ProbeResult;
    fn yk_probe_exec(target: u64) -> u64;
    static __yk_extable_start: [u64; 0];
    static __yk_extable_end: [u64; 0];
}

/// 例外ハンドラから呼ぶ: `rip` が例外テーブルにあれば fixup アドレスを返す。
pub fn search_exception_table(rip: u64) -> Option<u64> {
    let start = (&raw const __yk_extable_start).cast::<u64>();
    let end = (&raw const __yk_extable_end).cast::<u64>();
    let words = (end as usize - start as usize) / 8;
    // SAFETY: [start, end) はリンカスクリプトが定義する .yk_extable セクション
    // (読み取り専用、8 バイト境界、u64 の組の並び) で、カーネルの寿命の間変わらない。
    let table = unsafe { core::slice::from_raw_parts(start, words) };
    table
        .chunks_exact(2)
        .find(|entry| entry[0] == rip)
        .map(|entry| entry[1])
}

/// 例外テーブルのエントリ数 (診断用)。
pub fn exception_table_len() -> usize {
    let start = (&raw const __yk_extable_start) as usize;
    let end = (&raw const __yk_extable_end) as usize;
    (end - start) / 16
}

/// 例外ハンドラから呼ぶ: 回復したフォルトを記録する。
pub fn record_fault(fault: Fault) {
    // 例外ハンドラ内 (割り込み無効) で取る。プローブ中にこのロックを持つコードは無い。
    if let Some(mut slot) = LAST_FAULT.try_lock() {
        *slot = Some(fault);
    }
}

fn take_fault() -> Option<Fault> {
    LAST_FAULT.lock().take()
}

fn finish(result: ProbeResult) -> Result<u8, Fault> {
    finish_u64(result).map(|v| v as u8)
}

fn finish_u64(result: ProbeResult) -> Result<u64, Fault> {
    if result.faulted == 0 {
        Ok(result.value)
    } else {
        Err(take_fault().unwrap_or(Fault {
            vector: 0xff,
            error_code: 0,
            address: 0,
            rip: 0,
        }))
    }
}

/// `addr` から 1 バイト読む。フォルトしたらその内容を返す。
///
/// どのアドレスに対してもメモリ安全 (フォルトしてもカーネルは継続する) だが、
/// MMIO を読むとデバイスに副作用があり得るので、テストと診断以外では使わないこと。
pub fn read_u8(addr: u64) -> Result<u8, Fault> {
    // SAFETY: yk_probe_read_u8 は 1 バイト読むだけで、フォルトは例外テーブルで回復する。
    // 読み取りは Rust の値の不変条件を壊さない。
    finish(unsafe { yk_probe_read_u8(addr) })
}

/// `addr` から 8 バイト読む (8 バイト境界でなくてもよい)。フォルトしたらその内容を返す。
///
/// パニック時のバックトレース (壊れているかもしれないフレームポインタを辿る) にも使う。
pub fn read_u64(addr: u64) -> Result<u64, Fault> {
    // SAFETY: yk_probe_read_u64 は 8 バイト読むだけで、フォルトは例外テーブルで回復する。
    finish_u64(unsafe { yk_probe_read_u64(addr) })
}

/// `ud2` を実行して #UD (Invalid Opcode) を起こし、例外ハンドラ経由で復帰する (テスト用)。
pub fn invalid_opcode() -> Result<(), Fault> {
    // SAFETY: ud2 は必ず #UD になり、例外テーブルで fixup へ戻る。メモリには触れない。
    finish_u64(unsafe { yk_probe_ud2() }).map(|_| ())
}

/// `dividend / divisor` を `div` 命令で計算する。`divisor == 0` なら #DE になり、その内容を返す (テスト用)。
pub fn divide(dividend: u64, divisor: u64) -> Result<u64, Fault> {
    // SAFETY: div はレジスタだけを使い、#DE は例外テーブルで fixup へ戻る。
    finish_u64(unsafe { yk_probe_divide(dividend, divisor) })
}

/// `addr` へ 1 バイト書く。フォルトしたらその内容を返す。
///
/// # Safety
///
/// 書き込みが**成功した場合**に Rust から見えるメモリを壊さないこと
/// (フォルトする想定のアドレス、またはテスト専用に確保した領域であること)。
pub unsafe fn write_u8(addr: u64, value: u8) -> Result<(), Fault> {
    // SAFETY: 呼び出し元の契約により、書き込みが成功してもメモリ安全性を損なわない。
    finish(unsafe { yk_probe_write_u8(addr, u64::from(value)) }).map(|_| ())
}

/// SMAP を一時的に解除 (`stac`) して `addr` から 1 バイト読む。
///
/// # Safety
///
/// CPU が SMAP をサポートしていること (未サポートでは `stac` が #UD)。
pub unsafe fn read_u8_user(addr: u64) -> Result<u8, Fault> {
    // SAFETY: 呼び出し元の契約により stac/clac が使える。読み取りのみ。
    finish(unsafe { yk_probe_read_u8_user(addr) })
}

/// `target` を関数として呼ぶ。命令フェッチでフォルトしたら (NX / SMEP 等) その内容を返す。
///
/// # Safety
///
/// `target` で実行が**成功した場合**に安全であること (例: `ret` 1 命令だけのページ)。
pub unsafe fn exec(target: u64) -> Result<(), Fault> {
    EXEC_PROBE_TARGET.store(target, Ordering::SeqCst);
    // SAFETY: 呼び出し元の契約により target の実行は安全。フォルトした場合は #PF ハンドラが
    // EXEC_PROBE_TARGET を見て yk_probe_exec の `call` の次へ戻す。
    let r = unsafe { yk_probe_exec(target) };
    EXEC_PROBE_TARGET.store(0, Ordering::SeqCst);
    if r == 0 {
        Ok(())
    } else {
        Err(take_fault().unwrap_or(Fault {
            vector: 0xff,
            error_code: 0,
            address: 0,
            rip: 0,
        }))
    }
}

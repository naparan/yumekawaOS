//! 例外とメモリ保護のセルフテスト。
//!
//! 各テストは「フォルトしても復帰できるアクセス」([`probe`]) を使い、CPU が実際に例外を
//! 出したこと (ベクタ・エラーコード・CR2) を確かめる。最後にカーネルが継続していることを確認する。

use core::sync::atomic::{AtomicU32, Ordering};

use super::Suite;
use crate::arch::x86_64::ProtectionFeatures;
use crate::arch::x86_64::cpu::{self, CpuFeatures};
use crate::arch::x86_64::probe::{self, Fault};
use crate::interrupts::{self, exceptions};
use crate::memory::paging::PageFlags;
use crate::memory::vm::{KernelMapping, LowerHalfTestPage};
use crate::memory::{VirtAddr, hhdm, kernel_space};

const PF: u8 = 14;
const GP: u8 = 13;
const ERR_PRESENT: u64 = 1 << 0;
const ERR_WRITE: u64 = 1 << 1;
const ERR_USER: u64 = 1 << 2;
const ERR_FETCH: u64 = 1 << 4;

/// .rodata に置かれる定数 (W^X / NX の検査対象)。`ret` 命令のバイトを含む。
static RODATA_RET: [u8; 16] = [0xC3; 16];

fn is_pf(r: &Result<impl Sized, Fault>, addr: u64, must: u64, must_not: u64) -> bool {
    matches!(r, Err(f) if f.vector == PF && f.address == addr && f.error_code & must == must && f.error_code & must_not == 0)
}

pub(super) fn run(s: &mut Suite, features: &CpuFeatures, protection: &ProtectionFeatures) {
    // #BP: int3 から復帰して継続する
    let before = exceptions::breakpoint_count();
    cpu::breakpoint();
    let after = exceptions::breakpoint_count();
    s.check(
        "breakpoint_resume",
        after == before + 1,
        format_args!("breakpoint count {before} -> {after}, execution continued after int3"),
    );

    // #UD / #DE: 例外ハンドラが個別に処理し、例外テーブルで復帰できる
    let r = probe::invalid_opcode();
    s.check(
        "invalid_opcode",
        matches!(r, Err(f) if f.vector == 6),
        format_args!("ud2 -> {r:?}"),
    );
    let ok = probe::divide(100, 7);
    let r = probe::divide(100, core::hint::black_box(0));
    s.check(
        "divide_error",
        ok == Ok(14) && matches!(r, Err(f) if f.vector == 0),
        format_args!("100 / 7 -> {ok:?}; 100 / 0 -> {r:?}"),
    );

    // 未登録ベクタ: ログを出して数え、Local APIC の ISR に無いので EOI は送らずに戻る
    let before = interrupts::stats().unhandled;
    cpu::software_interrupt::<0x40>();
    let after = interrupts::stats().unhandled;
    s.check(
        "unregistered_vector",
        after == before + 1,
        format_args!("int 0x40 -> unhandled count {before} -> {after}, execution continued"),
    );

    // #PF: 未マップのアドレスの読み/書き (CR2 が一致)
    let unmapped = match KernelMapping::new_anonymous(1, PageFlags::KERNEL_RW) {
        Ok(m) => m.unmap().as_u64() + 0x10,
        Err(e) => {
            s.check("page_fault_read", false, format_args!("{e}"));
            return;
        }
    };
    let r = probe::read_u8(unmapped);
    s.check(
        "page_fault_read",
        is_pf(&r, unmapped, 0, ERR_PRESENT | ERR_WRITE),
        format_args!("read {unmapped:#x} -> {r:?}"),
    );
    // SAFETY: 未マップのアドレスなので書き込みは必ずフォルトする。
    let r = unsafe { probe::write_u8(unmapped, 1) };
    s.check(
        "page_fault_write",
        is_pf(&r, unmapped, ERR_WRITE, ERR_PRESENT),
        format_args!("write {unmapped:#x} -> {r:?}"),
    );

    // #GP: 非正規形アドレス
    let noncanonical = 0x8000_0000_0000_0000u64;
    let r = probe::read_u8(noncanonical);
    s.check(
        "general_protection_noncanonical",
        matches!(r, Err(f) if f.vector == GP),
        format_args!("read {noncanonical:#x} -> {r:?}"),
    );

    // W^X: .rodata と .text への書き込みは保護違反の #PF (CR0.WP により Ring 0 でも)
    let ro = RODATA_RET.as_ptr() as u64 + 3;
    // SAFETY: 読み取り専用ページなので書き込みはフォルトする (成功したら W^X の欠陥として FAIL)。
    let r = unsafe { probe::write_u8(ro, 0x90) };
    s.check(
        "wx_rodata_write_faults",
        is_pf(&r, ro, ERR_PRESENT | ERR_WRITE, 0) && RODATA_RET[3] == 0xC3,
        format_args!("write {ro:#x} (.rodata) -> {r:?}"),
    );
    let text = run as *const () as u64;
    // SAFETY: 実行専用ページなので書き込みはフォルトする。
    let r = unsafe { probe::write_u8(text, 0xCC) };
    s.check(
        "wx_text_write_faults",
        is_pf(&r, text, ERR_PRESENT | ERR_WRITE, 0),
        format_args!("write {text:#x} (.text) -> {r:?}"),
    );
    // HHDM 側のカーネルイメージの別名も書き込み不可
    match kernel_space::translate(VirtAddr::new_canonical(text)) {
        Some(t) => {
            let alias = hhdm::phys_to_virt(t.phys).as_u64();
            // SAFETY: 読み取り専用の別名なので書き込みはフォルトする。
            let r = unsafe { probe::write_u8(alias, 0xCC) };
            s.check(
                "wx_text_alias_write_faults",
                is_pf(&r, alias, ERR_PRESENT | ERR_WRITE, 0),
                format_args!("write {alias:#x} (HHDM alias of .text) -> {r:?}"),
            );
        }
        None => s.check(
            "wx_text_alias_write_faults",
            false,
            format_args!("text not mapped"),
        ),
    }

    // 実行の対照実験: RX ページの ret は実行できる
    let rx = KernelMapping::new_anonymous(1, PageFlags::KERNEL_RW).and_then(|mut m| {
        if let Some(b) = m.as_mut_slice() {
            b[0] = 0xC3;
        }
        m.set_flags(PageFlags::KERNEL_RX).map(|()| m)
    });
    match &rx {
        Ok(m) => {
            // SAFETY: ret 1 命令だけのページ。
            let r = unsafe { probe::exec(m.base().as_u64()) };
            s.check(
                "exec_rx_page_ok",
                r.is_ok(),
                format_args!("call {} (RX, contains ret) -> {r:?}", m.base()),
            );
        }
        Err(e) => s.check("exec_rx_page_ok", false, format_args!("{e}")),
    }

    if !protection.nx {
        s.skip("nx_data_exec_faults", "NX is not supported by this CPU");
        s.skip("nx_rodata_exec_faults", "NX is not supported by this CPU");
    } else {
        // NX: データページ (RW) の実行
        match KernelMapping::new_anonymous(1, PageFlags::KERNEL_RW) {
            Ok(mut m) => {
                if let Some(b) = m.as_mut_slice() {
                    b[0] = 0xC3;
                }
                let addr = m.base().as_u64();
                // SAFETY: ret 1 命令だけのページ (NX なので実行できないはず)。
                let r = unsafe { probe::exec(addr) };
                s.check(
                    "nx_data_exec_faults",
                    is_pf(&r, addr, ERR_PRESENT | ERR_FETCH, 0),
                    format_args!("call {addr:#x} (RW, NX) -> {r:?}"),
                );
            }
            Err(e) => s.check("nx_data_exec_faults", false, format_args!("{e}")),
        }
        // NX: .rodata の実行
        let addr = RODATA_RET.as_ptr() as u64;
        // SAFETY: ret 命令だけを含む定数 (NX なので実行できないはず)。
        let r = unsafe { probe::exec(addr) };
        s.check(
            "nx_rodata_exec_faults",
            is_pf(&r, addr, ERR_PRESENT | ERR_FETCH, 0),
            format_args!("call {addr:#x} (.rodata) -> {r:?}"),
        );
    }
    drop(rx);

    // SMEP / SMAP: カーネルから user ページを実行・参照できない
    let user_va = VirtAddr::new_canonical(0x0000_0020_0000_0000);
    if protection.smep {
        match LowerHalfTestPage::new(user_va, &[0xC3], PageFlags::USER_RX) {
            Ok(page) => {
                let addr = page.addr().as_u64();
                // SAFETY: ret 1 命令だけのページ (SMEP により実行できないはず)。
                let r = unsafe { probe::exec(addr) };
                s.check(
                    "smep_user_exec_faults",
                    is_pf(&r, addr, ERR_PRESENT | ERR_FETCH, ERR_USER),
                    format_args!("call {addr:#x} (user page) -> {r:?}"),
                );
            }
            Err(e) => s.check("smep_user_exec_faults", false, format_args!("{e}")),
        }
    } else {
        s.skip(
            "smep_user_exec_faults",
            if features.smep {
                "SMEP could not be enabled"
            } else {
                "CPU does not support SMEP"
            },
        );
    }
    if protection.smap {
        match LowerHalfTestPage::new(user_va, &[0x42], PageFlags::USER_RW) {
            Ok(page) => {
                let addr = page.addr().as_u64();
                let denied = probe::read_u8(addr);
                // SAFETY: SMAP が有効 (= stac/clac が使える) ことを確認済み。
                let allowed = unsafe { probe::read_u8_user(addr) };
                s.check(
                    "smap_user_access",
                    is_pf(&denied, addr, ERR_PRESENT, ERR_USER) && allowed == Ok(0x42),
                    format_args!(
                        "read {addr:#x} without stac -> {denied:?}; with stac/clac -> {allowed:?}"
                    ),
                );
            }
            Err(e) => s.check("smap_user_access", false, format_args!("{e}")),
        }
    } else {
        s.skip(
            "smap_user_access",
            if features.smap {
                "SMAP could not be enabled"
            } else {
                "CPU does not support SMAP"
            },
        );
    }

    s.check(
        "kernel_continues",
        true,
        format_args!(
            "{} faults recovered via the exception table / exec probe; kernel still running",
            exceptions::recovered_fault_count()
        ),
    );
}

static DEPTH: AtomicU32 = AtomicU32::new(0);

/// スタックを使い切るまで再帰する (戻らない)。
#[inline(never)]
fn recurse(level: u32) -> u32 {
    if level == u32::MAX {
        return 0;
    }
    let mut frame = [0u8; 1024];
    frame[0] = level as u8;
    let frame = core::hint::black_box(frame);
    DEPTH.store(level, Ordering::Relaxed);
    // 末尾呼び出しにならないよう、戻り値を使う
    let deeper = recurse(core::hint::black_box(level + 1));
    deeper.wrapping_add(u32::from(frame[0]))
}

/// カーネルスタックを溢れさせる (selftest=stackoverflow)。ガードページ → #DF → Kernel Panic。
pub(super) fn overflow_the_stack() -> ! {
    warn!("SELFTEST stackoverflow: recursing until the kernel stack guard page is hit");
    let r = recurse(0);
    // ここには来ない
    panic!(
        "stack overflow was not detected (recursion returned {r}, depth {})",
        DEPTH.load(Ordering::Relaxed)
    );
}

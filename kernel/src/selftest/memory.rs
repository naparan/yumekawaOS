//! メモリ管理のセルフテスト: フレームアロケータ・ページマッパ・ヒープ。

use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::alloc::Layout;

use super::Suite;
use crate::arch::x86_64::probe;
use crate::memory::addr::{PAGE_SIZE, PageSize, PhysFrame};
use crate::memory::frame::{self, FrameError};
use crate::memory::paging::{CacheMode, PageFlags};
use crate::memory::vm::KernelMapping;
use crate::memory::{PhysAddr, VirtAddr, heap, hhdm, kernel_space, layout};

/// フレームの先頭 8 バイトに HHDM 経由で書く (テスト用)。
fn poke_frame(frame: PhysFrame, value: u64) {
    let p = hhdm::phys_to_virt(frame.start()).as_mut_ptr::<u64>();
    // SAFETY: frame はこのテストが確保したばかりのフレームで、他に使われていない。
    // HHDM で書き込み可能にマップされ、4KiB 境界なので u64 として整列している。
    unsafe { p.write_volatile(value) };
}

fn peek_frame(frame: PhysFrame) -> u64 {
    let p = hhdm::phys_to_virt(frame.start()).as_ptr::<u64>();
    // SAFETY: poke_frame と同じ (読み取りのみ)。
    unsafe { p.read_volatile() }
}

/// テストで確保したフレームを返す。
fn release(frame: PhysFrame) -> Result<(), FrameError> {
    // SAFETY: テストが確保したフレームで、マップもせず (HHDM 経由で触っただけ) 参照も残っていない。
    unsafe { frame::free(frame) }
}

pub(super) fn run(s: &mut Suite) {
    frame_tests(s);
    paging_tests(s);
    heap_tests(s);
}

fn frame_tests(s: &mut Suite) {
    let before = frame::stats().unwrap_or_default();

    // 1 フレームの確保と解放
    match frame::alloc() {
        Ok(f) => {
            let allocated = frame::is_allocated(f) == Some(true);
            poke_frame(f, 0x7975_6d65_6b61_7761);
            let readback = peek_frame(f);
            let freed = release(f).is_ok();
            let after_free = frame::is_allocated(f) == Some(false);
            s.check(
                "frame.alloc_free",
                allocated && readback == 0x7975_6d65_6b61_7761 && freed && after_free,
                format_args!(
                    "frame {:#x}: allocated={allocated} readback_ok={} freed={freed} free_after={after_free}",
                    f.start().as_u64(),
                    readback == 0x7975_6d65_6b61_7761
                ),
            );
        }
        Err(e) => s.check("frame.alloc_free", false, format_args!("alloc failed: {e}")),
    }

    // 大量確保 (8MiB 分): 重複なし・整列・内容が混ざらない
    const BULK: usize = 2048;
    let mut frames: Vec<PhysFrame> = Vec::with_capacity(BULK);
    let mut alloc_error = None;
    for _ in 0..BULK {
        match frame::alloc() {
            Ok(f) => frames.push(f),
            Err(e) => {
                alloc_error = Some(e);
                break;
            }
        }
    }
    for (i, f) in frames.iter().enumerate() {
        poke_frame(*f, i as u64 ^ 0xA5A5_0000);
    }
    let contents_ok = frames
        .iter()
        .enumerate()
        .all(|(i, f)| peek_frame(*f) == i as u64 ^ 0xA5A5_0000);
    let aligned = frames.iter().all(|f| f.start().is_aligned(PAGE_SIZE));
    let mut sorted: Vec<u64> = frames.iter().map(|f| f.number()).collect();
    sorted.sort_unstable();
    sorted.dedup();
    let unique = sorted.len() == frames.len();
    let mid = frame::stats().unwrap_or_default();
    let mut free_errors = 0;
    for f in frames.drain(..) {
        if release(f).is_err() {
            free_errors += 1;
        }
    }
    let after = frame::stats().unwrap_or_default();
    s.check(
        "frame.bulk_2048",
        alloc_error.is_none()
            && contents_ok
            && aligned
            && unique
            && free_errors == 0
            && mid.free_frames + BULK as u64 == before.free_frames
            && after.free_frames == before.free_frames,
        format_args!(
            "allocated={} unique={unique} aligned={aligned} contents_ok={contents_ok} free_errors={free_errors} free before/during/after={}/{}/{} alloc_error={:?}",
            sorted.len(),
            before.free_frames,
            mid.free_frames,
            after.free_frames,
            alloc_error
        ),
    );

    // 連続確保 (2MiB = 512 フレーム, 2MiB 境界)
    match frame::alloc_contiguous(512, 512) {
        Ok(start) => {
            let aligned = start.start().is_aligned(2 * 1024 * 1024);
            let all_used = (0..512).all(|i| {
                PhysFrame::from_number(start.number() + i).and_then(frame::is_allocated)
                    == Some(true)
            });
            // SAFETY: 今確保した連続フレームで、どこにもマップしていない。
            let freed = unsafe { frame::free_contiguous(start, 512) }.is_ok();
            let restored = frame::stats().unwrap_or_default().free_frames == before.free_frames;
            s.check(
                "frame.contiguous_2mib",
                aligned && all_used && freed && restored,
                format_args!(
                    "start={:#x} aligned_2mib={aligned} all_allocated={all_used} freed={freed} restored={restored}",
                    start.start().as_u64()
                ),
            );
        }
        Err(e) => s.check(
            "frame.contiguous_2mib",
            false,
            format_args!("alloc_contiguous failed: {e}"),
        ),
    }

    // 二重解放の検出
    match frame::alloc() {
        Ok(f) => {
            let first = release(f);
            let second = release(f);
            s.check(
                "frame.double_free_detected",
                first.is_ok() && second == Err(FrameError::DoubleFree(f.start())),
                format_args!("first={first:?} second={second:?}"),
            );
        }
        Err(e) => s.check("frame.double_free_detected", false, format_args!("{e}")),
    }

    // 確保対象外のフレームの解放は拒否される (フレーム 0 / カーネルイメージ)
    let zero = PhysFrame::from_number(0).unwrap_or_else(|| unreachable!());
    let r0 = release(zero);
    let kernel_phys = kernel_space::translate(VirtAddr::new_canonical(run as *const () as u64))
        .map(|t| t.phys.align_down(PAGE_SIZE));
    let rk = kernel_phys.and_then(PhysFrame::from_start).map(release);
    s.check(
        "frame.reject_non_allocatable",
        matches!(r0, Err(FrameError::NotAllocatable(_)))
            && matches!(rk, Some(Err(FrameError::NotAllocatable(_)))),
        format_args!("frame 0 -> {r0:?}, kernel text frame -> {rk:?}"),
    );

    // 統計の整合性
    let st = frame::stats().unwrap_or_default();
    s.check(
        "frame.stats",
        st.free_frames <= st.total_frames
            && st.reclaimed_frames > 0
            && st.free_frames == before.free_frames,
        format_args!(
            "total={} free={} used={} reclaimed={} bitmap={} regions={}",
            st.total_frames,
            st.free_frames,
            st.used_frames(),
            st.reclaimed_frames,
            st.bitmap_frames,
            st.regions
        ),
    );
}

fn paging_tests(s: &mut Suite) {
    let frames_before = frame::stats().unwrap_or_default().free_frames;

    // map → 書き込み → translate → HHDM 別名で確認 → unmap → アクセスで #PF
    match KernelMapping::new_anonymous(4, PageFlags::KERNEL_RW) {
        Ok(mut m) => {
            let base = m.base();
            if let Some(buf) = m.as_mut_slice() {
                for (i, b) in buf.iter_mut().enumerate() {
                    *b = (i % 251) as u8;
                }
            }
            let mut translate_ok = true;
            let mut alias_ok = true;
            for page in 0..4u64 {
                let v = VirtAddr::new_canonical(base.as_u64() + page * PAGE_SIZE + 123);
                match kernel_space::translate(v) {
                    Some(t) => {
                        translate_ok &= t.size == PageSize::Size4K
                            && t.flags.writable
                            && !t.flags.executable
                            && t.flags.cache == CacheMode::WriteBack
                            && Some(t.phys.align_down(PAGE_SIZE)) == m.phys_of_page(page)
                            && t.phys.as_u64() % PAGE_SIZE == 123;
                        // 物理フレームを HHDM から読んでも同じ値か
                        let alias = hhdm::phys_to_virt(t.phys).as_u64();
                        let expected = ((page * PAGE_SIZE + 123) % 251) as u8;
                        alias_ok &= probe::read_u8(alias) == Ok(expected);
                    }
                    None => translate_ok = false,
                }
            }
            let gone = m.unmap();
            let fault = probe::read_u8(gone.as_u64() + 8);
            let pf_ok = matches!(fault, Err(f) if f.vector == 14 && f.address == gone.as_u64() + 8 && f.error_code & 1 == 0);
            s.check(
                "paging.map_write_translate_unmap",
                translate_ok && alias_ok && pf_ok && kernel_space::translate(gone).is_none(),
                format_args!(
                    "va={base}: translate_ok={translate_ok} hhdm_alias_ok={alias_ok}; after unmap read -> {fault:?}"
                ),
            );
        }
        Err(e) => s.check(
            "paging.map_write_translate_unmap",
            false,
            format_args!("new_anonymous failed: {e}"),
        ),
    }

    // HHDM が大きいページでマップされ、物理アドレスへ正しく変換される
    let probe_phys = PhysAddr::new(64 * 1024 * 1024 + 0x1234).unwrap_or_default();
    match kernel_space::translate(hhdm::phys_to_virt(probe_phys)) {
        Some(t) => s.check(
            "paging.hhdm_large_pages",
            t.phys == probe_phys && t.size != PageSize::Size4K && !t.flags.executable,
            format_args!(
                "hhdm({probe_phys}) -> {} via a {} page ({})",
                t.phys,
                t.size.name(),
                t.flags
            ),
        ),
        None => s.check(
            "paging.hhdm_large_pages",
            false,
            format_args!("physical {probe_phys} is not in the HHDM"),
        ),
    }

    // カーネルセクションの権限 (W^X)
    static RODATA_SAMPLE: [u8; 4] = *b"yume";
    static mut DATA_SAMPLE: u32 = 0;
    let sections = [
        ("text", run as *const () as u64, false, true),
        ("rodata", RODATA_SAMPLE.as_ptr() as u64, false, false),
        ("data", (&raw const DATA_SAMPLE) as u64, true, false),
    ];
    let mut ok = true;
    let mut detail = String::new();
    for (name, addr, w, x) in sections {
        match kernel_space::translate(VirtAddr::new_canonical(addr)) {
            Some(t) => {
                ok &= t.flags.writable == w && t.flags.executable == x;
                detail.push_str(&format!("{name}={} ", t.flags));
            }
            None => {
                ok = false;
                detail.push_str(&format!("{name}=unmapped "));
            }
        }
    }
    s.check(
        "paging.kernel_sections_wx",
        ok,
        format_args!("{}", detail.trim_end()),
    );

    // カーネル空間全体の W^X 監査
    match kernel_space::audit_wx() {
        Some(a) => s.check(
            "paging.wx_audit",
            a.violations == 0 && a.user == 0 && a.executable > 0,
            format_args!(
                "{} leaves, {} executable, {} writable+executable, {} user",
                a.leaves, a.executable, a.violations, a.user
            ),
        ),
        None => s.check("paging.wx_audit", false, format_args!("no kernel space")),
    }

    // 権限変更: RW で書いてから RX へ (ret 命令を実行できる・書けなくなる)
    match KernelMapping::new_anonymous(1, PageFlags::KERNEL_RW) {
        Ok(mut m) => {
            if let Some(buf) = m.as_mut_slice() {
                buf[0] = 0xC3; // ret
            }
            let set = m.set_flags(PageFlags::KERNEL_RX);
            let addr = m.base().as_u64();
            // SAFETY: このページは `ret` 1 命令だけを含む。
            let exec = unsafe { probe::exec(addr) };
            // SAFETY: 書き込みが成功しても、このテスト専用のページが変わるだけ。
            let write = unsafe { probe::write_u8(addr, 0x90) };
            let wx_rejected = m.set_flags(PageFlags {
                writable: true,
                ..PageFlags::KERNEL_RX
            });
            s.check(
                "paging.protect_rw_to_rx",
                set.is_ok()
                    && exec.is_ok()
                    && matches!(write, Err(f) if f.vector == 14 && f.error_code & 0b11 == 0b11)
                    && wx_rejected.is_err(),
                format_args!(
                    "set_flags={set:?} exec={exec:?} write_after_rx={write:?} request_wx={wx_rejected:?}"
                ),
            );
        }
        Err(e) => s.check("paging.protect_rw_to_rx", false, format_args!("{e}")),
    }

    // プロセス用アドレス空間: 上位半分の共有・切り替え・破棄
    user_space_test(s);

    let frames_after = frame::stats().unwrap_or_default().free_frames;
    s.check(
        "paging.no_frame_leak",
        frames_after == frames_before,
        format_args!("free frames before={frames_before} after={frames_after}"),
    );
}

fn user_space_test(s: &mut Suite) {
    let mut space = match kernel_space::new_user_space() {
        Ok(sp) => sp,
        Err(e) => {
            s.check("paging.user_address_space", false, format_args!("{e}"));
            return;
        }
    };
    let kernel_pml4 = kernel_space::kernel_pml4();
    // 上位半分の PML4 エントリがカーネルと同じか
    let shared = kernel_pml4.is_some()
        && (256..512).all(|i| space.kernel_pml4_entry(i) == kernel_space::kernel_pml4_entry(i));
    // 下位半分にユーザーページを 1 つ置く
    let user_va = VirtAddr::new_canonical(0x0000_0040_0000_0000);
    let frame = match frame::alloc_zeroed() {
        Ok(f) => f,
        Err(e) => {
            s.check("paging.user_address_space", false, format_args!("{e}"));
            return;
        }
    };
    // SAFETY: frame は今確保した未使用フレーム。user_va はこの新しい空間の下位半分で未使用。
    let mapped = unsafe { space.map(user_va, frame.start(), PageSize::Size4K, PageFlags::USER_RW) };
    let translated = space.translate(user_va).map(|t| (t.phys, t.flags.user));
    // ユーザー空間に切り替えてもカーネル (上位半分) はそのまま動く
    static MARKER: u64 = 0x5945_4b41_5741_4f53;
    // SAFETY: 新しい空間の上位半分はカーネル空間と同じ PDPT を共有しているので、
    // 実行中のコード・スタック・静的データはすべてマップされている。
    unsafe { space.activate() };
    let active = space.is_active();
    let marker = probe::read_u8(&raw const MARKER as u64);
    // SAFETY: カーネル空間へ戻る (同じく上位半分は共通)。
    unsafe { kernel_space::activate_kernel_space() };
    let back = !space.is_active();
    // SAFETY: user_va は上で map したページで、今は誰も参照していない。
    let unmapped = unsafe { space.unmap(user_va) }.map(|(p, _)| p);
    let freed_frame = release(frame);
    // SAFETY: space は CR3 にロードされていない (直前にカーネル空間へ戻した)。
    let destroyed = unsafe { space.destroy() };
    s.check(
        "paging.user_address_space",
        shared
            && mapped.is_ok()
            && translated == Some((frame.start(), true))
            && active
            && marker == Ok(MARKER.to_le_bytes()[0])
            && back
            && unmapped == Ok(frame.start())
            && freed_frame.is_ok()
            && destroyed.is_ok(),
        format_args!(
            "kernel half shared={shared}, map={mapped:?}, translate={translated:?}, switched={active}, kernel data readable={}, switched back={back}, unmap={unmapped:?}, destroy freed {:?} table frame(s)",
            marker.is_ok(),
            destroyed
        ),
    );
}

fn heap_tests(s: &mut Suite) {
    let start = heap::stats();

    // Box / Vec / String
    let b = Box::new(0x1234_5678u64);
    let mut v: Vec<u64> = Vec::new();
    for i in 0..10_000 {
        v.push(i);
    }
    let sum: u64 = v.iter().sum();
    let text = format!("yumekawa {} {}", *b, v.len());
    s.check(
        "heap.box_vec_string",
        *b == 0x1234_5678 && sum == 10_000 * 9_999 / 2 && text == "yumekawa 305419896 10000",
        format_args!("box={:#x} vec_sum={sum} string='{text}'", *b),
    );
    drop((b, v, text));

    // 大小混在の確保・解放 (内容が壊れないこと)
    let mut rng = 0x2545_f491_4f6c_dd1du64;
    let mut next = || {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        rng
    };
    let mut live: Vec<(Vec<u8>, u8)> = Vec::new();
    let mut corrupt = 0;
    let mut ops = 0;
    for i in 0..3000u32 {
        let r = next();
        if live.len() > 64 || (r & 3 == 0 && !live.is_empty()) {
            let idx = (r as usize >> 8) % live.len();
            let (buf, tag) = live.swap_remove(idx);
            if buf.iter().any(|&x| x != tag) {
                corrupt += 1;
            }
        } else {
            let size = match r % 10 {
                0 => 8 + (r >> 20) as usize % 64 * 1024, // 最大 64KiB
                1..=3 => 256 + (r >> 20) as usize % 4096,
                _ => 1 + (r >> 20) as usize % 128,
            };
            let tag = i as u8;
            live.push((alloc::vec![tag; size], tag));
        }
        ops += 1;
    }
    for (buf, tag) in live.drain(..) {
        if buf.iter().any(|&x| x != tag) {
            corrupt += 1;
        }
    }
    drop(live);
    let mid = heap::stats();
    s.check(
        "heap.mixed_sizes",
        corrupt == 0 && mid.used <= start.used + 64,
        format_args!(
            "{ops} operations, corrupted={corrupt}, used before/after={}/{} bytes, peak={}",
            start.used, mid.used, mid.peak_used
        ),
    );

    // 断片化後の再利用: 1 つおきに解放した穴に同じ大きさの確保が入る (ヒープが伸びない)
    let mut blocks: Vec<Option<Box<[u8; 256]>>> =
        (0..256).map(|_| Some(Box::new([7u8; 256]))).collect();
    let mut holes: Vec<usize> = Vec::with_capacity(128);
    for (i, b) in blocks.iter_mut().enumerate() {
        if i % 2 == 0
            && let Some(bx) = b.take()
        {
            holes.push(&*bx as *const [u8; 256] as usize);
        }
    }
    let size_before = heap::stats().size;
    let refill: Vec<Box<[u8; 256]>> = (0..128).map(|_| Box::new([9u8; 256])).collect();
    let reused = refill
        .iter()
        .filter(|b| holes.contains(&(&***b as *const [u8; 256] as usize)))
        .count();
    let size_after = heap::stats().size;
    s.check(
        "heap.fragmentation_reuse",
        reused >= 120 && size_after == size_before,
        format_args!(
            "{reused}/128 new blocks reused freed holes, heap size {size_before} -> {size_after}"
        ),
    );
    drop(refill);
    drop(blocks);
    drop(holes);

    // ヒープの拡張: 初期サイズ (1MiB) を超える確保
    let grow_before = heap::stats();
    let mut big: Vec<u8> = Vec::new();
    let reserve = big.try_reserve_exact(4 * 1024 * 1024);
    if reserve.is_ok() {
        big.resize(4 * 1024 * 1024, 0xAB);
    }
    let grow_after = heap::stats();
    let content_ok = big.iter().step_by(4093).all(|&x| x == 0xAB);
    s.check(
        "heap.grow",
        reserve.is_ok()
            && content_ok
            && grow_after.grow_count > grow_before.grow_count
            && grow_after.mapped > grow_before.mapped,
        format_args!(
            "4 MiB vec: reserve={reserve:?}, heap mapped {} KiB -> {} KiB, grow count {} -> {}",
            grow_before.mapped / 1024,
            grow_after.mapped / 1024,
            grow_before.grow_count,
            grow_after.grow_count
        ),
    );
    drop(big);

    // 整列した確保
    let layout = Layout::from_size_align(8192, 4096).unwrap_or_else(|_| unreachable!());
    // SAFETY: layout の大きさは 0 でない。返ったポインタは同じ layout で dealloc する。
    let p = unsafe { alloc::alloc::alloc(layout) };
    let aligned = !p.is_null() && (p as usize).is_multiple_of(4096);
    if !p.is_null() {
        // SAFETY: 上で同じ layout で確保したポインタ。
        unsafe { alloc::alloc::dealloc(p, layout) };
    }
    s.check(
        "heap.aligned_alloc",
        aligned,
        format_args!("8 KiB aligned to 4 KiB -> {p:p}"),
    );

    // 確保失敗の扱い: fallible API はパニックせずエラーを返す
    let failed_before = heap::stats().failed_allocs;
    let mut huge: Vec<u8> = Vec::new();
    let r = huge.try_reserve_exact((layout::KERNEL_HEAP_MAX + 1) as usize);
    let failed_after = heap::stats().failed_allocs;
    s.check(
        "heap.alloc_failure_reported",
        r.is_err() && failed_after == failed_before + 1,
        format_args!(
            "try_reserve({} GiB) -> {}, failed_allocs {failed_before} -> {failed_after}",
            (layout::KERNEL_HEAP_MAX + 1) >> 30,
            if r.is_err() { "Err" } else { "Ok" }
        ),
    );

    let end = heap::stats();
    s.check(
        "heap.stats",
        end.used <= start.used + 64 && end.size >= end.used,
        format_args!(
            "mapped={} KiB size={} KiB used={} B free={} KiB grows={} failed={} peak={} KiB",
            end.mapped / 1024,
            end.size / 1024,
            end.used,
            end.free / 1024,
            end.grow_count,
            end.failed_allocs,
            end.peak_used / 1024
        ),
    );
}

//! カーネルヒープ (`#[global_allocator]`)。
//!
//! - 仮想領域 `KERNEL_HEAP_START`〜 (最大 `KERNEL_HEAP_MAX`) に、必要な分だけ物理フレームを
//!   4KiB ページでマップして使う。初期サイズは [`INITIAL_SIZE`]。
//! - 空き管理は `linked_list_allocator` (first-fit・解放時に隣接ブロックを結合)。
//! - 確保に失敗したら、要求を満たす大きさ (最低 [`GROW_CHUNK`]) だけヒープの末尾に
//!   ページを追加マップして拡張し、再試行する。
//! - ヒープは [`IrqSpinLock`] で保護する (確保中は割り込み禁止)。
//!
//! ## 確保失敗時の挙動 (alloc error handler)
//!
//! 物理メモリ・仮想領域を使い切って拡張もできない場合、`GlobalAlloc::alloc` は null を返す。
//! `Box::new` / `Vec::push` 等の「失敗しない」API は、Rust の既定の alloc error handler
//! (stable の `no_std` では `panic!("memory allocation of N bytes failed")`) により
//! **Kernel Panic** になる。失敗を扱いたい呼び出し側は `Vec::try_reserve` 等の fallible API を使う。
//! (`#[alloc_error_handler]` は stable で使えないため、独自のハンドラは定義していない。)
//!
//! ## 割り込みハンドラ内での確保
//!
//! **禁止** (方針)。ハンドラは固定長のバッファ ([`crate::sync::SpscRing`] 等) を使うこと。
//! ロック自体は IrqSpinLock なので単一 CPU ではデッドロックしないが、ヒープ拡張は
//! ページテーブル操作を伴い遅く、割り込み応答を悪化させるため。debug ビルドでは
//! 割り込みハンドラ内の確保を `debug_assert!` で検出する。

use core::alloc::{GlobalAlloc, Layout};
use core::ptr::{self, NonNull};

use linked_list_allocator::Heap;

use super::addr::{PAGE_SIZE, PageSize, VirtAddr, align_up};
use super::frame;
use super::kernel_space::with_kernel_space;
use super::layout::{KERNEL_HEAP_MAX, KERNEL_HEAP_START};
use super::paging::{MapError, PageFlags};
use crate::sync::IrqSpinLock;

/// 初期サイズ。
pub const INITIAL_SIZE: u64 = 1024 * 1024;
/// 拡張の最小単位。
pub const GROW_CHUNK: u64 = 1024 * 1024;

struct KernelHeap {
    heap: Heap,
    /// マップ済みのバイト数 (KERNEL_HEAP_START から)
    mapped: u64,
    grow_count: u64,
    failed_allocs: u64,
    peak_used: usize,
}

impl KernelHeap {
    /// ヒープの末尾に `bytes` (ページ単位) を追加マップする。成功した分のバイト数を返す。
    fn map_more(&mut self, bytes: u64) -> Result<u64, MapError> {
        let bytes = align_up(bytes, PAGE_SIZE);
        if self.mapped + bytes > KERNEL_HEAP_MAX {
            return Err(MapError::OutOfVirtualSpace);
        }
        let mut added = 0;
        while added < bytes {
            let virt = VirtAddr::new_canonical(KERNEL_HEAP_START + self.mapped + added);
            let frame = match frame::alloc() {
                Ok(f) => f,
                Err(e) if added == 0 => return Err(e.into()),
                Err(_) => break,
            };
            let r = with_kernel_space(|space| {
                // SAFETY: frame は確保したばかり。virt はヒープ専用の仮想領域で、
                // マップ済みの末尾の次のページなので未使用。
                unsafe { space.map(virt, frame.start(), PageSize::Size4K, PageFlags::KERNEL_RW) }
            })
            .unwrap_or(Err(MapError::NotMapped(virt)));
            if let Err(e) = r {
                // SAFETY: マップに失敗したので frame はどこからも参照されていない。
                let _ = unsafe { frame::free(frame) };
                if added == 0 {
                    return Err(e);
                }
                break;
            }
            added += PAGE_SIZE;
        }
        self.mapped += added;
        Ok(added)
    }
}

static HEAP: IrqSpinLock<KernelHeap> = IrqSpinLock::new(KernelHeap {
    heap: Heap::empty(),
    mapped: 0,
    grow_count: 0,
    failed_allocs: 0,
    peak_used: 0,
});

/// ヒープを初期化する (カーネル空間の構築後に 1 度)。
pub fn init() -> Result<(), MapError> {
    let mut h = HEAP.lock();
    if h.mapped != 0 {
        return Ok(());
    }
    let added = h.map_more(INITIAL_SIZE)?;
    // SAFETY: [KERNEL_HEAP_START, +added) は今マップした、ヒープ専用の 'static な領域。
    unsafe { h.heap.init(KERNEL_HEAP_START as *mut u8, added as usize) };
    Ok(())
}

/// ヒープの統計。
#[derive(Debug, Clone, Copy, Default)]
pub struct HeapStats {
    /// マップ済みの大きさ
    pub mapped: u64,
    /// アロケータが管理する大きさ
    pub size: usize,
    /// 使用中
    pub used: usize,
    /// 空き
    pub free: usize,
    /// 拡張した回数
    pub grow_count: u64,
    /// 失敗した確保の回数 (拡張もできなかった)
    pub failed_allocs: u64,
    /// 使用量の最大値
    pub peak_used: usize,
}

fn stats_of(h: &KernelHeap) -> HeapStats {
    HeapStats {
        mapped: h.mapped,
        size: h.heap.size(),
        used: h.heap.used(),
        free: h.heap.free(),
        grow_count: h.grow_count,
        failed_allocs: h.failed_allocs,
        peak_used: h.peak_used,
    }
}

/// 統計。
pub fn stats() -> HeapStats {
    stats_of(&HEAP.lock())
}

/// パニック画面用: ロックが取れたときだけ統計を返す。
pub fn try_stats() -> Option<HeapStats> {
    HEAP.try_lock().map(|h| stats_of(&h))
}

/// カーネルのグローバルアロケータ。
#[derive(Debug)]
pub struct KernelAllocator;

// SAFETY: alloc は Heap::allocate_first_fit が返す、layout を満たす未使用ブロックを返し、
// dealloc は同じ layout で確保されたブロックだけを返却する (GlobalAlloc の契約)。
// Heap はロックで排他され、割り込みハンドラからの再入も IrqSpinLock により起きない。
unsafe impl GlobalAlloc for KernelAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        debug_assert!(
            !crate::interrupts::in_interrupt(),
            "heap allocation inside an interrupt handler ({} bytes)",
            layout.size()
        );
        let mut h = HEAP.lock();
        if h.mapped == 0 {
            return ptr::null_mut();
        }
        let ptr = match h.heap.allocate_first_fit(layout) {
            Ok(p) => p.as_ptr(),
            Err(()) => {
                // 拡張して再試行する (アラインの余白と管理領域の分を足す)
                let need = layout.size() as u64 + layout.align() as u64 + 64;
                let grow = align_up(need.max(GROW_CHUNK), PAGE_SIZE);
                match h.map_more(grow) {
                    Ok(added) => {
                        // SAFETY: [top, top+added) は今ヒープの直後にマップした、
                        // ヒープ専用の 'static な領域。
                        unsafe { h.heap.extend(added as usize) };
                        h.grow_count += 1;
                        match h.heap.allocate_first_fit(layout) {
                            Ok(p) => p.as_ptr(),
                            Err(()) => ptr::null_mut(),
                        }
                    }
                    Err(_) => ptr::null_mut(),
                }
            }
        };
        if ptr.is_null() {
            h.failed_allocs += 1;
        } else {
            h.peak_used = h.peak_used.max(h.heap.used());
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        let Some(ptr) = NonNull::new(ptr) else {
            return;
        };
        // SAFETY: GlobalAlloc の契約により、ptr はこのアロケータが同じ layout で返したもの。
        unsafe { HEAP.lock().heap.deallocate(ptr, layout) };
    }
}

#[global_allocator]
static ALLOCATOR: KernelAllocator = KernelAllocator;

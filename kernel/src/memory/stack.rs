//! ガードページ付きのカーネルスタック。
//!
//! スタック領域 (`KERNEL_STACKS_START`〜) を 1MiB のスロットに分け、各スロットの
//! **上端側** にスタックを置き、下側の残り (最低 1 ページ) は未マップのガードにする。
//!
//! ```text
//! slot_base                              slot_base + 1MiB
//! |<------- guard (未マップ) ------->|<---- stack (RW, NX) ---->|
//!                                    bottom                     top (初期 RSP)
//! ```
//!
//! スタックが溢れるとガードに触れて #PF になる。そのとき #PF の例外フレームも
//! 同じ (溢れた) スタックに積めないため CPU は #DF を発生させ、#DF は IST の専用スタックで
//! 確実に処理される。#DF ハンドラは CR2 / RSP が [`find_guard`] に該当するかを調べ、
//! 「kernel stack overflow」として Kernel Panic を出す。
//!
//! x86_64-unknown-none は stack probe (inline) が有効なので、1 ページを超える
//! スタックフレームでもガードを飛び越えずに必ず触れる。
//!
//! スタックは現状解放しない (起動時の数本だけ)。Phase 5 でタスクごとのスタックを
//! 作る際に解放を追加する。

use core::fmt;
use core::sync::atomic::{AtomicU64, Ordering};

use super::addr::{PAGE_SIZE, PageSize, VirtAddr, align_up};
use super::frame;
use super::kernel_space::with_kernel_space;
use super::layout::{KERNEL_STACK_SLOT_SIZE, KERNEL_STACK_SLOTS, KERNEL_STACKS_START};
use super::paging::{MapError, PageFlags};
use crate::sync::IrqSpinLock;

static NEXT_SLOT: AtomicU64 = AtomicU64::new(0);

const MAX_RECORDS: usize = 32;

/// 確保したスタックの情報。
#[derive(Debug, Clone, Copy)]
pub struct StackInfo {
    /// 名前
    pub name: &'static str,
    /// ガードの先頭 (= スロットの先頭)
    pub guard_start: u64,
    /// スタックの最下端 (= ガードの終端)
    pub bottom: u64,
    /// スタックの最上端 (初期 RSP)
    pub top: u64,
}

static RECORDS: IrqSpinLock<[Option<StackInfo>; MAX_RECORDS]> =
    IrqSpinLock::new([None; MAX_RECORDS]);

/// 確保したカーネルスタック。
pub struct KernelStack {
    info: StackInfo,
}

impl fmt::Debug for KernelStack {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&self.info, f)
    }
}

impl KernelStack {
    /// スタックトップ (16 バイト境界)。
    pub fn top(&self) -> u64 {
        self.info.top
    }

    /// 情報。
    pub fn info(&self) -> StackInfo {
        self.info
    }
}

/// `size` バイト (ページ単位に切り上げ) のスタックを、ガード付きで確保する。
pub fn allocate(name: &'static str, size: u64) -> Result<KernelStack, MapError> {
    let size = align_up(size, PAGE_SIZE);
    if size == 0 || size > KERNEL_STACK_SLOT_SIZE - PAGE_SIZE {
        return Err(MapError::Misaligned);
    }
    let slot = NEXT_SLOT.fetch_add(1, Ordering::Relaxed);
    if slot >= KERNEL_STACK_SLOTS {
        return Err(MapError::OutOfVirtualSpace);
    }
    let slot_base = KERNEL_STACKS_START + slot * KERNEL_STACK_SLOT_SIZE;
    let top = slot_base + KERNEL_STACK_SLOT_SIZE;
    let bottom = top - size;
    let mut v = bottom;
    while v < top {
        let frame = frame::alloc_zeroed()?;
        let virt = VirtAddr::new_canonical(v);
        with_kernel_space(|space| {
            // SAFETY: frame は確保したばかりで未使用。virt はこのスタック専用のスロット内。
            unsafe { space.map(virt, frame.start(), PageSize::Size4K, PageFlags::KERNEL_RW) }
        })
        .unwrap_or(Err(MapError::NotMapped(virt)))?;
        v += PAGE_SIZE;
    }
    let info = StackInfo {
        name,
        guard_start: slot_base,
        bottom,
        top,
    };
    let mut records = RECORDS.lock();
    if let Some(r) = records.iter_mut().find(|r| r.is_none()) {
        *r = Some(info);
    } else {
        warn!("stack registry full; overflow diagnostics for '{name}' will be generic");
    }
    Ok(KernelStack { info })
}

/// `addr` がいずれかのスタックのガード領域にあればその情報を返す。
///
/// 例外ハンドラから呼ぶので `try_lock` を使い、ロックが取れなければアドレスの範囲だけで
/// 判定した名前不明の情報を返す。
pub fn find_guard(addr: u64) -> Option<StackInfo> {
    let end = KERNEL_STACKS_START + KERNEL_STACK_SLOTS * KERNEL_STACK_SLOT_SIZE;
    if !(KERNEL_STACKS_START..end).contains(&addr) {
        return None;
    }
    if let Some(records) = RECORDS.try_lock() {
        return records
            .iter()
            .flatten()
            .find(|r| (r.guard_start..r.bottom).contains(&addr))
            .copied();
    }
    let slot_base = addr - (addr - KERNEL_STACKS_START) % KERNEL_STACK_SLOT_SIZE;
    Some(StackInfo {
        name: "<unknown stack>",
        guard_start: slot_base,
        bottom: slot_base,
        top: slot_base + KERNEL_STACK_SLOT_SIZE,
    })
}

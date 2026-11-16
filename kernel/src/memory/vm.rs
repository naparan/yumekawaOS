//! カーネル仮想メモリ領域 (`KERNEL_VM_START`〜) の匿名マッピング。
//!
//! [`KernelMapping`] は新しい物理フレームを確保して連続した仮想領域にマップし、
//! drop 時にマップを外してフレームを解放する (所有権でマップの寿命を管理する safe API)。
//!
//! 仮想アドレスは単純な前方確保 (bump) で、解放した仮想範囲は再利用しない
//! (領域は 16TiB あり、Phase 2 の用途では使い切らない)。各マッピングの間には
//! 1 ページの未マップのすき間を置き、境界を越えたアクセスを #PF で検出できるようにする。

use core::fmt;
use core::sync::atomic::{AtomicU64, Ordering};

use super::addr::{PAGE_SIZE, PageSize, PhysAddr, VirtAddr};
use super::frame;
use super::kernel_space::with_kernel_space;
use super::layout::{KERNEL_VM_END, KERNEL_VM_START};
use super::paging::{MapError, PageFlags};

static NEXT: AtomicU64 = AtomicU64::new(KERNEL_VM_START);

/// `pages` ページ分 (+ すき間 1 ページ) の仮想範囲を予約する。
pub(super) fn reserve(pages: u64) -> Result<VirtAddr, MapError> {
    let bytes = pages
        .checked_add(1)
        .and_then(|p| p.checked_mul(PAGE_SIZE))
        .ok_or(MapError::OutOfVirtualSpace)?;
    let base = NEXT.fetch_add(bytes, Ordering::Relaxed);
    if base
        .checked_add(bytes)
        .is_none_or(|end| end > KERNEL_VM_END)
    {
        return Err(MapError::OutOfVirtualSpace);
    }
    VirtAddr::new(base).ok_or(MapError::OutOfVirtualSpace)
}

/// フレームを確保してカーネル仮想領域にマップした、所有権付きの領域。
pub struct KernelMapping {
    base: VirtAddr,
    pages: u64,
    flags: PageFlags,
}

impl fmt::Debug for KernelMapping {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("KernelMapping")
            .field("base", &self.base)
            .field("pages", &self.pages)
            .field("flags", &self.flags)
            .finish()
    }
}

impl KernelMapping {
    /// `pages` ページの匿名領域を作る (ゼロクリア済み)。
    pub fn new_anonymous(pages: u64, flags: PageFlags) -> Result<Self, MapError> {
        if pages == 0 || flags.user {
            return Err(MapError::Misaligned);
        }
        if flags.writable && flags.executable {
            return Err(MapError::WritableAndExecutable);
        }
        let base = reserve(pages)?;
        let mut mapping = Self {
            base,
            pages: 0,
            flags,
        };
        for i in 0..pages {
            let frame = frame::alloc_zeroed()?;
            let virt = VirtAddr::new_canonical(base.as_u64() + i * PAGE_SIZE);
            let result = with_kernel_space(|space| {
                // SAFETY: frame は今確保したばかりで他に使われていない。virt は reserve で
                // このマッピング専用に予約した仮想アドレスで、他のコードは使っていない。
                unsafe { space.map(virt, frame.start(), PageSize::Size4K, flags) }
            })
            .unwrap_or(Err(MapError::NotMapped(virt)));
            if let Err(e) = result {
                // SAFETY: マップに失敗したので frame はどこからも参照されていない。
                if let Err(fe) = unsafe { frame::free(frame) } {
                    warn!("failed to free frame after map error: {fe}");
                }
                return Err(e); // ここまでの分は mapping の drop で解放される
            }
            mapping.pages += 1;
        }
        Ok(mapping)
    }

    /// 先頭の仮想アドレス。
    pub fn base(&self) -> VirtAddr {
        self.base
    }

    /// バイト数。
    pub fn size_bytes(&self) -> usize {
        (self.pages * PAGE_SIZE) as usize
    }

    /// 内容を書き換える (書き込み可能なときだけ)。
    pub fn as_mut_slice(&mut self) -> Option<&mut [u8]> {
        if !self.flags.writable {
            return None;
        }
        // SAFETY: 上と同じ領域で、書き込み可能にマップされている。`&mut self` により一意。
        Some(unsafe {
            core::slice::from_raw_parts_mut(self.base.as_mut_ptr::<u8>(), self.size_bytes())
        })
    }

    /// `index` 番目のページの物理アドレス。
    pub fn phys_of_page(&self, index: u64) -> Option<PhysAddr> {
        if index >= self.pages {
            return None;
        }
        super::kernel_space::translate(VirtAddr::new_canonical(
            self.base.as_u64() + index * PAGE_SIZE,
        ))
        .map(|t| t.phys)
    }

    /// 権限を変更する (例: 書き込んだ後に読み取り専用 + 実行可能にする)。
    pub fn set_flags(&mut self, flags: PageFlags) -> Result<(), MapError> {
        if flags.writable && flags.executable {
            return Err(MapError::WritableAndExecutable);
        }
        for i in 0..self.pages {
            let virt = VirtAddr::new_canonical(self.base.as_u64() + i * PAGE_SIZE);
            with_kernel_space(|space| {
                // SAFETY: このマッピングが所有するページで、`&mut self` によりこの領域への
                // 参照 (スライス) は他に存在しないため、権限を変えても誰も影響を受けない。
                unsafe { space.protect(virt, flags) }
            })
            .unwrap_or(Err(MapError::NotMapped(virt)))?;
        }
        self.flags = flags;
        Ok(())
    }

    /// マップを外してフレームを解放し、外した先頭アドレスを返す。
    ///
    /// 返したアドレス範囲は再利用されないので、以降のアクセスは必ず #PF になる (テスト用)。
    pub fn unmap(mut self) -> VirtAddr {
        let base = self.base;
        self.release();
        core::mem::forget(self);
        base
    }

    fn release(&mut self) {
        for i in 0..self.pages {
            let virt = VirtAddr::new_canonical(self.base.as_u64() + i * PAGE_SIZE);
            let result = with_kernel_space(|space| {
                // SAFETY: このマッピングが所有するページで、`&mut self` (または drop) により
                // 参照は残っていない。
                unsafe { space.unmap(virt) }
            });
            match result {
                Some(Ok((phys, _))) => {
                    if let Some(frame) = super::addr::PhysFrame::from_start(phys) {
                        // SAFETY: このマッピング用に確保したフレームで、今マップを外した。
                        if let Err(e) = unsafe { frame::free(frame) } {
                            error!("KernelMapping: failed to free {phys}: {e}");
                        }
                    }
                }
                Some(Err(e)) => error!("KernelMapping: failed to unmap {virt}: {e}"),
                None => error!("KernelMapping: kernel address space is not initialized"),
            }
        }
        self.pages = 0;
    }
}

impl Drop for KernelMapping {
    fn drop(&mut self) {
        self.release();
    }
}

/// カーネル空間の**下位半分**に user ページを 1 枚置く (SMEP/SMAP のセルフテスト専用)。
///
/// 通常、カーネル空間の下位半分は空で、ユーザーページはプロセスの空間にだけ置く。
/// このテストでは「カーネル (Ring 0) から user ページへの実行・アクセスが CPU に拒否される」
/// ことを確かめるため、一時的にカーネル空間へ置き、drop で外して中間テーブルごと解放する。
pub struct LowerHalfTestPage {
    virt: VirtAddr,
    frame: super::addr::PhysFrame,
}

impl fmt::Debug for LowerHalfTestPage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LowerHalfTestPage")
            .field("virt", &self.virt)
            .field("frame", &self.frame)
            .finish()
    }
}

impl LowerHalfTestPage {
    /// `virt` (下位半分, 4KiB 境界) に `content` を書いたページを `flags` (user) でマップする。
    pub fn new(virt: VirtAddr, content: &[u8], flags: PageFlags) -> Result<Self, MapError> {
        if virt.is_kernel_half() || !flags.user || content.len() > PAGE_SIZE as usize {
            return Err(MapError::WrongHalf(virt));
        }
        let frame = frame::alloc_zeroed()?;
        let dst = super::hhdm::phys_to_virt(frame.start()).as_mut_ptr::<u8>();
        // SAFETY: frame は今確保したばかりで HHDM で書き込み可能。content は 4KiB 以下。
        unsafe { core::ptr::copy_nonoverlapping(content.as_ptr(), dst, content.len()) };
        let r = with_kernel_space(|space| {
            // SAFETY: frame は未使用。virt はカーネル空間の下位半分 (通常は空) で、
            // このテスト以外は使わない。
            unsafe { space.map(virt, frame.start(), PageSize::Size4K, flags) }
        })
        .unwrap_or(Err(MapError::NotMapped(virt)));
        if let Err(e) = r {
            // SAFETY: マップに失敗したので未使用。
            let _ = unsafe { frame::free(frame) };
            return Err(e);
        }
        Ok(Self { virt, frame })
    }

    /// 仮想アドレス。
    pub fn addr(&self) -> VirtAddr {
        self.virt
    }
}

impl Drop for LowerHalfTestPage {
    fn drop(&mut self) {
        let r = with_kernel_space(|space| {
            // SAFETY: このテストページだけのマップで、参照は残っていない。
            unsafe { space.unmap(self.virt) }
        });
        if !matches!(r, Some(Ok(_))) {
            error!("LowerHalfTestPage: unmap failed: {r:?}");
            return;
        }
        // SAFETY: マップを外したので frame はどこからも参照されていない。
        if let Err(e) = unsafe { frame::free(self.frame) } {
            error!("LowerHalfTestPage: free failed: {e}");
        }
    }
}

//! MMIO 領域 (`MMIO_START`〜) へのデバイスレジスタのマップ。
//!
//! デバイスのレジスタはキャッシュ不可 (UC)・NX・カーネル専用でマップし、
//! [`MmioRegion`] の範囲・整列チェック付きの volatile アクセスだけを公開する。
//! マップは解除しない (デバイスはカーネルの寿命の間存在する)。

use core::fmt;
use core::sync::atomic::{AtomicU64, Ordering};

use super::addr::{PAGE_SIZE, PageSize, PhysAddr, VirtAddr, align_down, align_up};
use super::kernel_space::with_kernel_space;
use super::layout::{MMIO_END, MMIO_START};
use super::paging::{MapError, PageFlags};

static NEXT: AtomicU64 = AtomicU64::new(MMIO_START);

/// マップ済みの MMIO レジスタ範囲。
pub struct MmioRegion {
    virt: VirtAddr,
    phys: PhysAddr,
    len: u64,
}

impl fmt::Debug for MmioRegion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MmioRegion")
            .field("phys", &self.phys)
            .field("virt", &self.virt)
            .field("len", &self.len)
            .finish()
    }
}

/// 物理 `[phys, phys+len)` のデバイスレジスタを UC でマップする。
///
/// # Safety
///
/// その物理範囲がデバイスのレジスタ (RAM ではない) で、読み書きがメモリ安全性を
/// 損なわないこと (DMA を起動するレジスタ等は上位のドライバが責任を持つ)。
pub unsafe fn map(phys: PhysAddr, len: u64) -> Result<MmioRegion, MapError> {
    if len == 0 {
        return Err(MapError::Misaligned);
    }
    let start = align_down(phys.as_u64(), PAGE_SIZE);
    let end = align_up(phys.as_u64() + len, PAGE_SIZE);
    let pages = (end - start) / PAGE_SIZE;
    // すき間 1 ページを空ける
    let base = NEXT.fetch_add((pages + 1) * PAGE_SIZE, Ordering::Relaxed);
    if base + (pages + 1) * PAGE_SIZE > MMIO_END {
        return Err(MapError::OutOfVirtualSpace);
    }
    for i in 0..pages {
        let virt = VirtAddr::new_canonical(base + i * PAGE_SIZE);
        let pa = PhysAddr::new(start + i * PAGE_SIZE).ok_or(MapError::Misaligned)?;
        with_kernel_space(|space| {
            // SAFETY: 呼び出し元の契約によりデバイスレジスタで、virt はこの関数が
            // 予約した MMIO 領域の未使用アドレス。
            unsafe { space.map(virt, pa, PageSize::Size4K, PageFlags::MMIO) }
        })
        .unwrap_or(Err(MapError::NotMapped(virt)))?;
    }
    Ok(MmioRegion {
        virt: VirtAddr::new_canonical(base + (phys.as_u64() - start)),
        phys,
        len,
    })
}

impl MmioRegion {
    fn check(&self, offset: u64, size: u64) -> *mut u8 {
        assert!(
            offset.is_multiple_of(size) && offset + size <= self.len,
            "MMIO access out of range: offset {offset:#x} size {size} len {:#x}",
            self.len
        );
        (self.virt.as_u64() + offset) as *mut u8
    }

    /// 32bit レジスタを読む。
    pub fn read_u32(&self, offset: u64) -> u32 {
        let ptr = self.check(offset, 4).cast::<u32>();
        // SAFETY: check により範囲内・4 バイト整列。map の契約によりデバイスレジスタで、
        // UC でマップされている。
        unsafe { ptr.read_volatile() }
    }

    /// 32bit レジスタへ書く。
    pub fn write_u32(&self, offset: u64, value: u32) {
        let ptr = self.check(offset, 4).cast::<u32>();
        // SAFETY: read_u32 と同じ。
        unsafe { ptr.write_volatile(value) };
    }

    /// 64bit レジスタを読む。
    #[allow(dead_code)] // HPET 等で使う想定
    pub fn read_u64(&self, offset: u64) -> u64 {
        let ptr = self.check(offset, 8).cast::<u64>();
        // SAFETY: check により範囲内・8 バイト整列。デバイスレジスタで UC マップ。
        unsafe { ptr.read_volatile() }
    }
}

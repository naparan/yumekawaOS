//! HHDM (Higher Half Direct Map): 物理アドレス `p` を仮想アドレス `p + offset` で参照する。
//!
//! オフセットは Limine が決めた値 ([`crate::boot::BootInfo::hhdm_offset`]) をそのまま使う。
//! カーネル自身のページテーブルへ切り替えた後も同じオフセットで HHDM を作り直すため、
//! Limine から受け取った HHDM 上のポインタ (framebuffer 等) は切り替え後も有効。
//!
//! 切り替え後の HHDM がマップするのは RAM (usable / bootloader-reclaimable /
//! kernel-and-modules)、ACPI テーブル (読み取り専用)、framebuffer だけ。MMIO は
//! [`super::mmio`] で別領域に UC でマップする。

use core::sync::atomic::{AtomicU64, Ordering};

use super::addr::{PhysAddr, VirtAddr};

static OFFSET: AtomicU64 = AtomicU64::new(0);

/// HHDM オフセットを設定する (起動時に 1 度)。
pub fn set_offset(offset: u64) {
    OFFSET.store(offset, Ordering::Relaxed);
}

/// HHDM オフセット。
pub fn offset() -> u64 {
    OFFSET.load(Ordering::Relaxed)
}

/// 物理アドレスに対応する HHDM 上の仮想アドレス。
///
/// アドレスの計算だけで、そのページが実際にマップされているかは保証しない。
pub fn phys_to_virt(phys: PhysAddr) -> VirtAddr {
    VirtAddr::new_canonical(phys.as_u64() + offset())
}

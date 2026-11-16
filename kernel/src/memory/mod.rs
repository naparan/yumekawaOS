//! メモリ管理 (§8): 物理フレーム・ページテーブル・カーネルヒープ・カーネルスタック。
//!
//! | モジュール | 内容 | 主な API |
//! | --- | --- | --- |
//! | [`addr`] | アドレス・フレームの型 | `PhysAddr`, `VirtAddr`, `PhysFrame`, `PageSize` |
//! | [`layout`] | 仮想アドレス空間のレイアウト定数 | |
//! | [`hhdm`] | 物理メモリの直接マップ | `phys_to_virt` |
//! | [`frame`] | 物理フレームアロケータ | `alloc`, `alloc_contiguous`, `free` (unsafe), `stats` |
//! | [`paging`] | ページテーブル・アドレス空間 | `AddressSpace`, `PageFlags` |
//! | [`kernel_space`] | カーネル空間の構築と safe API | `translate`, `audit_wx` |
//! | [`vm`] | 匿名マッピング (所有権付き) | `KernelMapping` |
//! | [`mmio`] | デバイスレジスタのマップ | `map` (unsafe), `MmioRegion` |
//! | [`stack`] | ガードページ付きスタック | `allocate`, `find_guard` |
//! | [`heap`] | `#[global_allocator]` | `alloc` クレート (Box/Vec/String) |

pub mod addr;
pub mod frame;
pub mod heap;
pub mod hhdm;
pub mod kernel_space;
pub mod layout;
pub mod mmio;
pub mod paging;
pub mod stack;
pub mod vm;

pub use self::addr::{PhysAddr, VirtAddr};

/// ファームウェアのデータ (ACPI テーブル等) を読み取り専用で参照する。
///
/// HHDM でマップされていない部分 (メモリマップ上 reserved の領域など) は
/// 読み取り専用・NX で追加マップしてから、HHDM 上のスライスとして返す。
///
/// # Safety
///
/// `[phys, phys+len)` は、カーネルの寿命の間内容が変わらず解放もされない
/// ファームウェアのデータ (ACPI テーブル等) であること。MMIO であってはならない。
pub unsafe fn map_firmware_readonly(
    phys: u64,
    len: usize,
) -> Result<&'static [u8], paging::MapError> {
    kernel_space::ensure_hhdm_readonly(phys, len as u64)?;
    let addr = PhysAddr::new(phys).ok_or(paging::MapError::OutOfVirtualSpace)?;
    let virt = hhdm::phys_to_virt(addr);
    // SAFETY: 直前に [phys, phys+len) が HHDM で (少なくとも読み取り可能に) マップされている
    // ことを確認した。呼び出し元の契約により、内容はカーネルの寿命の間変わらない。
    Ok(unsafe { core::slice::from_raw_parts(virt.as_ptr::<u8>(), len) })
}

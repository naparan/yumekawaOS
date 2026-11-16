//! カーネルの仮想アドレス空間レイアウト (4 段ページング, 48bit)。
//!
//! ```text
//! 0x0000_0000_0000_0000 ┐
//!          ...          │ 下位半分 (PML4 0〜255): プロセスごと (Phase 7 のユーザー空間)。
//! 0x0000_7fff_ffff_ffff ┘ カーネルの PML4 では空。
//! 0xffff_8000_0000_0000 ┐ HHDM (PML4 256〜): 物理アドレス p を p + hhdm_offset で参照。
//!          ...          │ オフセットは Limine が決めた値をそのまま使う (QEMU では左端)。
//! 0xffff_bfff_ffff_ffff ┘ RAM / ACPI / framebuffer だけをマップ (MMIO はマップしない)。
//! 0xffff_c000_0000_0000 ┐ カーネルヒープ (PML4 384, 最大 KERNEL_HEAP_MAX)
//! 0xffff_c800_0000_0000 ┤ カーネルスタック (PML4 400): 1MiB のスロットごとに、
//!                       │ 下側にガード (未マップ) を置いてスタックを配置
//! 0xffff_d000_0000_0000 ┤ カーネル仮想メモリ領域 (PML4 416): 匿名マッピング等
//! 0xffff_e000_0000_0000 ┤ MMIO 領域 (PML4 448): LAPIC / IOAPIC / HPET 等を UC でマップ
//!          ...          │ (未使用)
//! 0xffff_ffff_8000_0000 ┘ カーネルイメージ (PML4 511, linker.ld)
//! ```
//!
//! 上位半分の PML4 エントリ (256〜511) は起動時にすべて PDPT を割り当てて埋めておき、
//! 以降変更しない。これによりプロセスごとの PML4 は上位半分の 256 エントリを
//! コピーするだけでカーネル空間を共有でき、カーネル側のマップ変更も全プロセスに反映される。

/// 上位半分の先頭。
pub const KERNEL_HALF_START: u64 = 0xffff_8000_0000_0000;
/// HHDM に使える上限 (この手前まで)。
pub const HHDM_LIMIT: u64 = 0xffff_c000_0000_0000;

/// カーネルヒープの先頭。
pub const KERNEL_HEAP_START: u64 = 0xffff_c000_0000_0000;
/// カーネルヒープの最大サイズ (仮想領域の予約。実際の物理メモリは必要に応じてマップ)。
pub const KERNEL_HEAP_MAX: u64 = 64 * 1024 * 1024 * 1024;

/// カーネルスタック領域の先頭。
pub const KERNEL_STACKS_START: u64 = 0xffff_c800_0000_0000;
/// スタック 1 つ分のスロット (ガード + スタック)。
pub const KERNEL_STACK_SLOT_SIZE: u64 = 1024 * 1024;
/// スタックスロットの数。
pub const KERNEL_STACK_SLOTS: u64 = 1024;

/// カーネル仮想メモリ領域の先頭。
pub const KERNEL_VM_START: u64 = 0xffff_d000_0000_0000;
/// カーネル仮想メモリ領域の終端 (この手前まで)。
pub const KERNEL_VM_END: u64 = 0xffff_e000_0000_0000;

/// MMIO 領域の先頭。
pub const MMIO_START: u64 = 0xffff_e000_0000_0000;
/// MMIO 領域の終端 (この手前まで)。
pub const MMIO_END: u64 = 0xffff_f000_0000_0000;

/// カーネルイメージの仮想ベース (linker.ld と一致)。
pub const KERNEL_IMAGE_BASE: u64 = 0xffff_ffff_8000_0000;

const _: () = assert!(KERNEL_HEAP_START + KERNEL_HEAP_MAX <= KERNEL_STACKS_START);
const _: () =
    assert!(KERNEL_STACKS_START + KERNEL_STACK_SLOT_SIZE * KERNEL_STACK_SLOTS <= KERNEL_VM_START);

//! 4 段ページテーブル (PML4 → PDPT → PD → PT) とアドレス空間 ([`AddressSpace`])。
//!
//! - ページテーブルのフレームは [`super::frame`] から確保し、HHDM 経由で読み書きする。
//! - 葉のエントリは 4KiB / 2MiB / 1GiB に対応する。大きいページの一部だけを
//!   unmap/remap する (ページの分割) は未対応で、[`MapError::HugePageConflict`] を返す。
//! - 権限は [`PageFlags`] で表し、W^X (書き込み可能かつ実行可能) を
//!   [`AddressSpace::map`] で拒否する。
//! - キャッシュ種別は PAT を `PAT_VALUE` に設定した上で、WB / WC / UC を選べる。
//!
//! # TLB
//!
//! 単一 CPU (BSP) 前提で、変更したページに `invlpg` を行う。上位半分 (カーネル空間) は
//! 全アドレス空間で共有され Global ページなので、変更時は常に `invlpg` する。SMP 導入時は
//! TLB shootdown (IPI) が必要 (docs/architecture.md「Phase 5 以降への注意」)。

use core::fmt;
use core::sync::atomic::{AtomicBool, Ordering};

use super::addr::{PAGE_SIZE, PageSize, PhysAddr, PhysFrame, VirtAddr};
use super::frame::{self, FrameError};
use super::hhdm;
use crate::arch::x86_64::registers;

const ENTRY_PRESENT: u64 = 1 << 0;
const ENTRY_WRITABLE: u64 = 1 << 1;
const ENTRY_USER: u64 = 1 << 2;
const ENTRY_PWT: u64 = 1 << 3;
const ENTRY_PCD: u64 = 1 << 4;
const ENTRY_HUGE: u64 = 1 << 7;
/// PT エントリ (4KiB) の PAT ビット
const ENTRY_PAT_4K: u64 = 1 << 7;
const ENTRY_GLOBAL: u64 = 1 << 8;
/// PD/PDPT の大きいページの PAT ビット
const ENTRY_PAT_LARGE: u64 = 1 << 12;
const ENTRY_NO_EXECUTE: u64 = 1 << 63;

const ADDR_MASK_4K: u64 = 0x000F_FFFF_FFFF_F000;
const ADDR_MASK_2M: u64 = 0x000F_FFFF_FFE0_0000;
const ADDR_MASK_1G: u64 = 0x000F_FFFF_C000_0000;

/// IA32_PAT に設定する値。
///
/// | index | PAT | PCD | PWT | 種別 |
/// | --- | --- | --- | --- | --- |
/// | 0 | 0 | 0 | 0 | WB |
/// | 1 | 0 | 0 | 1 | WT |
/// | 2 | 0 | 1 | 0 | UC- |
/// | 3 | 0 | 1 | 1 | UC |
/// | 4 | 1 | 0 | 0 | **WC** (電源投入時の既定は WB。framebuffer 用に変更) |
/// | 5 | 1 | 0 | 1 | WP |
/// | 6 | 1 | 1 | 0 | UC- |
/// | 7 | 1 | 1 | 1 | UC |
pub const PAT_VALUE: u64 = 0x0007_0501_0007_0406;

/// NX ビットを使うか (CPU が NX 非対応なら立てない: 予約ビット違反の #PF になるため)。
static NX_ENABLED: AtomicBool = AtomicBool::new(false);
/// PAT を設定済みか (未設定なら WC は UC で代用する)。
static PAT_ENABLED: AtomicBool = AtomicBool::new(false);
/// Global ビットを使うか (CR4.PGE)。
static GLOBAL_ENABLED: AtomicBool = AtomicBool::new(false);

/// ページング機能の有効/無効をまとめて設定する (起動時に 1 度)。
pub fn configure(nx: bool, pat: bool, global: bool) {
    NX_ENABLED.store(nx, Ordering::Relaxed);
    PAT_ENABLED.store(pat, Ordering::Relaxed);
    GLOBAL_ENABLED.store(global, Ordering::Relaxed);
}

/// キャッシュ種別。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheMode {
    /// Write-Back (通常の RAM)
    WriteBack,
    /// Write-Combining (framebuffer)
    WriteCombining,
    /// Uncacheable (MMIO)
    Uncached,
}

/// 葉のページの権限とキャッシュ種別。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageFlags {
    /// 書き込み可能
    pub writable: bool,
    /// 実行可能 (偽なら NX)
    pub executable: bool,
    /// ユーザー (Ring 3) からアクセス可能
    pub user: bool,
    /// Global (CR3 切り替えで TLB から消えない。カーネル空間用)
    pub global: bool,
    /// キャッシュ種別
    pub cache: CacheMode,
}

impl PageFlags {
    /// カーネル読み取り専用データ (R, NX)。
    pub const KERNEL_RO: Self = Self {
        writable: false,
        executable: false,
        user: false,
        global: true,
        cache: CacheMode::WriteBack,
    };
    /// カーネル読み書きデータ (RW, NX)。
    pub const KERNEL_RW: Self = Self {
        writable: true,
        ..Self::KERNEL_RO
    };
    /// カーネルコード (RX)。
    pub const KERNEL_RX: Self = Self {
        executable: true,
        ..Self::KERNEL_RO
    };
    /// MMIO (RW, NX, UC)。
    pub const MMIO: Self = Self {
        cache: CacheMode::Uncached,
        ..Self::KERNEL_RW
    };
    /// framebuffer (RW, NX, WC)。
    pub const FRAMEBUFFER: Self = Self {
        cache: CacheMode::WriteCombining,
        ..Self::KERNEL_RW
    };
    /// ユーザー読み書きデータ (RW, NX, User, 非 Global)。
    pub const USER_RW: Self = Self {
        user: true,
        global: false,
        ..Self::KERNEL_RW
    };
    /// ユーザーコード (RX, User, 非 Global)。
    pub const USER_RX: Self = Self {
        user: true,
        global: false,
        ..Self::KERNEL_RX
    };

    fn to_bits(self, size: PageSize) -> u64 {
        let mut bits = ENTRY_PRESENT;
        if self.writable {
            bits |= ENTRY_WRITABLE;
        }
        if self.user {
            bits |= ENTRY_USER;
        }
        if !self.executable && NX_ENABLED.load(Ordering::Relaxed) {
            bits |= ENTRY_NO_EXECUTE;
        }
        if self.global && GLOBAL_ENABLED.load(Ordering::Relaxed) {
            bits |= ENTRY_GLOBAL;
        }
        let pat_bit = match size {
            PageSize::Size4K => ENTRY_PAT_4K,
            PageSize::Size2M | PageSize::Size1G => ENTRY_PAT_LARGE,
        };
        bits |= match self.cache {
            CacheMode::WriteBack => 0,
            CacheMode::Uncached => ENTRY_PCD | ENTRY_PWT,
            CacheMode::WriteCombining if PAT_ENABLED.load(Ordering::Relaxed) => pat_bit,
            // PAT が使えない CPU では WC の代わりに UC
            CacheMode::WriteCombining => ENTRY_PCD | ENTRY_PWT,
        };
        if size != PageSize::Size4K {
            bits |= ENTRY_HUGE;
        }
        bits
    }

    fn from_bits(bits: u64, size: PageSize) -> Self {
        let pat_bit = match size {
            PageSize::Size4K => ENTRY_PAT_4K,
            PageSize::Size2M | PageSize::Size1G => ENTRY_PAT_LARGE,
        };
        let cache = match (
            bits & pat_bit != 0,
            bits & ENTRY_PCD != 0,
            bits & ENTRY_PWT != 0,
        ) {
            (false, false, false) => CacheMode::WriteBack,
            (true, false, false) => CacheMode::WriteCombining,
            _ => CacheMode::Uncached,
        };
        Self {
            writable: bits & ENTRY_WRITABLE != 0,
            executable: bits & ENTRY_NO_EXECUTE == 0,
            user: bits & ENTRY_USER != 0,
            global: bits & ENTRY_GLOBAL != 0,
            cache,
        }
    }
}

impl fmt::Display for PageFlags {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "R{}{}{}{} {:?}",
            if self.writable { "W" } else { "-" },
            if self.executable { "X" } else { "-" },
            if self.user { " user" } else { "" },
            if self.global { " global" } else { "" },
            self.cache
        )
    }
}

/// マップの失敗。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MapError {
    /// 既にマップされている
    AlreadyMapped(VirtAddr),
    /// 大きいページと重なる (分割は未対応)
    HugePageConflict(VirtAddr),
    /// アドレスがページサイズに揃っていない
    Misaligned,
    /// 書き込み可能かつ実行可能なマップの要求 (W^X 違反)
    WritableAndExecutable,
    /// カーネル空間に user ページ、またはユーザー空間にカーネルページを置こうとした
    WrongHalf(VirtAddr),
    /// ページテーブル用のフレームが確保できない
    Frame(FrameError),
    /// 仮想アドレス領域が足りない
    OutOfVirtualSpace,
    /// マップされていない
    NotMapped(VirtAddr),
}

impl From<FrameError> for MapError {
    fn from(e: FrameError) -> Self {
        Self::Frame(e)
    }
}

impl fmt::Display for MapError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AlreadyMapped(v) => write!(f, "{v} is already mapped"),
            Self::HugePageConflict(v) => write!(f, "{v} overlaps a large page"),
            Self::Misaligned => f.write_str("address is not aligned to the page size"),
            Self::WritableAndExecutable => f.write_str("W^X violation: writable and executable"),
            Self::WrongHalf(v) => write!(f, "{v} is in the wrong half of the address space"),
            Self::Frame(e) => write!(f, "page table frame: {e}"),
            Self::OutOfVirtualSpace => f.write_str("out of virtual address space"),
            Self::NotMapped(v) => write!(f, "{v} is not mapped"),
        }
    }
}

/// 仮想アドレスの変換結果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Translation {
    /// 対応する物理アドレス (ページ内オフセット込み)
    pub phys: PhysAddr,
    /// 葉のページの大きさ
    pub size: PageSize,
    /// 権限
    pub flags: PageFlags,
}

#[repr(C, align(4096))]
struct PageTable {
    entries: [u64; 512],
}

/// ページテーブルのフレームを HHDM 経由で指すポインタ。
fn table_ptr(frame: PhysFrame) -> *mut PageTable {
    hhdm::phys_to_virt(frame.start()).as_mut_ptr()
}

/// テーブル `table` の `index` 番目のエントリへのポインタ。
fn entry_ptr(table: PhysFrame, index: usize) -> *mut u64 {
    assert!(index < 512, "page table index out of range");
    table_ptr(table).cast::<u64>().wrapping_add(index)
}

fn read_entry(table: PhysFrame, index: usize) -> u64 {
    let ptr = entry_ptr(table, index);
    // SAFETY: `table` はページテーブルとして確保したフレームで HHDM でマップされており、
    // entry_ptr が index < 512 を確認している。ハードウェアのページウォーカーと共有する
    // メモリなので volatile で読む。
    unsafe { ptr.read_volatile() }
}

fn write_entry(table: PhysFrame, index: usize, value: u64) {
    let ptr = entry_ptr(table, index);
    // SAFETY: read_entry と同じ。書き込みはアドレス空間の `&mut self` を持つ呼び出し元
    // (= そのアドレス空間のロック保持者) だけが行う。
    unsafe { ptr.write_volatile(value) };
}

fn entry_frame(entry: u64) -> PhysFrame {
    PhysFrame::from_start(PhysAddr::new(entry & ADDR_MASK_4K).unwrap_or_default())
        .unwrap_or_else(|| unreachable!("masked address is page aligned"))
}

fn table_is_empty(table: PhysFrame) -> bool {
    (0..512).all(|i| read_entry(table, i) & ENTRY_PRESENT == 0)
}

/// アドレス空間の種類。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpaceKind {
    /// カーネルのアドレス空間 (下位半分は空)
    Kernel,
    /// プロセスのアドレス空間 (上位半分はカーネルと共有)
    User,
}

/// 1 つのアドレス空間 (= 1 つの PML4)。
///
/// 上位半分の PML4 エントリはカーネル空間 ([`AddressSpace::new_kernel`]) で
/// 全て割り当て済みにしてあり、ユーザー空間 ([`AddressSpace::new_user`]) はそれを
/// コピーして共有する。
pub struct AddressSpace {
    pml4: PhysFrame,
    kind: SpaceKind,
}

impl fmt::Debug for AddressSpace {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AddressSpace")
            .field("pml4", &self.pml4)
            .field("kind", &self.kind)
            .finish()
    }
}

impl AddressSpace {
    /// カーネルのアドレス空間を作る: PML4 と、上位半分 256 エントリ分の PDPT を確保する。
    pub fn new_kernel() -> Result<Self, MapError> {
        let pml4 = frame::alloc_zeroed()?;
        for i in 256..512 {
            let pdpt = frame::alloc_zeroed()?;
            write_entry(
                pml4,
                i,
                pdpt.start().as_u64() | ENTRY_PRESENT | ENTRY_WRITABLE,
            );
        }
        Ok(Self {
            pml4,
            kind: SpaceKind::Kernel,
        })
    }

    /// プロセス用のアドレス空間を作る。上位半分はカーネル空間の PML4 エントリをコピーする。
    #[allow(dead_code)] // Phase 7 (プロセス) で使用。Phase 2 ではセルフテストで検証
    pub fn new_user(kernel: &AddressSpace) -> Result<Self, MapError> {
        let pml4 = frame::alloc_zeroed()?;
        for i in 256..512 {
            write_entry(pml4, i, read_entry(kernel.pml4, i));
        }
        Ok(Self {
            pml4,
            kind: SpaceKind::User,
        })
    }

    /// PML4 のフレーム。
    pub fn pml4(&self) -> PhysFrame {
        self.pml4
    }

    /// 上位半分の PML4 エントリ `index` (256〜511) の値 (共有の検証用)。
    pub fn kernel_pml4_entry(&self, index: usize) -> u64 {
        assert!((256..512).contains(&index));
        read_entry(self.pml4, index)
    }

    /// 現在 CR3 にロードされているか。
    pub fn is_active(&self) -> bool {
        registers::read_cr3() & ADDR_MASK_4K == self.pml4.start().as_u64()
    }

    /// このアドレス空間へ切り替える (CR3 をロード)。
    ///
    /// # Safety
    ///
    /// 現在実行中のコード・スタック・以降アクセスするデータがこの空間でマップされていること。
    /// カーネル空間・ユーザー空間とも上位半分は共有されるので、カーネル内からの切り替えは
    /// 通常この条件を満たす。
    pub unsafe fn activate(&self) {
        // SAFETY: 呼び出し元の契約により、切り替え後も実行に必要な領域がマップされている。
        unsafe { registers::write_cr3(self.pml4.start().as_u64()) };
    }

    fn check_half(&self, virt: VirtAddr, flags: PageFlags) -> Result<(), MapError> {
        // カーネル空間 (上位半分) に user ページは置かない。ユーザー空間の下位半分には
        // user ページだけを置く (カーネルのデータはすべて上位半分)。
        // セルフテストはカーネル空間の下位半分へ user ページを一時的に置くので許可する。
        if virt.is_kernel_half() && flags.user {
            return Err(MapError::WrongHalf(virt));
        }
        if self.kind == SpaceKind::User && !virt.is_kernel_half() && !flags.user {
            return Err(MapError::WrongHalf(virt));
        }
        Ok(())
    }

    /// 仮想ページ `virt` を物理 `phys` に `size` の大きさでマップする。
    ///
    /// # Safety
    ///
    /// - `phys` は呼び出し元がマップしてよい物理メモリであること (他の用途で使われている
    ///   RAM を書き込み可能にマップすると、Rust の別名規則を破り得る)。
    /// - 上位半分の場合、その仮想アドレスを他のカーネルコードが使っていないこと。
    pub unsafe fn map(
        &mut self,
        virt: VirtAddr,
        phys: PhysAddr,
        size: PageSize,
        flags: PageFlags,
    ) -> Result<(), MapError> {
        if !virt.is_aligned(size.bytes()) || !phys.is_aligned(size.bytes()) {
            return Err(MapError::Misaligned);
        }
        if flags.writable && flags.executable {
            return Err(MapError::WritableAndExecutable);
        }
        self.check_half(virt, flags)?;
        let leaf_level = match size {
            PageSize::Size1G => 1, // PDPT
            PageSize::Size2M => 2, // PD
            PageSize::Size4K => 3, // PT
        };
        let idx = virt.table_indices();
        let mut table = self.pml4;
        for (level, &index) in idx.iter().enumerate().take(leaf_level) {
            let entry = read_entry(table, index);
            if entry & ENTRY_PRESENT == 0 {
                if level == 0 && virt.is_kernel_half() {
                    // 上位半分の PML4 エントリは new_kernel で全て埋めてある
                    unreachable!("kernel PML4 entry {index} is missing");
                }
                let next = frame::alloc_zeroed()?;
                let mut bits = next.start().as_u64() | ENTRY_PRESENT | ENTRY_WRITABLE;
                if flags.user {
                    bits |= ENTRY_USER;
                }
                write_entry(table, index, bits);
                table = next;
            } else if entry & ENTRY_HUGE != 0 && level > 0 {
                return Err(MapError::HugePageConflict(virt));
            } else {
                if flags.user && entry & ENTRY_USER == 0 {
                    // 下位半分の中間テーブルにだけ到達する (上位半分の user は check_half で拒否)
                    write_entry(table, index, entry | ENTRY_USER);
                }
                table = entry_frame(entry);
            }
        }
        let leaf_index = idx[leaf_level];
        let existing = read_entry(table, leaf_index);
        if existing & ENTRY_PRESENT != 0 {
            return Err(if existing & ENTRY_HUGE != 0 && size == PageSize::Size4K {
                MapError::HugePageConflict(virt)
            } else {
                MapError::AlreadyMapped(virt)
            });
        }
        // 大きいページを置く位置に下位テーブルがある場合 (present かつ非 huge) も
        // 上で AlreadyMapped として拒否している。
        let addr_bits = phys.as_u64()
            & match size {
                PageSize::Size4K => ADDR_MASK_4K,
                PageSize::Size2M => ADDR_MASK_2M,
                PageSize::Size1G => ADDR_MASK_1G,
            };
        write_entry(table, leaf_index, addr_bits | flags.to_bits(size));
        Ok(())
    }

    /// 葉のエントリまでの経路 (各レベルのテーブルとインデックス) を辿る。
    /// 戻り値: (葉のテーブル, 葉のインデックス, ページサイズ, 経路)
    fn walk(&self, virt: VirtAddr) -> Option<(PhysFrame, usize, PageSize, [PhysFrame; 4])> {
        let idx = virt.table_indices();
        let mut path = [self.pml4; 4];
        let mut table = self.pml4;
        for level in 0..4 {
            path[level] = table;
            let entry = read_entry(table, idx[level]);
            if entry & ENTRY_PRESENT == 0 {
                return None;
            }
            let size = match level {
                1 if entry & ENTRY_HUGE != 0 => Some(PageSize::Size1G),
                2 if entry & ENTRY_HUGE != 0 => Some(PageSize::Size2M),
                3 => Some(PageSize::Size4K),
                _ => None,
            };
            if let Some(size) = size {
                return Some((table, idx[level], size, path));
            }
            table = entry_frame(entry);
        }
        None
    }

    /// 仮想アドレスを物理アドレスへ変換する。
    pub fn translate(&self, virt: VirtAddr) -> Option<Translation> {
        let (table, index, size, _) = self.walk(virt)?;
        let entry = read_entry(table, index);
        let mask = match size {
            PageSize::Size4K => ADDR_MASK_4K,
            PageSize::Size2M => ADDR_MASK_2M,
            PageSize::Size1G => ADDR_MASK_1G,
        };
        let phys = (entry & mask) + (virt.as_u64() & (size.bytes() - 1));
        Some(Translation {
            phys: PhysAddr::new(phys)?,
            size,
            flags: PageFlags::from_bits(entry, size),
        })
    }

    /// `virt` のマップを外す。空になった中間テーブルは解放する
    /// (ただし上位半分の PML4 エントリが指す PDPT は共有のため残す)。
    ///
    /// 戻り値はマップされていた物理アドレスと大きさ (葉のフレームは解放しない)。
    ///
    /// # Safety
    ///
    /// そのページを参照しているコードが無いこと (外した後のアクセスは #PF になる)。
    pub unsafe fn unmap(&mut self, virt: VirtAddr) -> Result<(PhysAddr, PageSize), MapError> {
        let (table, index, size, path) = self.walk(virt).ok_or(MapError::NotMapped(virt))?;
        if !virt.is_aligned(size.bytes()) {
            return Err(MapError::HugePageConflict(virt));
        }
        let entry = read_entry(table, index);
        let mask = match size {
            PageSize::Size4K => ADDR_MASK_4K,
            PageSize::Size2M => ADDR_MASK_2M,
            PageSize::Size1G => ADDR_MASK_1G,
        };
        write_entry(table, index, 0);
        let leaf_level = match size {
            PageSize::Size1G => 1,
            PageSize::Size2M => 2,
            PageSize::Size4K => 3,
        };
        // 空になった中間テーブルを下から解放する
        let idx = virt.table_indices();
        let mut level = leaf_level;
        while level >= 1 {
            let t = path[level];
            // 上位半分の PDPT (level 1) は全アドレス空間で共有なので解放しない
            if (level == 1 && virt.is_kernel_half()) || !table_is_empty(t) {
                break;
            }
            write_entry(path[level - 1], idx[level - 1], 0);
            // SAFETY: t は今このアドレス空間から外したページテーブルで、空であり、
            // どこからも参照されていない (下位半分のテーブルは各アドレス空間が専有し、
            // 上位半分の PD/PT はカーネル空間の PDPT からだけ参照される)。
            if let Err(e) = unsafe { frame::free(t) } {
                warn!("failed to free page table frame {:?}: {e}", t);
            }
            level -= 1;
        }
        if virt.is_kernel_half() || self.is_active() {
            registers::invlpg(virt.as_u64());
        }
        Ok((
            PhysAddr::new(entry & mask).ok_or(MapError::NotMapped(virt))?,
            size,
        ))
    }

    /// 既存のマップの権限を変更する。
    ///
    /// # Safety
    ///
    /// 権限を落とす場合、そのページを旧権限で使っているコードが無いこと。
    #[allow(dead_code)] // Phase 3 以降 (framebuffer の二重化等) で使う想定。セルフテストで検証
    pub unsafe fn protect(&mut self, virt: VirtAddr, flags: PageFlags) -> Result<(), MapError> {
        if flags.writable && flags.executable {
            return Err(MapError::WritableAndExecutable);
        }
        self.check_half(virt, flags)?;
        let (table, index, size, _) = self.walk(virt).ok_or(MapError::NotMapped(virt))?;
        let entry = read_entry(table, index);
        let mask = match size {
            PageSize::Size4K => ADDR_MASK_4K,
            PageSize::Size2M => ADDR_MASK_2M,
            PageSize::Size1G => ADDR_MASK_1G,
        };
        write_entry(table, index, (entry & mask) | flags.to_bits(size));
        if virt.is_kernel_half() || self.is_active() {
            registers::invlpg(virt.as_u64());
        }
        Ok(())
    }

    /// PML4 インデックス `pml4_indices` の範囲にある葉のマップをすべて走査する (監査・診断用)。
    /// コールバックには (仮想アドレス, 大きさ, 権限) が渡る。
    pub fn for_each_leaf(
        &self,
        pml4_indices: core::ops::Range<usize>,
        mut f: impl FnMut(VirtAddr, PageSize, PageFlags),
    ) {
        for i4 in pml4_indices {
            let e4 = read_entry(self.pml4, i4);
            if e4 & ENTRY_PRESENT == 0 {
                continue;
            }
            let base4 = sign_extend((i4 as u64) << 39);
            let pdpt = entry_frame(e4);
            for i3 in 0..512 {
                let base3 = base4 + ((i3 as u64) << 30);
                let e3 = read_entry(pdpt, i3);
                if e3 & ENTRY_PRESENT == 0 {
                    continue;
                }
                if e3 & ENTRY_HUGE != 0 {
                    let flags = PageFlags::from_bits(e3, PageSize::Size1G);
                    f(VirtAddr::new_canonical(base3), PageSize::Size1G, flags);
                    continue;
                }
                let pd = entry_frame(e3);
                for i2 in 0..512 {
                    let base2 = base3 + ((i2 as u64) << 21);
                    let e2 = read_entry(pd, i2);
                    if e2 & ENTRY_PRESENT == 0 {
                        continue;
                    }
                    if e2 & ENTRY_HUGE != 0 {
                        let flags = PageFlags::from_bits(e2, PageSize::Size2M);
                        f(VirtAddr::new_canonical(base2), PageSize::Size2M, flags);
                        continue;
                    }
                    let pt = entry_frame(e2);
                    for i1 in 0..512 {
                        let e1 = read_entry(pt, i1);
                        if e1 & ENTRY_PRESENT != 0 {
                            let flags = PageFlags::from_bits(e1, PageSize::Size4K);
                            f(
                                VirtAddr::new_canonical(base2 + ((i1 as u64) << 12)),
                                PageSize::Size4K,
                                flags,
                            );
                        }
                    }
                }
            }
        }
    }

    /// ユーザー空間を破棄する: 下位半分のページテーブルと PML4 を解放する。
    /// 葉がマップしていたフレームは解放しない (所有者が別途解放する)。
    ///
    /// # Safety
    ///
    /// このアドレス空間が CR3 にロードされていないこと。
    #[allow(dead_code)] // Phase 7 で使用。Phase 2 ではセルフテストで検証
    pub unsafe fn destroy(self) -> Result<u64, MapError> {
        assert!(
            self.kind == SpaceKind::User,
            "the kernel address space cannot be destroyed"
        );
        assert!(!self.is_active(), "cannot destroy the active address space");
        let mut freed = 0u64;
        for i4 in 0..256 {
            let e4 = read_entry(self.pml4, i4);
            if e4 & ENTRY_PRESENT == 0 {
                continue;
            }
            let pdpt = entry_frame(e4);
            for i3 in 0..512 {
                let e3 = read_entry(pdpt, i3);
                if e3 & ENTRY_PRESENT == 0 || e3 & ENTRY_HUGE != 0 {
                    continue;
                }
                let pd = entry_frame(e3);
                for i2 in 0..512 {
                    let e2 = read_entry(pd, i2);
                    if e2 & ENTRY_PRESENT == 0 || e2 & ENTRY_HUGE != 0 {
                        continue;
                    }
                    // SAFETY: このユーザー空間専有の PT で、空間ごと破棄するので参照は残らない。
                    unsafe { frame::free(entry_frame(e2))? };
                    freed += 1;
                }
                // SAFETY: 同上 (PD)。
                unsafe { frame::free(pd)? };
                freed += 1;
            }
            // SAFETY: 同上 (PDPT)。
            unsafe { frame::free(pdpt)? };
            freed += 1;
        }
        // SAFETY: 同上 (PML4)。上位半分のエントリはカーネル空間の PDPT を指すだけなので解放しない。
        unsafe { frame::free(self.pml4)? };
        Ok(freed + 1)
    }
}

/// 48bit の仮想アドレスを正規形へ符号拡張する。
const fn sign_extend(v: u64) -> u64 {
    if v & (1 << 47) != 0 {
        v | 0xffff_0000_0000_0000
    } else {
        v
    }
}

/// 現在の CR3 が指す PML4 の物理アドレス。
pub fn active_pml4() -> PhysAddr {
    PhysAddr::new(registers::read_cr3() & ADDR_MASK_4K).unwrap_or_default()
}

/// PAT を [`PAT_VALUE`] に設定する。
///
/// # Safety
///
/// CPU が PAT に対応していること。現在のマップが PAT index 4 (WC) を使っていないこと
/// (起動時、カーネルのページテーブルに切り替える直前に呼ぶ)。
pub unsafe fn program_pat() {
    // Intel SDM 11.12.4: PAT 変更時はキャッシュを書き戻してから変更し、TLB を flush する
    // (直後の CR3 ロードで非グローバル TLB が、CR4.PGE の再設定で全 TLB が無効化される)。
    registers::wbinvd();
    // SAFETY: 呼び出し元の契約により PAT 対応 CPU。値は SDM で定義された種別のみ。
    unsafe { registers::wrmsr(registers::MSR_PAT, PAT_VALUE) };
    registers::wbinvd();
}

/// ページサイズの小さい方から順に、`[virt, virt+len)` を `phys` 起点で
/// 可能な限り大きいページでマップするための次のページサイズを選ぶ。
pub fn best_page_size(virt: u64, phys: u64, remaining: u64, allow_1g: bool) -> PageSize {
    let fits = |s: PageSize| {
        virt.is_multiple_of(s.bytes()) && phys.is_multiple_of(s.bytes()) && remaining >= s.bytes()
    };
    if allow_1g && fits(PageSize::Size1G) {
        PageSize::Size1G
    } else if fits(PageSize::Size2M) {
        PageSize::Size2M
    } else {
        PageSize::Size4K
    }
}

const _: () = assert!(PAGE_SIZE == 4096);

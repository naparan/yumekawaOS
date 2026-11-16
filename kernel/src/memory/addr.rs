//! 物理アドレス・仮想アドレス・フレーム・ページの型。
//!
//! 物理アドレスと仮想アドレスを別の型にして取り違えを防ぐ。

use core::fmt;

/// 4KiB。
pub const PAGE_SIZE: u64 = 4096;
/// 2MiB。
pub const LARGE_PAGE_SIZE: u64 = 2 * 1024 * 1024;
/// 1GiB。
pub const HUGE_PAGE_SIZE: u64 = 1024 * 1024 * 1024;

/// 物理アドレス (52bit まで)。
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
#[repr(transparent)]
pub struct PhysAddr(u64);

impl PhysAddr {
    /// x86_64 のアーキテクチャ上の上限 (52bit)。
    pub const MAX: u64 = (1 << 52) - 1;

    /// 物理アドレスを作る。52bit を超える値は `None`。
    pub const fn new(addr: u64) -> Option<Self> {
        if addr <= Self::MAX {
            Some(Self(addr))
        } else {
            None
        }
    }

    /// 数値。
    pub const fn as_u64(self) -> u64 {
        self.0
    }

    /// `align` (2 のべき乗) の倍数か。
    pub const fn is_aligned(self, align: u64) -> bool {
        self.0 & (align - 1) == 0
    }

    /// `align` (2 のべき乗) の倍数へ切り下げ。
    pub const fn align_down(self, align: u64) -> Self {
        Self(self.0 & !(align - 1))
    }
}

impl fmt::Debug for PhysAddr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PhysAddr({:#x})", self.0)
    }
}

impl fmt::Display for PhysAddr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:#x}", self.0)
    }
}

/// 正規形 (canonical) の仮想アドレス (4 段ページング, 48bit)。
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
#[repr(transparent)]
pub struct VirtAddr(u64);

impl VirtAddr {
    /// 正規形なら作る。
    pub const fn new(addr: u64) -> Option<Self> {
        let top = addr >> 47;
        if top == 0 || top == 0x1_ffff {
            Some(Self(addr))
        } else {
            None
        }
    }

    /// 正規形であることが分かっている値から作る (違えばパニック)。
    #[track_caller]
    pub const fn new_canonical(addr: u64) -> Self {
        match Self::new(addr) {
            Some(v) => v,
            None => panic!("non-canonical virtual address"),
        }
    }

    /// 数値。
    pub const fn as_u64(self) -> u64 {
        self.0
    }

    /// ポインタへ変換。
    pub const fn as_ptr<T>(self) -> *const T {
        self.0 as *const T
    }

    /// 可変ポインタへ変換。
    pub const fn as_mut_ptr<T>(self) -> *mut T {
        self.0 as *mut T
    }

    /// `align` の倍数か。
    pub const fn is_aligned(self, align: u64) -> bool {
        self.0 & (align - 1) == 0
    }

    /// 上位半分 (カーネル空間) か。
    pub const fn is_kernel_half(self) -> bool {
        self.0 >> 63 == 1
    }

    /// 4 段ページングの各レベルのインデックス (PML4, PDPT, PD, PT)。
    pub const fn table_indices(self) -> [usize; 4] {
        [
            ((self.0 >> 39) & 0x1ff) as usize,
            ((self.0 >> 30) & 0x1ff) as usize,
            ((self.0 >> 21) & 0x1ff) as usize,
            ((self.0 >> 12) & 0x1ff) as usize,
        ]
    }
}

impl fmt::Debug for VirtAddr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "VirtAddr({:#x})", self.0)
    }
}

impl fmt::Display for VirtAddr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:#x}", self.0)
    }
}

/// 4KiB の物理フレーム。
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PhysFrame {
    start: PhysAddr,
}

impl PhysFrame {
    /// 4KiB 境界のアドレスからフレームを作る。
    pub const fn from_start(start: PhysAddr) -> Option<Self> {
        if start.is_aligned(PAGE_SIZE) {
            Some(Self { start })
        } else {
            None
        }
    }

    /// フレーム番号から作る。
    pub const fn from_number(number: u64) -> Option<Self> {
        match number.checked_mul(PAGE_SIZE) {
            Some(a) => match PhysAddr::new(a) {
                Some(start) => Some(Self { start }),
                None => None,
            },
            None => None,
        }
    }

    /// 先頭の物理アドレス。
    pub const fn start(self) -> PhysAddr {
        self.start
    }

    /// フレーム番号 (= アドレス / 4096)。
    pub const fn number(self) -> u64 {
        self.start.as_u64() / PAGE_SIZE
    }
}

impl fmt::Debug for PhysFrame {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PhysFrame({:#x})", self.start.as_u64())
    }
}

/// ページの大きさ。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageSize {
    /// 4KiB (PT エントリ)
    Size4K,
    /// 2MiB (PD エントリ, PS=1)
    Size2M,
    /// 1GiB (PDPT エントリ, PS=1)
    Size1G,
}

impl PageSize {
    /// バイト数。
    pub const fn bytes(self) -> u64 {
        match self {
            Self::Size4K => PAGE_SIZE,
            Self::Size2M => LARGE_PAGE_SIZE,
            Self::Size1G => HUGE_PAGE_SIZE,
        }
    }

    /// 表示名。
    pub const fn name(self) -> &'static str {
        match self {
            Self::Size4K => "4KiB",
            Self::Size2M => "2MiB",
            Self::Size1G => "1GiB",
        }
    }
}

/// `v` を `align` (2 のべき乗) の倍数へ切り上げる。
pub const fn align_up(v: u64, align: u64) -> u64 {
    (v + align - 1) & !(align - 1)
}

/// `v` を `align` (2 のべき乗) の倍数へ切り下げる。
pub const fn align_down(v: u64, align: u64) -> u64 {
    v & !(align - 1)
}

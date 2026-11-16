//! ブートローダから受け取る情報 (§32: Bootloader interface)。
//!
//! Limine boot protocol の生の応答構造体は `protocol` サブモジュールに
//! 閉じ込め、カーネルの他の部分には safe な [`BootInfo`] だけを渡す。
//!
//! [`take`] は 1 度しか成功しない。これにより、framebuffer への書き込み権
//! ([`FramebufferInfo`]) がカーネル内で一意に所有されることを保証する。
//!
//! Phase 2 から、[`take`] はメモリマップと文字列 (コマンドライン・ブートローダ名) を
//! **カーネル自身のメモリへコピー**して返す。Limine の応答は bootloader-reclaimable 領域に
//! あり、メモリ管理の初期化後にその領域を回収 (フレームアロケータへ返却) するため、
//! 回収後も [`BootInfo`] を安全に使い続けられるようにする。

mod protocol;

use core::fmt;
use core::ptr::NonNull;

pub use self::protocol::take;

/// ブート情報の取得エラー。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootError {
    /// [`take`] が 2 回以上呼ばれた。
    AlreadyTaken,
    /// ブートローダが要求した base revision をサポートしていない。
    UnsupportedBaseRevision {
        /// 要求した revision
        requested: u64,
        /// ブートローダが実際に使った revision (不明なら `None`)
        loaded: Option<u64>,
    },
    /// 必須の応答が返らなかった。
    MissingResponse(&'static str),
    /// メモリマップの領域が多すぎて保持できない。
    MemoryMapTooLarge {
        /// ブートローダが返した領域数
        entries: usize,
        /// 保持できる上限
        capacity: usize,
    },
}

impl fmt::Display for BootError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AlreadyTaken => f.write_str("boot information was already taken"),
            Self::UnsupportedBaseRevision {
                requested,
                loaded: Some(loaded),
            } => write!(
                f,
                "Limine base revision {requested} is not supported (bootloader uses {loaded})"
            ),
            Self::UnsupportedBaseRevision {
                requested,
                loaded: None,
            } => write!(
                f,
                "Limine base revision {requested} is not supported (not loaded by a Limine-compliant bootloader?)"
            ),
            Self::MissingResponse(what) => {
                write!(f, "bootloader did not answer the {what} request")
            }
            Self::MemoryMapTooLarge { entries, capacity } => write!(
                f,
                "memory map has {entries} regions (only {capacity} are supported)"
            ),
        }
    }
}

/// ブートローダから受け取った情報一式。
#[derive(Debug)]
pub struct BootInfo {
    /// ブートローダ名・バージョン。
    pub bootloader: Option<BootloaderInfo>,
    /// ファームウェアの種類。
    pub firmware: FirmwareKind,
    /// HHDM (Higher Half Direct Map) のオフセット。
    /// 物理アドレス `p` は仮想アドレス `p + hhdm_offset` でアクセスできる。
    /// ただし base revision 3 でマップされるのは usable / bootloader-reclaimable /
    /// kernel-and-modules / framebuffer 等の領域だけで、ACPI 領域・予約領域・MMIO は
    /// マップされない点に注意。
    pub hhdm_offset: u64,
    /// 物理メモリマップ。
    pub memory_map: MemoryMap,
    /// 線形 framebuffer (最初の 1 枚)。無い環境では `None`。
    pub framebuffer: Option<FramebufferInfo>,
    /// framebuffer の総数(参考情報)。
    pub framebuffer_count: usize,
    /// カーネルがロードされた物理/仮想ベースアドレス。
    pub kernel_address: Option<KernelAddress>,
    /// ACPI RSDP の**物理**アドレス (base revision 3 の仕様。HHDM 上にはマップされない)。
    pub rsdp_address: Option<u64>,
    /// カーネルコマンドライン (limine.conf の `cmdline:`)。カーネル内にコピー済み。
    pub cmdline: BootString<CMDLINE_CAPACITY>,
}

/// コマンドラインの最大長 (超えた分は切り捨て、[`BootString::is_truncated`] が真になる)。
pub const CMDLINE_CAPACITY: usize = 512;

/// ブートローダから受け取った文字列のコピー (固定長)。
#[derive(Clone, Copy)]
pub struct BootString<const N: usize> {
    buf: [u8; N],
    len: usize,
    truncated: bool,
}

impl<const N: usize> BootString<N> {
    /// 文字列をコピーする (容量を超えた分は文字境界で切り捨て)。
    pub fn copy_from(s: &str) -> Self {
        let mut take = s.len().min(N);
        while !s.is_char_boundary(take) {
            take -= 1;
        }
        let mut buf = [0u8; N];
        buf[..take].copy_from_slice(&s.as_bytes()[..take]);
        Self {
            buf,
            len: take,
            truncated: take < s.len(),
        }
    }

    /// 文字列。
    pub fn as_str(&self) -> &str {
        core::str::from_utf8(&self.buf[..self.len]).unwrap_or("")
    }

    /// 容量不足で切り捨てたか。
    pub fn is_truncated(&self) -> bool {
        self.truncated
    }

    /// 空か。
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

impl<const N: usize> fmt::Debug for BootString<N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self.as_str(), f)
    }
}

impl<const N: usize> fmt::Display for BootString<N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// ブートローダ名・バージョン (カーネル内にコピー済み)。
#[derive(Debug, Clone, Copy)]
pub struct BootloaderInfo {
    /// 例: "Limine"
    pub name: BootString<32>,
    /// 例: "11.4.1"
    pub version: BootString<32>,
}

/// ファームウェアの種類。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FirmwareKind {
    /// レガシー BIOS
    Bios,
    /// 32bit UEFI
    Uefi32,
    /// 64bit UEFI
    Uefi64,
    /// 応答が無い、または未知の値
    Unknown,
}

/// カーネルのロードアドレス。
#[derive(Debug, Clone, Copy)]
pub struct KernelAddress {
    /// 物理ベースアドレス
    pub physical_base: u64,
    /// 仮想ベースアドレス(linker.ld の 0xffffffff80000000)
    pub virtual_base: u64,
}

/// メモリ領域の種類 (Limine のメモリマップ種別に対応)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryKind {
    /// 自由に使用できる RAM
    Usable,
    /// 予約済み
    Reserved,
    /// ACPI テーブル(解析後に回収可能)
    AcpiReclaimable,
    /// ACPI NVS(使用不可)
    AcpiNvs,
    /// 不良メモリ
    BadMemory,
    /// ブートローダが使用中(応答を使い終えたら回収可能)
    BootloaderReclaimable,
    /// カーネルとモジュール
    KernelAndModules,
    /// framebuffer
    Framebuffer,
    /// 未知の種別
    Unknown,
}

impl MemoryKind {
    /// 表示用の名前。
    pub const fn name(self) -> &'static str {
        match self {
            Self::Usable => "usable",
            Self::Reserved => "reserved",
            Self::AcpiReclaimable => "acpi-reclaimable",
            Self::AcpiNvs => "acpi-nvs",
            Self::BadMemory => "bad-memory",
            Self::BootloaderReclaimable => "bootloader-reclaimable",
            Self::KernelAndModules => "kernel-and-modules",
            Self::Framebuffer => "framebuffer",
            Self::Unknown => "unknown",
        }
    }
}

/// 物理メモリ領域。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemoryRegion {
    /// 物理ベースアドレス
    pub base: u64,
    /// バイト長
    pub length: u64,
    /// 種別
    pub kind: MemoryKind,
}

impl MemoryRegion {
    /// 終端(この値は含まない)。
    pub const fn end(&self) -> u64 {
        self.base.saturating_add(self.length)
    }
}

/// 物理メモリマップ (Limine の応答をカーネル内にコピーしたもの)。
///
/// エントリはベースアドレス昇順・重複なし (Limine の仕様)。
/// Phase 1 では Limine の応答 (bootloader-reclaimable 領域) を直接参照していたが、
/// Phase 2 でその領域を回収するため、[`take`] の時点でコピーするようにした。
#[derive(Clone, Copy)]
pub struct MemoryMap {
    entries: [MemoryRegion; MEMORY_MAP_CAPACITY],
    len: usize,
}

/// 保持できるメモリマップの領域数。
pub const MEMORY_MAP_CAPACITY: usize = 512;

impl MemoryMap {
    /// 領域数。
    pub fn len(&self) -> usize {
        self.len
    }

    /// 領域が 1 つも無いか。
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// 全領域を走査する。
    pub fn iter(&self) -> impl Iterator<Item = MemoryRegion> + '_ {
        self.entries[..self.len].iter().copied()
    }

    /// 指定種別の合計バイト数。
    pub fn total_bytes(&self, kind: MemoryKind) -> u64 {
        self.iter()
            .filter(|r| r.kind == kind)
            .map(|r| r.length)
            .sum()
    }
}

impl fmt::Debug for MemoryMap {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.iter()).finish()
    }
}

/// framebuffer の 1 チャネル (R/G/B) のビット配置。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChannelMask {
    /// ビット数
    pub size: u8,
    /// LSB からのシフト量
    pub shift: u8,
}

/// ブートローダが設定した線形 framebuffer の情報と、その書き込み権。
///
/// 値は非 `Clone` で、[`take`] によって 1 度だけ生成される。
/// これを消費して [`crate::graphics::Framebuffer`] を作る。
#[derive(Debug)]
pub struct FramebufferInfo {
    // フィールドはすべて非公開: 寸法を書き換えて範囲外書き込みを起こせないようにする。
    address: NonNull<u8>,
    width: u64,
    height: u64,
    pitch: u64,
    bpp: u16,
    memory_model: u8,
    red: ChannelMask,
    green: ChannelMask,
    blue: ChannelMask,
}

impl FramebufferInfo {
    /// framebuffer 先頭の仮想アドレス (HHDM 経由でマップ済み)。
    pub fn address(&self) -> NonNull<u8> {
        self.address
    }
    /// 幅 (ピクセル)。
    pub fn width(&self) -> u64 {
        self.width
    }
    /// 高さ (ピクセル)。
    pub fn height(&self) -> u64 {
        self.height
    }
    /// 1 行のバイト数 (`width * bpp / 8` 以上)。
    pub fn pitch(&self) -> u64 {
        self.pitch
    }
    /// 1 ピクセルのビット数。
    pub fn bpp(&self) -> u16 {
        self.bpp
    }
    /// メモリモデル (1 = RGB, それ以外は未対応)。
    pub fn memory_model(&self) -> u8 {
        self.memory_model
    }
    /// 赤チャネルのビット配置。
    pub fn red(&self) -> ChannelMask {
        self.red
    }
    /// 緑チャネルのビット配置。
    pub fn green(&self) -> ChannelMask {
        self.green
    }
    /// 青チャネルのビット配置。
    pub fn blue(&self) -> ChannelMask {
        self.blue
    }
}

//! カーネルのアドレス空間の構築と、上位層向けの safe API。
//!
//! 起動時に Limine のページテーブルから、カーネル自身のページテーブルへ切り替える:
//!
//! | 領域 | マップ | 権限 |
//! | --- | --- | --- |
//! | `.limine_requests` | 4KiB | RW, NX |
//! | `.text` | 4KiB | R, X |
//! | `.rodata` (+ 例外テーブル) | 4KiB | R, NX |
//! | `.data` / `.bss` | 4KiB | RW, NX |
//! | HHDM: usable / bootloader-reclaimable | 1GiB/2MiB/4KiB (揃う範囲で最大) | RW, NX |
//! | HHDM: kernel-and-modules (カーネルイメージの別名) | 同上 | R, NX |
//! | HHDM: ACPI reclaimable / ACPI NVS | 同上 | R, NX |
//! | HHDM: framebuffer | 同上 | RW, NX, Write-Combining |
//!
//! カーネル空間はグローバルな [`IrqSpinLock`] で保護する。ロック順序は
//! kernel heap → kernel space → frames (ページテーブル用フレームの確保)。

use core::fmt;

use super::addr::{PAGE_SIZE, PageSize, PhysAddr, VirtAddr, align_down, align_up};
use super::hhdm;
use super::layout;
use super::paging::{self, AddressSpace, MapError, PageFlags, Translation};
use crate::boot::{BootInfo, MemoryKind};
use crate::sync::IrqSpinLock;

unsafe extern "C" {
    static __kernel_start: [u8; 0];
    static __limine_requests_end: [u8; 0];
    static __text_start: [u8; 0];
    static __text_end: [u8; 0];
    static __rodata_start: [u8; 0];
    static __rodata_end: [u8; 0];
    static __data_start: [u8; 0];
    static __data_end: [u8; 0];
}

/// リンカスクリプトが定義するカーネルイメージの各セクションの範囲 (仮想アドレス)。
#[derive(Debug, Clone, Copy)]
pub struct KernelSections {
    /// `.limine_requests` (先頭〜テキスト手前)
    pub requests: (u64, u64),
    /// `.text`
    pub text: (u64, u64),
    /// `.rodata`
    pub rodata: (u64, u64),
    /// `.data` + `.bss`
    pub data: (u64, u64),
}

impl KernelSections {
    /// リンカシンボルから読む。
    pub fn get() -> Self {
        let a = |p: *const [u8; 0]| p as u64;
        Self {
            requests: (
                a(&raw const __kernel_start),
                a(&raw const __limine_requests_end),
            ),
            text: (a(&raw const __text_start), a(&raw const __text_end)),
            rodata: (a(&raw const __rodata_start), a(&raw const __rodata_end)),
            data: (a(&raw const __data_start), a(&raw const __data_end)),
        }
    }
}

/// 構築結果の統計。
#[derive(Debug, Clone, Copy, Default)]
pub struct BuildReport {
    /// 4KiB ページ数
    pub pages_4k: u64,
    /// 2MiB ページ数
    pub pages_2m: u64,
    /// 1GiB ページ数
    pub pages_1g: u64,
    /// HHDM でマップした物理メモリ量
    pub hhdm_bytes: u64,
    /// HHDM の最大物理アドレス (含まない)
    pub hhdm_top: u64,
    /// ページテーブルに使ったフレーム数
    pub table_frames: u64,
}

/// カーネル空間の構築エラー。
#[derive(Debug, Clone, Copy)]
pub enum BuildError {
    /// ブートローダがカーネルのロードアドレスを返さなかった
    NoKernelAddress,
    /// HHDM がカーネルのレイアウトと重なる
    HhdmOutOfRange {
        /// HHDM オフセット
        offset: u64,
        /// HHDM の終端 (仮想)
        top: u64,
    },
    /// マップに失敗
    Map(MapError),
    /// 切り替え後に必要な領域がマップされていない
    MissingMapping(&'static str, u64),
}

impl From<MapError> for BuildError {
    fn from(e: MapError) -> Self {
        Self::Map(e)
    }
}

impl fmt::Display for BuildError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoKernelAddress => f.write_str("bootloader did not report the kernel address"),
            Self::HhdmOutOfRange { offset, top } => write!(
                f,
                "HHDM [{offset:#x}, {top:#x}) does not fit below {:#x}",
                layout::HHDM_LIMIT
            ),
            Self::Map(e) => write!(f, "{e}"),
            Self::MissingMapping(what, addr) => {
                write!(f, "{what} ({addr:#x}) is not mapped in the new page tables")
            }
        }
    }
}

static KERNEL_SPACE: IrqSpinLock<Option<AddressSpace>> = IrqSpinLock::new(None);

fn count(report: &mut BuildReport, size: PageSize) {
    match size {
        PageSize::Size4K => report.pages_4k += 1,
        PageSize::Size2M => report.pages_2m += 1,
        PageSize::Size1G => report.pages_1g += 1,
    }
}

/// 物理範囲 `[phys, phys+len)` を HHDM へ、揃う範囲で最大のページでマップする。
fn map_hhdm_range(
    space: &mut AddressSpace,
    phys: u64,
    len: u64,
    flags: PageFlags,
    allow_1g: bool,
    report: &mut BuildReport,
) -> Result<(), MapError> {
    let mut p = align_down(phys, PAGE_SIZE);
    let end = align_up(phys + len, PAGE_SIZE);
    let off = hhdm::offset();
    while p < end {
        let size = paging::best_page_size(p + off, p, end - p, allow_1g);
        let virt = VirtAddr::new(p + off).ok_or(MapError::OutOfVirtualSpace)?;
        let pa = PhysAddr::new(p).ok_or(MapError::OutOfVirtualSpace)?;
        // SAFETY: HHDM は物理メモリの別名として作る領域であり、ここでマップするのは
        // RAM / ACPI / framebuffer の領域だけ。HHDM の仮想範囲はこの関数以外では使わない。
        unsafe { space.map(virt, pa, size, flags)? };
        count(report, size);
        p += size.bytes();
    }
    Ok(())
}

fn hhdm_flags(kind: MemoryKind) -> Option<PageFlags> {
    match kind {
        MemoryKind::Usable | MemoryKind::BootloaderReclaimable => Some(PageFlags::KERNEL_RW),
        // カーネルイメージの別名 (HHDM 側) は読み取り専用にし、.text を HHDM 経由で
        // 書き換えられないようにする (W^X を別名でも破らない)。
        MemoryKind::KernelAndModules => Some(PageFlags::KERNEL_RO),
        MemoryKind::AcpiReclaimable | MemoryKind::AcpiNvs => Some(PageFlags::KERNEL_RO),
        MemoryKind::Framebuffer => Some(PageFlags::FRAMEBUFFER),
        MemoryKind::Reserved | MemoryKind::BadMemory | MemoryKind::Unknown => None,
    }
}

/// カーネル自身のページテーブルを構築し、CR3 を切り替える。
///
/// 呼び出し時点では Limine のページテーブル・Limine のスタックで動いている。
/// HHDM を同じオフセットで作り直すので、Limine のスタック (bootloader-reclaimable、
/// HHDM 上) は切り替え後もそのまま使える。
///
/// `framebuffer` は (物理アドレス, バイト数)。メモリマップの framebuffer 領域に
/// 含まれていない場合に備えて個別にもマップする。
pub fn build_and_activate(
    boot: &BootInfo,
    framebuffer: Option<(u64, u64)>,
    allow_1g: bool,
    use_pat: bool,
) -> Result<BuildReport, BuildError> {
    let kernel = boot.kernel_address.ok_or(BuildError::NoKernelAddress)?;
    if kernel.virtual_base != layout::KERNEL_IMAGE_BASE {
        warn!(
            "kernel virtual base {:#x} differs from the linker script ({:#x})",
            kernel.virtual_base,
            layout::KERNEL_IMAGE_BASE
        );
    }
    let mut report = BuildReport::default();
    let frames_before = super::frame::stats().map_or(0, |s| s.used_frames());

    let mut space = AddressSpace::new_kernel()?;

    // 1. カーネルイメージ (W^X)
    let sections = KernelSections::get();
    let image = [
        (sections.requests, PageFlags::KERNEL_RW),
        (sections.text, PageFlags::KERNEL_RX),
        (sections.rodata, PageFlags::KERNEL_RO),
        (sections.data, PageFlags::KERNEL_RW),
    ];
    for ((start, end), flags) in image {
        let mut v = align_down(start, PAGE_SIZE);
        while v < end {
            let phys = kernel.physical_base + (v - kernel.virtual_base);
            let virt = VirtAddr::new(v).ok_or(MapError::OutOfVirtualSpace)?;
            let pa = PhysAddr::new(phys).ok_or(MapError::OutOfVirtualSpace)?;
            // SAFETY: カーネルイメージ自身の物理ページを、リンク時の仮想アドレスへ
            // セクションごとの権限でマップし直すだけ (Limine のマップと同じ対応関係)。
            unsafe { space.map(virt, pa, PageSize::Size4K, flags)? };
            count(&mut report, PageSize::Size4K);
            v += PAGE_SIZE;
        }
    }

    // 2. HHDM: 隣接する同じ権限の領域をつなげてから、大きいページでマップする
    let off = hhdm::offset();
    let mut run: Option<(u64, u64, PageFlags)> = None;
    let mut hhdm_top = 0u64;
    let runs_flush = |run: (u64, u64, PageFlags),
                      space: &mut AddressSpace,
                      report: &mut BuildReport|
     -> Result<(), MapError> {
        let (start, end, flags) = run;
        map_hhdm_range(space, start, end - start, flags, allow_1g, report)?;
        report.hhdm_bytes += end - start;
        Ok(())
    };
    for r in boot.memory_map.iter() {
        let Some(flags) = hhdm_flags(r.kind) else {
            continue;
        };
        let (start, end) = (align_down(r.base, PAGE_SIZE), align_up(r.end(), PAGE_SIZE));
        hhdm_top = hhdm_top.max(end);
        run = match run {
            Some((s, e, f)) if f == flags && e == start => Some((s, end, f)),
            Some(prev) => {
                runs_flush(prev, &mut space, &mut report)?;
                Some((start, end, flags))
            }
            None => Some((start, end, flags)),
        };
    }
    if let Some(prev) = run {
        runs_flush(prev, &mut space, &mut report)?;
    }
    // framebuffer がメモリマップの framebuffer 領域外にある場合 (仕様上は無いはず)
    if let Some((fb_phys, fb_len)) = framebuffer {
        let mut p = align_down(fb_phys, PAGE_SIZE);
        while p < fb_phys + fb_len {
            let virt = VirtAddr::new(p + off).ok_or(MapError::OutOfVirtualSpace)?;
            if space.translate(virt).is_none() {
                warn!("framebuffer page {p:#x} was not covered by the memory map; mapping it");
                map_hhdm_range(
                    &mut space,
                    p,
                    PAGE_SIZE,
                    PageFlags::FRAMEBUFFER,
                    false,
                    &mut report,
                )?;
            }
            p += PAGE_SIZE;
        }
        hhdm_top = hhdm_top.max(fb_phys + fb_len);
    }
    report.hhdm_top = hhdm_top;
    if off < layout::KERNEL_HALF_START || off.saturating_add(hhdm_top) > layout::HHDM_LIMIT {
        return Err(BuildError::HhdmOutOfRange {
            offset: off,
            top: off.saturating_add(hhdm_top),
        });
    }

    // 3. 切り替え後も必要な領域がマップされているか確認する
    let rsp = current_rsp();
    let checks = [
        ("current stack", rsp),
        ("kernel code", build_and_activate as *const () as u64),
        ("kernel data", (&raw const KERNEL_SPACE) as u64),
    ];
    for (what, addr) in checks {
        let v = VirtAddr::new(addr).ok_or(BuildError::MissingMapping(what, addr))?;
        if space.translate(v).is_none() {
            return Err(BuildError::MissingMapping(what, addr));
        }
    }
    if let Some((fb_phys, _)) = framebuffer
        && space
            .translate(VirtAddr::new_canonical(fb_phys + off))
            .is_none()
    {
        return Err(BuildError::MissingMapping("framebuffer", fb_phys));
    }

    report.table_frames = super::frame::stats().map_or(0, |s| s.used_frames()) - frames_before;

    // 4. PAT を設定して CR3 を切り替える
    if use_pat {
        // SAFETY: 呼び出し元が CPU の PAT 対応を確認している。Limine のページテーブルは
        // framebuffer を含めて PAT index 4 を使っていない (Limine は WB/UC/WC を
        // index 0〜3 / PAT bit で表すが、切り替えまでの間 framebuffer に書かないため、
        // 一時的に種別が変わっても問題にならない)。
        unsafe { paging::program_pat() };
    }
    // SAFETY: 上の確認により、新しいページテーブルは現在のコード・スタック・カーネルの
    // 静的データ・framebuffer をマップしている。HHDM は同じオフセットで作り直しているので
    // ページテーブル用フレームや Limine のスタックへのアクセスも継続できる。
    unsafe { space.activate() };

    *KERNEL_SPACE.lock() = Some(space);
    Ok(report)
}

fn current_rsp() -> u64 {
    let rsp: u64;
    // SAFETY: RSP を読むだけ。
    unsafe {
        core::arch::asm!("mov {}, rsp", out(reg) rsp, options(nomem, nostack, preserves_flags))
    };
    rsp
}

/// カーネル空間に対して `f` を実行する (内部用)。
pub(super) fn with_kernel_space<R>(f: impl FnOnce(&mut AddressSpace) -> R) -> Option<R> {
    KERNEL_SPACE.lock().as_mut().map(f)
}

/// カーネル空間で仮想アドレスを変換する。
pub fn translate(virt: VirtAddr) -> Option<Translation> {
    KERNEL_SPACE.lock().as_ref()?.translate(virt)
}

/// W^X 監査の結果。
#[derive(Debug, Clone, Copy, Default)]
pub struct WxAudit {
    /// 検査した葉の数
    pub leaves: u64,
    /// 書き込み可能かつ実行可能な葉の数
    pub violations: u64,
    /// 最初の違反アドレス
    pub first_violation: Option<u64>,
    /// 実行可能な葉の数
    pub executable: u64,
    /// user ビット付きの葉の数 (カーネル空間では 0 のはず)
    pub user: u64,
}

/// カーネル空間の上位半分すべてについて W^X を監査する。
pub fn audit_wx() -> Option<WxAudit> {
    let guard = KERNEL_SPACE.lock();
    let space = guard.as_ref()?;
    let mut a = WxAudit::default();
    space.for_each_leaf(256..512, |v, _, flags| {
        a.leaves += 1;
        if flags.executable {
            a.executable += 1;
        }
        if flags.user {
            a.user += 1;
        }
        if flags.writable && flags.executable {
            a.violations += 1;
            a.first_violation.get_or_insert(v.as_u64());
        }
    });
    Some(a)
}

/// カーネル空間の PML4 (プロセス用の空間を作るときのコピー元)。
#[allow(dead_code)] // Phase 7 で使用。Phase 2 ではセルフテストで検証
pub fn kernel_pml4() -> Option<super::addr::PhysFrame> {
    KERNEL_SPACE.lock().as_ref().map(AddressSpace::pml4)
}

/// カーネル空間の上位半分の PML4 エントリ (共有の検証用)。
pub fn kernel_pml4_entry(index: usize) -> u64 {
    KERNEL_SPACE
        .lock()
        .as_ref()
        .map_or(0, |k| k.kernel_pml4_entry(index))
}

/// カーネル空間へ切り替える (プロセス空間からカーネル空間へ戻る)。
///
/// # Safety
///
/// 現在の実行に必要なのが上位半分 (カーネル空間) のマップだけであること
/// (下位半分のユーザーページを参照中でないこと)。
pub unsafe fn activate_kernel_space() {
    if let Some(k) = KERNEL_SPACE.lock().as_ref() {
        // SAFETY: 呼び出し元の契約により、下位半分を参照していない。上位半分は全空間で共通。
        unsafe { k.activate() };
    }
}

/// カーネル空間を引数にプロセス用アドレス空間を作る。
#[allow(dead_code)] // Phase 7 で使用。Phase 2 ではセルフテストで検証
pub fn new_user_space() -> Result<AddressSpace, MapError> {
    let guard = KERNEL_SPACE.lock();
    let kernel = guard
        .as_ref()
        .ok_or(MapError::Frame(super::frame::FrameError::NotInitialized))?;
    AddressSpace::new_user(kernel)
}

/// 物理範囲が HHDM でマップされていなければ、読み取り専用 (NX, WB) で 4KiB ずつマップする。
/// ACPI テーブル等、メモリマップ上 reserved にあるファームウェアのデータを読むために使う。
pub(super) fn ensure_hhdm_readonly(phys: u64, len: u64) -> Result<(), MapError> {
    let mut guard = KERNEL_SPACE.lock();
    let space = guard
        .as_mut()
        .ok_or(MapError::Frame(super::frame::FrameError::NotInitialized))?;
    let off = hhdm::offset();
    let mut p = align_down(phys, PAGE_SIZE);
    let end = align_up(
        phys.checked_add(len).ok_or(MapError::OutOfVirtualSpace)?,
        PAGE_SIZE,
    );
    while p < end {
        if off + p >= layout::HHDM_LIMIT {
            return Err(MapError::OutOfVirtualSpace);
        }
        let virt = VirtAddr::new(off + p).ok_or(MapError::OutOfVirtualSpace)?;
        if space.translate(virt).is_none() {
            let pa = PhysAddr::new(p).ok_or(MapError::OutOfVirtualSpace)?;
            // SAFETY: HHDM の、まだマップされていない位置に、対応する物理ページを
            // 読み取り専用で置くだけ (書き込みできないので別名による破壊は起きない)。
            unsafe { space.map(virt, pa, PageSize::Size4K, PageFlags::KERNEL_RO)? };
        }
        p += PAGE_SIZE;
    }
    Ok(())
}

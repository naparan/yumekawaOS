//! Local APIC と I/O APIC。
//!
//! - Local APIC: 各 CPU の割り込み受付 (EOI・タイマ・スプリアス・LINT/エラー)。
//!   xAPIC (MMIO) を基本とし、ファームウェアが既に x2APIC モードにしている場合は
//!   MSR (0x800〜) でアクセスする (x2APIC から xAPIC へは戻せないため)。
//! - I/O APIC: 外部割り込み (ISA IRQ / GSI) を Local APIC のベクタへ配送する。
//!   ACPI MADT の Interrupt Source Override に従って ISA IRQ → GSI・極性・トリガを決める。
//!
//! どちらも ACPI MADT ([`crate::acpi::Madt`]) から得たアドレスを MMIO 領域に UC でマップする。

use core::fmt;
use core::sync::atomic::{AtomicU64, Ordering};

use crate::acpi::Madt;
use crate::arch::x86_64::registers::{self, MSR_APIC_BASE};
use crate::arch::x86_64::trap::TrapFrame;
use crate::interrupts::{self, vectors};
use crate::memory::{PhysAddr, mmio};
use crate::sync::{IrqSpinLock, Once};

// Local APIC レジスタ (xAPIC の MMIO オフセット。x2APIC では MSR 0x800 + offset/16)
const REG_ID: u32 = 0x020;
const REG_VERSION: u32 = 0x030;
const REG_TPR: u32 = 0x080;
const REG_EOI: u32 = 0x0B0;
const REG_SVR: u32 = 0x0F0;
const REG_ISR0: u32 = 0x100;
const REG_ESR: u32 = 0x280;
const REG_LVT_TIMER: u32 = 0x320;
const REG_LVT_LINT0: u32 = 0x350;
const REG_LVT_LINT1: u32 = 0x360;
const REG_LVT_ERROR: u32 = 0x370;
const REG_TIMER_INITIAL: u32 = 0x380;
const REG_TIMER_CURRENT: u32 = 0x390;
const REG_TIMER_DIVIDE: u32 = 0x3E0;

const LVT_MASKED: u32 = 1 << 16;
const LVT_TIMER_PERIODIC: u32 = 1 << 17;
const LVT_DELIVERY_NMI: u32 = 0b100 << 8;
const LVT_LEVEL: u32 = 1 << 15;
const LVT_ACTIVE_LOW: u32 = 1 << 13;
const SVR_ENABLE: u32 = 1 << 8;

const APIC_BASE_BSP: u64 = 1 << 8;
const APIC_BASE_X2APIC: u64 = 1 << 10;
const APIC_BASE_ENABLE: u64 = 1 << 11;

/// タイマの分周比 (divide configuration の値 0b0011 = 16 分周)。
pub const TIMER_DIVIDE_VALUE: u32 = 0b0011;
/// 分周比。
pub const TIMER_DIVISOR: u64 = 16;

/// APIC の初期化エラー。
#[derive(Debug, Clone, Copy)]
pub enum ApicError {
    /// CPU が APIC/MSR をサポートしない
    Unsupported,
    /// MMIO のマップに失敗
    Map(crate::memory::paging::MapError),
    /// I/O APIC が無い
    NoIoApic,
    /// GSI を担当する I/O APIC が無い
    NoIoApicForGsi(u32),
}

impl fmt::Display for ApicError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported => f.write_str("CPU has no local APIC / MSR support"),
            Self::Map(e) => write!(f, "cannot map APIC registers: {e}"),
            Self::NoIoApic => f.write_str("no I/O APIC described in MADT"),
            Self::NoIoApicForGsi(g) => write!(f, "no I/O APIC handles GSI {g}"),
        }
    }
}

enum Access {
    XApic(mmio::MmioRegion),
    X2Apic,
}

/// Local APIC。
pub struct LocalApic {
    access: Access,
    phys_base: u64,
}

impl fmt::Debug for LocalApic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LocalApic")
            .field("phys_base", &self.phys_base)
            .field("x2apic", &matches!(self.access, Access::X2Apic))
            .finish()
    }
}

impl LocalApic {
    fn read(&self, reg: u32) -> u32 {
        match &self.access {
            Access::XApic(m) => m.read_u32(u64::from(reg)),
            // SAFETY: x2APIC モードが有効で、0x800 + reg/16 は x2APIC のレジスタ MSR。
            Access::X2Apic => unsafe { registers::rdmsr(0x800 + (reg >> 4)) as u32 },
        }
    }

    fn write(&self, reg: u32, value: u32) {
        match &self.access {
            Access::XApic(m) => m.write_u32(u64::from(reg), value),
            // SAFETY: x2APIC のレジスタ MSR への書き込み。ここで書くのは EOI / LVT / タイマ /
            // SVR / TPR / ESR だけで、メモリ安全性に影響しない。
            Access::X2Apic => unsafe { registers::wrmsr(0x800 + (reg >> 4), u64::from(value)) },
        }
    }

    /// APIC ID。
    pub fn id(&self) -> u32 {
        match self.access {
            Access::XApic(_) => self.read(REG_ID) >> 24,
            Access::X2Apic => self.read(REG_ID),
        }
    }

    /// バージョンレジスタ (bit0-7: version, bit16-23: max LVT)。
    pub fn version(&self) -> u32 {
        self.read(REG_VERSION)
    }

    /// EOI を送る。
    pub fn eoi(&self) {
        self.write(REG_EOI, 0);
    }

    /// `vector` が In-Service Register に立っているか。
    pub fn in_service(&self, vector: u8) -> bool {
        let reg = REG_ISR0 + u32::from(vector / 32) * 0x10;
        self.read(reg) & (1 << (vector % 32)) != 0
    }

    /// タイマを止めて、マスクした one-shot モードで `initial` から数え始める (校正用)。
    pub fn timer_start_measurement(&self, initial: u32) {
        self.write(REG_TIMER_DIVIDE, TIMER_DIVIDE_VALUE);
        self.write(REG_LVT_TIMER, LVT_MASKED | u32::from(vectors::LAPIC_TIMER));
        self.write(REG_TIMER_INITIAL, initial);
    }

    /// タイマの現在値。
    pub fn timer_current(&self) -> u32 {
        self.read(REG_TIMER_CURRENT)
    }

    /// タイマを止める。
    pub fn timer_stop(&self) {
        self.write(REG_TIMER_INITIAL, 0);
        self.write(REG_LVT_TIMER, LVT_MASKED | u32::from(vectors::LAPIC_TIMER));
    }

    /// 周期モードでタイマを開始する (`initial` カウントごとに `vectors::LAPIC_TIMER`)。
    pub fn timer_start_periodic(&self, initial: u32) {
        self.write(REG_TIMER_DIVIDE, TIMER_DIVIDE_VALUE);
        self.write(
            REG_LVT_TIMER,
            LVT_TIMER_PERIODIC | u32::from(vectors::LAPIC_TIMER),
        );
        self.write(REG_TIMER_INITIAL, initial);
    }
}

static LAPIC: Once<LocalApic> = Once::new();
static LAPIC_ERRORS: AtomicU64 = AtomicU64::new(0);

/// 初期化済みの Local APIC。
pub fn local_apic() -> Option<&'static LocalApic> {
    LAPIC.get()
}

/// 割り込みディスパッチャ用: `vector` が Local APIC で処理中なら EOI を送る。
pub fn eoi_if_in_service(vector: u8) {
    if let Some(lapic) = LAPIC.get()
        && lapic.in_service(vector)
    {
        lapic.eoi();
    }
}

/// Local APIC の初期化結果。
#[derive(Debug, Clone, Copy)]
pub struct LapicInfo {
    /// APIC ID
    pub id: u32,
    /// バージョン
    pub version: u32,
    /// 物理アドレス
    pub phys_base: u64,
    /// x2APIC モード
    pub x2apic: bool,
    /// BSP か
    pub bsp: bool,
}

/// BSP の Local APIC を初期化する (割り込み無効の状態で呼ぶ)。
pub fn init_local(
    madt: &Madt,
    cpu_has_apic: bool,
    cpu_has_msr: bool,
) -> Result<LapicInfo, ApicError> {
    if !cpu_has_apic || !cpu_has_msr {
        return Err(ApicError::Unsupported);
    }
    // SAFETY: CPUID で MSR と APIC のサポートを確認済みなので IA32_APIC_BASE は存在する。
    let mut base_msr = unsafe { registers::rdmsr(MSR_APIC_BASE) };
    let msr_phys = base_msr & 0x000F_FFFF_FFFF_F000;
    if msr_phys != madt.lapic_address {
        warn!(
            "lapic: IA32_APIC_BASE ({msr_phys:#x}) differs from MADT ({:#x}); using the MSR value",
            madt.lapic_address
        );
    }
    if base_msr & APIC_BASE_ENABLE == 0 {
        base_msr |= APIC_BASE_ENABLE;
        // SAFETY: APIC のグローバル有効化ビットを立てるだけ (ベースアドレスは変えない)。
        unsafe { registers::wrmsr(MSR_APIC_BASE, base_msr) };
        info!("lapic: globally enabled via IA32_APIC_BASE");
    }
    let x2apic = base_msr & APIC_BASE_X2APIC != 0;
    let access = if x2apic {
        info!("lapic: firmware left the APIC in x2APIC mode; using MSR access");
        Access::X2Apic
    } else {
        let phys = PhysAddr::new(msr_phys).ok_or(ApicError::Unsupported)?;
        // SAFETY: IA32_APIC_BASE が示す Local APIC のレジスタ (4KiB の MMIO) で、RAM ではない。
        let region = unsafe { mmio::map(phys, 0x1000) }.map_err(ApicError::Map)?;
        Access::XApic(region)
    };
    let lapic = LocalApic {
        access,
        phys_base: msr_phys,
    };

    // 割り込みを受け付ける前に LVT を既知の状態にする
    lapic.write(REG_TPR, 0);
    lapic.timer_stop();
    // LINT0 (8259 の ExtINT) は使わない
    lapic.write(REG_LVT_LINT0, LVT_MASKED);
    // LINT1 は MADT の LAPIC NMI 設定に従う (既定: NMI)
    let mut lint1 = LVT_MASKED;
    for nmi in madt
        .lapic_nmis()
        .iter()
        .filter(|n| n.processor == 0xFF || u32::from(n.processor) == lapic.id())
    {
        let mut v = LVT_DELIVERY_NMI;
        if nmi.flags & 0b11 == 0b11 {
            v |= LVT_ACTIVE_LOW;
        }
        if (nmi.flags >> 2) & 0b11 == 0b11 {
            v |= LVT_LEVEL;
        }
        if nmi.lint == 1 {
            lint1 = v;
        } else if nmi.lint == 0 {
            lapic.write(REG_LVT_LINT0, v);
        }
    }
    lapic.write(REG_LVT_LINT1, lint1);
    lapic.write(REG_LVT_ERROR, u32::from(vectors::LAPIC_ERROR));
    // ESR は書いてから読む (SDM 10.5.3)
    lapic.write(REG_ESR, 0);
    let _ = lapic.read(REG_ESR);
    // 保留中の割り込みがあれば EOI で片付ける
    lapic.eoi();
    // ソフトウェア有効化 + スプリアスベクタ
    lapic.write(REG_SVR, SVR_ENABLE | u32::from(vectors::SPURIOUS));

    let info = LapicInfo {
        id: lapic.id(),
        version: lapic.version(),
        phys_base: msr_phys,
        x2apic,
        bsp: base_msr & APIC_BASE_BSP != 0,
    };
    if LAPIC.set(lapic).is_err() {
        warn!("lapic: already initialized");
    }
    if let Err(e) = interrupts::register(vectors::LAPIC_ERROR, lapic_error_interrupt) {
        warn!("lapic: cannot register the error handler: {e}");
    }
    Ok(info)
}

fn lapic_error_interrupt(_frame: &mut TrapFrame) {
    if let Some(lapic) = LAPIC.get() {
        lapic.write(REG_ESR, 0);
        let esr = lapic.read(REG_ESR);
        let n = LAPIC_ERRORS.fetch_add(1, Ordering::Relaxed);
        if n < 8 {
            warn!("lapic: error interrupt, ESR={esr:#x}");
        }
    }
}

/// 1 つの I/O APIC。
struct IoApic {
    regs: mmio::MmioRegion,
    id: u8,
    gsi_base: u32,
    entries: u32,
}

impl IoApic {
    fn read(&self, reg: u32) -> u32 {
        self.regs.write_u32(0x00, reg);
        self.regs.read_u32(0x10)
    }

    fn write(&self, reg: u32, value: u32) {
        self.regs.write_u32(0x00, reg);
        self.regs.write_u32(0x10, value);
    }

    fn set_redirection(&self, index: u32, low: u32, high: u32) {
        // 書き換え中に中途半端な設定で配送されないよう、先にマスクしてから上位→下位の順に書く
        self.write(0x10 + index * 2, LVT_MASKED);
        self.write(0x10 + index * 2 + 1, high);
        self.write(0x10 + index * 2, low);
    }
}

const MAX_IOAPICS: usize = 8;

struct IoApics {
    list: [Option<IoApic>; MAX_IOAPICS],
}

static IOAPICS: IrqSpinLock<IoApics> = IrqSpinLock::new(IoApics {
    list: [const { None }; MAX_IOAPICS],
});

/// I/O APIC の初期化結果 (1 個分)。
#[derive(Debug, Clone, Copy)]
pub struct IoApicInfo {
    /// ID
    pub id: u8,
    /// 物理アドレス
    pub address: u32,
    /// 最初の GSI
    pub gsi_base: u32,
    /// リダイレクションエントリ数
    pub entries: u32,
    /// バージョン
    pub version: u8,
}

/// MADT にあるすべての I/O APIC をマップし、全エントリをマスクする。
pub fn init_io(madt: &Madt, mut report: impl FnMut(IoApicInfo)) -> Result<usize, ApicError> {
    if madt.ioapics().is_empty() {
        return Err(ApicError::NoIoApic);
    }
    let mut set = IOAPICS.lock();
    let mut count = 0;
    for (slot, entry) in set.list.iter_mut().zip(madt.ioapics()) {
        let phys = PhysAddr::new(u64::from(entry.address)).ok_or(ApicError::NoIoApic)?;
        // SAFETY: MADT が示す I/O APIC のレジスタ (IOREGSEL/IOWIN を含む 0x20 バイト) で、RAM ではない。
        let regs = unsafe { mmio::map(phys, 0x20) }.map_err(ApicError::Map)?;
        let mut io = IoApic {
            regs,
            id: entry.id,
            gsi_base: entry.gsi_base,
            entries: 0,
        };
        let ver = io.read(0x01);
        io.entries = ((ver >> 16) & 0xff) + 1;
        for i in 0..io.entries {
            io.set_redirection(i, LVT_MASKED, 0);
        }
        report(IoApicInfo {
            id: io.id,
            address: entry.address,
            gsi_base: io.gsi_base,
            entries: io.entries,
            version: ver as u8,
        });
        *slot = Some(io);
        count += 1;
    }
    Ok(count)
}

/// ISA IRQ `irq` を、APIC ID `dest` の CPU のベクタ `vector` へ配送するよう設定して有効化する。
/// 戻り値は使った GSI と (アクティブ Low, レベルトリガ)。
pub fn route_isa_irq(
    madt: &Madt,
    irq: u8,
    vector: u8,
    dest: u32,
) -> Result<(u32, bool, bool), ApicError> {
    let (gsi, active_low, level) = madt.isa_irq(irq);
    let set = IOAPICS.lock();
    let io = set
        .list
        .iter()
        .flatten()
        .find(|io| (io.gsi_base..io.gsi_base + io.entries).contains(&gsi))
        .ok_or(ApicError::NoIoApicForGsi(gsi))?;
    let mut low = u32::from(vector); // fixed delivery, physical destination
    if active_low {
        low |= LVT_ACTIVE_LOW;
    }
    if level {
        low |= LVT_LEVEL;
    }
    io.set_redirection(gsi - io.gsi_base, low, (dest & 0xff) << 24);
    Ok((gsi, active_low, level))
}

/// GSI をマスクする (ドライバの停止用)。
#[allow(dead_code)] // Phase 3 以降のドライバで使用
pub fn mask_gsi(gsi: u32) -> Result<(), ApicError> {
    let set = IOAPICS.lock();
    let io = set
        .list
        .iter()
        .flatten()
        .find(|io| (io.gsi_base..io.gsi_base + io.entries).contains(&gsi))
        .ok_or(ApicError::NoIoApicForGsi(gsi))?;
    let index = gsi - io.gsi_base;
    let low = io.read(0x10 + index * 2);
    io.write(0x10 + index * 2, low | LVT_MASKED);
    Ok(())
}

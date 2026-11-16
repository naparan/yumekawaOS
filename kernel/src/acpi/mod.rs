//! ACPI テーブルの最小限の解析 (RSDP → XSDT/RSDT → MADT / FADT / HPET)。
//!
//! - すべてのテーブルでシグネチャ・長さ・チェックサムを検証する。検証に失敗した
//!   テーブルは使わずにエラー/警告を出す。
//! - ACPI テーブルは物理アドレスで与えられ、Limine (base revision 3) の HHDM には
//!   マップされていない。[`crate::memory::map_firmware_readonly`] で読み取り専用にマップしてから読む。
//! - 読み取りは境界チェック付きのバイト列操作だけで行う (構造体へのキャストはしない)。
//! - AML (DSDT の解釈) は行わない。
//!
//! ACPI reclaimable 領域は回収しない (解析結果を後から再参照できるように残す)。

use core::fmt;

use crate::memory;
use crate::sync::Once;

static INFO: Once<AcpiInfo> = Once::new();

/// 解析結果をグローバルに保存する (起動時に 1 度)。後続のドライバ (Phase 3 のマウス等) は
/// [`info`] から MADT (ISA IRQ の割り当て) 等を参照する。
pub fn publish(info: AcpiInfo) -> &'static AcpiInfo {
    match INFO.set(info) {
        Ok(r) => r,
        Err(_) => INFO
            .get()
            .unwrap_or_else(|| unreachable!("ACPI info is being initialized")),
    }
}

/// 保存済みの解析結果。
#[allow(dead_code)] // Phase 3 (マウスの IRQ12 ルーティング) で使用
pub fn info() -> Option<&'static AcpiInfo> {
    INFO.get()
}

/// 解析エラー。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcpiError {
    /// RSDP が与えられていない
    NoRsdp,
    /// シグネチャが違う
    BadSignature {
        /// 期待したシグネチャ
        expected: [u8; 8],
        /// 物理アドレス
        phys: u64,
    },
    /// チェックサム不一致
    BadChecksum {
        /// テーブル名
        table: [u8; 4],
        /// 物理アドレス
        phys: u64,
    },
    /// 長さが不正
    BadLength {
        /// テーブル名
        table: [u8; 4],
        /// 長さ
        length: u32,
    },
    /// マップできない
    Map(memory::paging::MapError),
    /// MADT が無い
    NoMadt,
}

impl fmt::Display for AcpiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoRsdp => f.write_str("no RSDP was provided by the bootloader"),
            Self::BadSignature { expected, phys } => write!(
                f,
                "bad signature at {phys:#x} (expected {:?})",
                core::str::from_utf8(expected).unwrap_or("?")
            ),
            Self::BadChecksum { table, phys } => write!(
                f,
                "checksum mismatch in {} at {phys:#x}",
                core::str::from_utf8(table).unwrap_or("?")
            ),
            Self::BadLength { table, length } => write!(
                f,
                "invalid length {length} in {}",
                core::str::from_utf8(table).unwrap_or("?")
            ),
            Self::Map(e) => write!(f, "cannot map ACPI table: {e}"),
            Self::NoMadt => f.write_str("MADT (APIC) table not found"),
        }
    }
}

impl From<memory::paging::MapError> for AcpiError {
    fn from(e: memory::paging::MapError) -> Self {
        Self::Map(e)
    }
}

/// 境界チェック付きのリトルエンディアン読み出し。
fn le_u8(b: &[u8], off: usize) -> u8 {
    b.get(off).copied().unwrap_or(0)
}
fn le_u16(b: &[u8], off: usize) -> u16 {
    u16::from_le_bytes([le_u8(b, off), le_u8(b, off + 1)])
}
fn le_u32(b: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([
        le_u8(b, off),
        le_u8(b, off + 1),
        le_u8(b, off + 2),
        le_u8(b, off + 3),
    ])
}
fn le_u64(b: &[u8], off: usize) -> u64 {
    u64::from(le_u32(b, off)) | (u64::from(le_u32(b, off + 4)) << 32)
}

fn checksum_ok(b: &[u8]) -> bool {
    b.iter().fold(0u8, |a, &x| a.wrapping_add(x)) == 0
}

/// 物理アドレスのファームウェアデータを読み取り専用で参照する。
fn map(phys: u64, len: usize) -> Result<&'static [u8], AcpiError> {
    // SAFETY: ACPI テーブル (RSDP / SDT) はファームウェアが用意した読み取り専用のデータで、
    // カーネルは ACPI reclaimable 領域を回収しないため、寿命の間変わらず解放されない。
    Ok(unsafe { memory::map_firmware_readonly(phys, len)? })
}

/// SDT (ヘッダ付きテーブル) を検証してマップする。
fn map_sdt(phys: u64) -> Result<&'static [u8], AcpiError> {
    let header = map(phys, 36)?;
    let mut sig = [0u8; 4];
    sig.copy_from_slice(&header[0..4]);
    let length = le_u32(header, 4);
    if !(36..=16 * 1024 * 1024).contains(&length) {
        return Err(AcpiError::BadLength { table: sig, length });
    }
    let table = map(phys, length as usize)?;
    if !checksum_ok(table) {
        return Err(AcpiError::BadChecksum { table: sig, phys });
    }
    Ok(table)
}

/// I/O APIC の情報 (MADT type 1)。
#[derive(Debug, Clone, Copy, Default)]
pub struct IoApicEntry {
    /// I/O APIC ID
    pub id: u8,
    /// MMIO 物理アドレス
    pub address: u32,
    /// 担当する最初の GSI
    pub gsi_base: u32,
}

/// Interrupt Source Override (MADT type 2): ISA IRQ → GSI の対応と極性/トリガ。
#[derive(Debug, Clone, Copy, Default)]
pub struct InterruptOverride {
    /// ISA IRQ 番号
    pub source: u8,
    /// 対応する GSI
    pub gsi: u32,
    /// MPS INTI flags (bit 0-1: 極性, bit 2-3: トリガ)
    pub flags: u16,
}

impl InterruptOverride {
    /// アクティブ Low か (極性 = 3)。
    pub fn active_low(&self) -> bool {
        self.flags & 0b11 == 0b11
    }
    /// レベルトリガか (トリガ = 3)。
    pub fn level_triggered(&self) -> bool {
        (self.flags >> 2) & 0b11 == 0b11
    }
}

/// Local APIC の NMI 入力 (MADT type 4)。
#[derive(Debug, Clone, Copy, Default)]
pub struct LapicNmi {
    /// 対象の ACPI processor UID (0xFF = 全 CPU)
    pub processor: u8,
    /// MPS INTI flags
    pub flags: u16,
    /// LINT 番号 (0 / 1)
    pub lint: u8,
}

const MAX_IOAPICS: usize = 8;
const MAX_OVERRIDES: usize = 24;
const MAX_NMIS: usize = 8;

/// MADT の解析結果。
#[derive(Debug, Clone, Copy)]
pub struct Madt {
    /// Local APIC の物理アドレス (type 5 の 64bit override を反映済み)
    pub lapic_address: u64,
    /// 8259 互換 PIC が存在する (PCAT_COMPAT)
    pub pcat_compat: bool,
    /// 有効な CPU (Local APIC / x2APIC) の数
    pub cpu_count: u32,
    ioapics: [IoApicEntry; MAX_IOAPICS],
    ioapic_count: usize,
    overrides: [InterruptOverride; MAX_OVERRIDES],
    override_count: usize,
    nmis: [LapicNmi; MAX_NMIS],
    nmi_count: usize,
}

impl Madt {
    /// I/O APIC の一覧。
    pub fn ioapics(&self) -> &[IoApicEntry] {
        &self.ioapics[..self.ioapic_count]
    }
    /// Interrupt Source Override の一覧。
    pub fn overrides(&self) -> &[InterruptOverride] {
        &self.overrides[..self.override_count]
    }
    /// LAPIC NMI の一覧。
    pub fn lapic_nmis(&self) -> &[LapicNmi] {
        &self.nmis[..self.nmi_count]
    }

    /// ISA IRQ `irq` に対応する GSI と (アクティブ Low, レベルトリガ)。
    /// override が無ければ GSI = IRQ, アクティブ High, エッジ (ISA の既定)。
    pub fn isa_irq(&self, irq: u8) -> (u32, bool, bool) {
        match self.overrides().iter().find(|o| o.source == irq) {
            Some(o) => (o.gsi, o.active_low(), o.level_triggered()),
            None => (u32::from(irq), false, false),
        }
    }
}

fn parse_madt(t: &[u8]) -> Madt {
    let mut m = Madt {
        lapic_address: u64::from(le_u32(t, 36)),
        pcat_compat: le_u32(t, 40) & 1 != 0,
        cpu_count: 0,
        ioapics: [IoApicEntry::default(); MAX_IOAPICS],
        ioapic_count: 0,
        overrides: [InterruptOverride::default(); MAX_OVERRIDES],
        override_count: 0,
        nmis: [LapicNmi::default(); MAX_NMIS],
        nmi_count: 0,
    };
    let mut off = 44;
    while off + 2 <= t.len() {
        let kind = le_u8(t, off);
        let len = usize::from(le_u8(t, off + 1));
        if len < 2 || off + len > t.len() {
            warn!("MADT: malformed entry at offset {off} (type {kind}, len {len}); stopping");
            break;
        }
        let e = &t[off..off + len];
        match kind {
            0 if len >= 8 => {
                // Processor Local APIC: flags bit0 = enabled, bit1 = online capable
                if le_u32(e, 4) & 0b11 != 0 {
                    m.cpu_count += 1;
                }
            }
            1 if len >= 12 => {
                if m.ioapic_count < MAX_IOAPICS {
                    m.ioapics[m.ioapic_count] = IoApicEntry {
                        id: le_u8(e, 2),
                        address: le_u32(e, 4),
                        gsi_base: le_u32(e, 8),
                    };
                    m.ioapic_count += 1;
                } else {
                    warn!("MADT: more than {MAX_IOAPICS} I/O APICs; ignoring the rest");
                }
            }
            2 if len >= 10 => {
                if m.override_count < MAX_OVERRIDES {
                    m.overrides[m.override_count] = InterruptOverride {
                        source: le_u8(e, 3),
                        gsi: le_u32(e, 4),
                        flags: le_u16(e, 8),
                    };
                    m.override_count += 1;
                } else {
                    warn!("MADT: too many interrupt source overrides; ignoring the rest");
                }
            }
            4 if len >= 6 => {
                if m.nmi_count < MAX_NMIS {
                    m.nmis[m.nmi_count] = LapicNmi {
                        processor: le_u8(e, 2),
                        flags: le_u16(e, 3),
                        lint: le_u8(e, 5),
                    };
                    m.nmi_count += 1;
                }
            }
            5 if len >= 12 => m.lapic_address = le_u64(e, 4),
            9 if len >= 16 => {
                // Processor Local x2APIC
                if le_u32(e, 8) & 0b11 != 0 {
                    m.cpu_count += 1;
                }
            }
            _ => trace!("MADT: entry type {kind} (len {len}) ignored"),
        }
        off += len;
    }
    m
}

/// ACPI PM timer (FADT)。
#[derive(Debug, Clone, Copy)]
pub struct PmTimer {
    /// I/O ポート
    pub port: u16,
    /// 32bit カウンタか (偽なら 24bit)
    pub is_32bit: bool,
}

/// FADT から使う情報。
#[derive(Debug, Clone, Copy)]
pub struct Fadt {
    /// FADT のリビジョン
    pub revision: u8,
    /// ACPI PM timer (I/O 空間にある場合)
    pub pm_timer: Option<PmTimer>,
    /// IAPC_BOOT_ARCH の 8042 ビット (FADT rev >= 2 のときのみ `Some`)
    pub has_8042: Option<bool>,
    /// IAPC_BOOT_ARCH の LEGACY_DEVICES ビット
    pub legacy_devices: Option<bool>,
}

fn parse_fadt(t: &[u8]) -> Fadt {
    let revision = le_u8(t, 8);
    let flags = le_u32(t, 112);
    let tmr_val_ext = flags & (1 << 8) != 0;
    let mut pm_timer = None;
    // X_PM_TMR_BLK (Generic Address Structure, offset 208) を優先
    if t.len() >= 220 {
        let space = le_u8(t, 208);
        let addr = le_u64(t, 212);
        if addr != 0 {
            if space == 1 && addr <= 0xffff {
                pm_timer = Some(PmTimer {
                    port: addr as u16,
                    is_32bit: tmr_val_ext,
                });
            } else {
                warn!(
                    "FADT: X_PM_TMR_BLK is not in I/O space (space {space}, {addr:#x}); not supported"
                );
            }
        }
    }
    if pm_timer.is_none() && t.len() >= 80 {
        let port = le_u32(t, 76);
        if port != 0 && port <= 0xffff {
            pm_timer = Some(PmTimer {
                port: port as u16,
                is_32bit: tmr_val_ext,
            });
        }
    }
    let (has_8042, legacy_devices) = if revision >= 2 && t.len() >= 111 {
        let arch = le_u16(t, 109);
        (Some(arch & 0b10 != 0), Some(arch & 0b01 != 0))
    } else {
        (None, None)
    };
    Fadt {
        revision,
        pm_timer,
        has_8042,
        legacy_devices,
    }
}

/// ACPI の解析結果。
#[derive(Debug, Clone, Copy)]
pub struct AcpiInfo {
    /// ACPI リビジョン (RSDP)
    pub revision: u8,
    /// OEM ID
    pub oem_id: [u8; 6],
    /// MADT
    pub madt: Madt,
    /// FADT (無ければ `None`)
    pub fadt: Option<Fadt>,
    /// HPET の MMIO 物理アドレス (HPET テーブルがあれば)
    pub hpet_address: Option<u64>,
    /// ルートテーブル (XSDT / RSDT) のエントリ数
    pub table_count: usize,
}

impl AcpiInfo {
    /// OEM ID (表示用)。
    pub fn oem(&self) -> &str {
        core::str::from_utf8(&self.oem_id).unwrap_or("?").trim()
    }
}

/// RSDP (物理アドレス) から ACPI テーブルを解析する。
pub fn parse(rsdp_phys: Option<u64>) -> Result<AcpiInfo, AcpiError> {
    let rsdp_phys = rsdp_phys.ok_or(AcpiError::NoRsdp)?;
    let rsdp = map(rsdp_phys, 20)?;
    if &rsdp[0..8] != b"RSD PTR " {
        return Err(AcpiError::BadSignature {
            expected: *b"RSD PTR ",
            phys: rsdp_phys,
        });
    }
    if !checksum_ok(&rsdp[0..20]) {
        return Err(AcpiError::BadChecksum {
            table: *b"RSDP",
            phys: rsdp_phys,
        });
    }
    let revision = le_u8(rsdp, 15);
    let mut oem_id = [0u8; 6];
    oem_id.copy_from_slice(&rsdp[9..15]);

    // ACPI 2.0+ は XSDT (64bit ポインタ) を使う
    let (root_phys, entry_size) = if revision >= 2 {
        let full = map(rsdp_phys, 36)?;
        let length = le_u32(full, 20) as usize;
        let full = map(rsdp_phys, length.clamp(36, 1024))?;
        if !checksum_ok(full) {
            return Err(AcpiError::BadChecksum {
                table: *b"RSDP",
                phys: rsdp_phys,
            });
        }
        let xsdt = le_u64(full, 24);
        if xsdt != 0 {
            (xsdt, 8)
        } else {
            (u64::from(le_u32(rsdp, 16)), 4)
        }
    } else {
        (u64::from(le_u32(rsdp, 16)), 4)
    };

    let root = map_sdt(root_phys)?;
    let expected: &[u8] = if entry_size == 8 { b"XSDT" } else { b"RSDT" };
    if &root[0..4] != expected {
        let mut e = [b' '; 8];
        e[..4].copy_from_slice(expected);
        return Err(AcpiError::BadSignature {
            expected: e,
            phys: root_phys,
        });
    }
    let table_count = (root.len() - 36) / entry_size;
    debug!(
        "revision {revision}, OEM '{}', {} at {root_phys:#x} with {table_count} tables",
        core::str::from_utf8(&oem_id).unwrap_or("?").trim(),
        if entry_size == 8 { "XSDT" } else { "RSDT" }
    );

    let mut madt = None;
    let mut fadt = None;
    let mut hpet_address = None;
    for i in 0..table_count {
        let off = 36 + i * entry_size;
        let phys = if entry_size == 8 {
            le_u64(root, off)
        } else {
            u64::from(le_u32(root, off))
        };
        if phys == 0 {
            continue;
        }
        let table = match map_sdt(phys) {
            Ok(t) => t,
            Err(e) => {
                warn!("skipping table #{i} at {phys:#x}: {e}");
                continue;
            }
        };
        let sig = &table[0..4];
        debug!(
            "  {} at {phys:#x} (len {}, rev {})",
            core::str::from_utf8(sig).unwrap_or("????"),
            table.len(),
            le_u8(table, 8)
        );
        match sig {
            b"APIC" if table.len() >= 44 => madt = Some(parse_madt(table)),
            b"FACP" => fadt = Some(parse_fadt(table)),
            b"HPET" if table.len() >= 56 => {
                // Event Timer Block: GAS at offset 40, address at 44
                hpet_address = Some(le_u64(table, 44));
            }
            _ => {}
        }
    }
    let madt = madt.ok_or(AcpiError::NoMadt)?;
    Ok(AcpiInfo {
        revision,
        oem_id,
        madt,
        fadt,
        hpet_address,
        table_count,
    })
}

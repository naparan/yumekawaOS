//! Limine boot protocol のリクエスト定義と応答の変換。
//!
//! リクエストは `.limine_requests*` セクションに置き、linker.ld で
//! 開始/終了マーカーの間に並べる。Limine はカーネル起動前にこれらを走査して
//! 応答ポインタを書き込む。

use core::ptr::NonNull;
use core::sync::atomic::{AtomicBool, Ordering};

use limine::BaseRevision;
use limine::firmware_type::FirmwareType;
use limine::memory_map::EntryType;
use limine::request::{
    BootloaderInfoRequest, ExecutableAddressRequest, ExecutableCmdlineRequest, FirmwareTypeRequest,
    FramebufferRequest, HhdmRequest, MemoryMapRequest, RequestsEndMarker, RequestsStartMarker,
    RsdpRequest, StackSizeRequest,
};

use super::{
    BootError, BootInfo, BootString, BootloaderInfo, ChannelMask, FirmwareKind, FramebufferInfo,
    KernelAddress, MEMORY_MAP_CAPACITY, MemoryKind, MemoryMap, MemoryRegion,
};

/// 要求する Limine base revision。
/// limine クレート 0.5.0 が対応する最新は 3 (Limine 11.x は 0〜6 をサポート)。
const REQUESTED_BASE_REVISION: u64 = 3;

/// カーネルスタックサイズ (Limine 既定の 64KiB では debug ビルドで不足し得る)。
const KERNEL_STACK_SIZE: u64 = 256 * 1024;

#[used]
#[unsafe(link_section = ".limine_requests_start")]
static REQUESTS_START: RequestsStartMarker = RequestsStartMarker::new();

#[used]
#[unsafe(link_section = ".limine_requests")]
static BASE_REVISION: BaseRevision = BaseRevision::with_revision(REQUESTED_BASE_REVISION);

#[used]
#[unsafe(link_section = ".limine_requests")]
static STACK_SIZE_REQUEST: StackSizeRequest = StackSizeRequest::new().with_size(KERNEL_STACK_SIZE);

#[used]
#[unsafe(link_section = ".limine_requests")]
static BOOTLOADER_INFO_REQUEST: BootloaderInfoRequest = BootloaderInfoRequest::new();

#[used]
#[unsafe(link_section = ".limine_requests")]
static FIRMWARE_TYPE_REQUEST: FirmwareTypeRequest = FirmwareTypeRequest::new();

#[used]
#[unsafe(link_section = ".limine_requests")]
static HHDM_REQUEST: HhdmRequest = HhdmRequest::new();

#[used]
#[unsafe(link_section = ".limine_requests")]
static MEMORY_MAP_REQUEST: MemoryMapRequest = MemoryMapRequest::new();

#[used]
#[unsafe(link_section = ".limine_requests")]
static FRAMEBUFFER_REQUEST: FramebufferRequest = FramebufferRequest::new();

#[used]
#[unsafe(link_section = ".limine_requests")]
static EXECUTABLE_ADDRESS_REQUEST: ExecutableAddressRequest = ExecutableAddressRequest::new();

#[used]
#[unsafe(link_section = ".limine_requests")]
static EXECUTABLE_CMDLINE_REQUEST: ExecutableCmdlineRequest = ExecutableCmdlineRequest::new();

#[used]
#[unsafe(link_section = ".limine_requests")]
static RSDP_REQUEST: RsdpRequest = RsdpRequest::new();

#[used]
#[unsafe(link_section = ".limine_requests_end")]
static REQUESTS_END: RequestsEndMarker = RequestsEndMarker::new();

static TAKEN: AtomicBool = AtomicBool::new(false);

/// Limine の応答を検証して [`BootInfo`] に変換する。2 回目以降は
/// [`BootError::AlreadyTaken`] を返す。
pub fn take() -> Result<BootInfo, BootError> {
    if TAKEN.swap(true, Ordering::AcqRel) {
        return Err(BootError::AlreadyTaken);
    }

    if !BASE_REVISION.is_supported() {
        return Err(BootError::UnsupportedBaseRevision {
            requested: REQUESTED_BASE_REVISION,
            loaded: BASE_REVISION.loaded_revision(),
        });
    }
    trace!(
        "limine base revision {REQUESTED_BASE_REVISION} accepted (loaded={:?})",
        BASE_REVISION.loaded_revision()
    );
    trace!(
        "limine responses: stack_size={} bootloader_info={} firmware_type={} hhdm={} memmap={} \
         framebuffer={} executable_address={} cmdline={} rsdp={}",
        STACK_SIZE_REQUEST.get_response().is_some(),
        BOOTLOADER_INFO_REQUEST.get_response().is_some(),
        FIRMWARE_TYPE_REQUEST.get_response().is_some(),
        HHDM_REQUEST.get_response().is_some(),
        MEMORY_MAP_REQUEST.get_response().is_some(),
        FRAMEBUFFER_REQUEST.get_response().is_some(),
        EXECUTABLE_ADDRESS_REQUEST.get_response().is_some(),
        EXECUTABLE_CMDLINE_REQUEST.get_response().is_some(),
        RSDP_REQUEST.get_response().is_some(),
    );
    if STACK_SIZE_REQUEST.get_response().is_none() {
        warn!("bootloader did not acknowledge the {KERNEL_STACK_SIZE}-byte stack size request");
    }

    let hhdm_offset = HHDM_REQUEST
        .get_response()
        .ok_or(BootError::MissingResponse("HHDM"))?
        .offset();

    let entries = MEMORY_MAP_REQUEST
        .get_response()
        .ok_or(BootError::MissingResponse("memory map"))?
        .entries();
    if entries.len() > MEMORY_MAP_CAPACITY {
        return Err(BootError::MemoryMapTooLarge {
            entries: entries.len(),
            capacity: MEMORY_MAP_CAPACITY,
        });
    }
    let mut memory_map = MemoryMap {
        entries: [MemoryRegion {
            base: 0,
            length: 0,
            kind: MemoryKind::Unknown,
        }; MEMORY_MAP_CAPACITY],
        len: entries.len(),
    };
    for (slot, e) in memory_map.entries.iter_mut().zip(entries) {
        *slot = MemoryRegion {
            base: e.base,
            length: e.length,
            kind: memory_kind(e.entry_type),
        };
    }

    let bootloader = BOOTLOADER_INFO_REQUEST
        .get_response()
        .map(|r| BootloaderInfo {
            name: BootString::copy_from(r.name()),
            version: BootString::copy_from(r.version()),
        });

    let firmware = match FIRMWARE_TYPE_REQUEST
        .get_response()
        .map(|r| r.firmware_type())
    {
        Some(FirmwareType::X86_BIOS) => FirmwareKind::Bios,
        Some(FirmwareType::UEFI_32) => FirmwareKind::Uefi32,
        Some(FirmwareType::UEFI_64) => FirmwareKind::Uefi64,
        _ => FirmwareKind::Unknown,
    };

    let kernel_address = EXECUTABLE_ADDRESS_REQUEST
        .get_response()
        .map(|r| KernelAddress {
            physical_base: r.physical_base(),
            virtual_base: r.virtual_base(),
        });

    let rsdp_address = RSDP_REQUEST.get_response().map(|r| r.address() as u64);

    let cmdline = BootString::copy_from(
        EXECUTABLE_CMDLINE_REQUEST
            .get_response()
            .and_then(|r| r.cmdline().to_str().ok())
            .unwrap_or(""),
    );

    let (framebuffer, framebuffer_count) = match FRAMEBUFFER_REQUEST.get_response() {
        Some(response) => {
            let count = response.framebuffers().count();
            let first = response.framebuffers().next().and_then(|fb| {
                let address = NonNull::new(fb.addr())?;
                // SAFETY: アドレスと寸法は Limine が framebuffer 応答として返した値であり、
                // Limine は framebuffer を HHDM 経由で書き込み可能にマップしている。
                // `TAKEN` によりこの関数は 1 度しか先へ進まないため、
                // FramebufferInfo (= 書き込み権) はカーネル内で一意となる。
                Some(unsafe {
                    FramebufferInfo::from_bootloader(
                        address,
                        fb.width(),
                        fb.height(),
                        fb.pitch(),
                        fb.bpp(),
                        memory_model_raw(fb.memory_model()),
                        ChannelMask {
                            size: fb.red_mask_size(),
                            shift: fb.red_mask_shift(),
                        },
                        ChannelMask {
                            size: fb.green_mask_size(),
                            shift: fb.green_mask_shift(),
                        },
                        ChannelMask {
                            size: fb.blue_mask_size(),
                            shift: fb.blue_mask_shift(),
                        },
                    )
                })
            });
            (first, count)
        }
        None => (None, 0),
    };

    Ok(BootInfo {
        bootloader,
        firmware,
        hhdm_offset,
        memory_map,
        framebuffer,
        framebuffer_count,
        kernel_address,
        rsdp_address,
        cmdline,
    })
}

fn memory_model_raw(model: limine::framebuffer::MemoryModel) -> u8 {
    if model == limine::framebuffer::MemoryModel::RGB {
        1
    } else {
        0
    }
}

/// Limine のメモリ種別をカーネルの型へ変換する。
fn memory_kind(entry_type: EntryType) -> MemoryKind {
    match entry_type {
        EntryType::USABLE => MemoryKind::Usable,
        EntryType::RESERVED => MemoryKind::Reserved,
        EntryType::ACPI_RECLAIMABLE => MemoryKind::AcpiReclaimable,
        EntryType::ACPI_NVS => MemoryKind::AcpiNvs,
        EntryType::BAD_MEMORY => MemoryKind::BadMemory,
        EntryType::BOOTLOADER_RECLAIMABLE => MemoryKind::BootloaderReclaimable,
        EntryType::EXECUTABLE_AND_MODULES => MemoryKind::KernelAndModules,
        EntryType::FRAMEBUFFER => MemoryKind::Framebuffer,
        _ => MemoryKind::Unknown,
    }
}

impl FramebufferInfo {
    /// # Safety
    ///
    /// `address` から `pitch * height` バイトが書き込み可能にマップされた
    /// framebuffer メモリであり、この値の生存中に他から書き込まれないこと。
    #[allow(clippy::too_many_arguments)]
    unsafe fn from_bootloader(
        address: NonNull<u8>,
        width: u64,
        height: u64,
        pitch: u64,
        bpp: u16,
        memory_model: u8,
        red: ChannelMask,
        green: ChannelMask,
        blue: ChannelMask,
    ) -> Self {
        Self {
            address,
            width,
            height,
            pitch,
            bpp,
            memory_model,
            red,
            green,
            blue,
        }
    }
}

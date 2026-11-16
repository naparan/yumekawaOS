//! yumekawaOS カーネル (Phase 1: Boot)。
//!
//! Limine (UEFI) から higher-half (0xffffffff80000000) にロードされ、`kmain` から実行される。
//! Phase 1 で行うこと:
//!
//! 1. COM1 シリアルへのカーネルログを初期化する
//! 2. Limine のブート情報 (framebuffer / メモリマップ / HHDM 等) を受け取り検証する
//! 3. framebuffer にスプラッシュ画面を描く
//! 4. 起動完了マーカー [`BOOT_COMPLETE_MARKER`] をログに出し、`hlt` で停止する

#![no_std]
#![no_main]

// ログマクロ (`info!` 等) を他モジュールより先に定義する
#[macro_use]
mod log;

mod arch;
mod boot;
mod drivers;
mod fmtbuf;
mod graphics;
mod panic;
mod sync;

use core::fmt::Write;

use crate::arch::x86_64::cpu::{self, CpuIdentity};
use crate::boot::{BootInfo, FirmwareKind, MemoryKind};
use crate::fmtbuf::StackString;
use crate::graphics::Framebuffer;
use crate::graphics::splash::{self, SplashText};

/// 起動完了時にシリアルへ出力するマーカー。scripts/test-boot.sh がこれを待つ。
pub const BOOT_COMPLETE_MARKER: &str = "YUMEKAWAOS_BOOT_OK";

/// 現在の開発フェーズ。
const PHASE: u32 = 1;

const MIB: u64 = 1024 * 1024;

/// カーネルのエントリポイント (linker.ld の `ENTRY(kmain)`)。
///
/// Limine により 64bit Long Mode・割り込み無効・Limine が用意したスタックで呼ばれる。
#[unsafe(no_mangle)]
extern "C" fn kmain() -> ! {
    let serial_result = log::init();

    info!(
        "yumekawaOS kernel v{} (phase {PHASE}: boot)",
        env!("CARGO_PKG_VERSION")
    );
    info!(
        "build profile: {}, log level >= {}",
        if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        },
        log::min_level().label().trim()
    );
    if let Err(e) = serial_result {
        // ここには出力されないが、エラーを握りつぶさず記録しておく(画面表示は継続)
        error!("serial console unavailable: {e}");
    }

    let boot_info = match boot::take() {
        Ok(info) => info,
        Err(e) => halt_with_fatal(format_args!("failed to read boot information: {e}")),
    };
    apply_cmdline(boot_info.cmdline);
    log_boot_info(&boot_info);

    let cpu = CpuIdentity::read();
    info!(
        "cpu: vendor={} brand={}",
        cpu.vendor(),
        cpu.brand().unwrap_or("<unavailable>")
    );
    debug!("interrupts enabled: {}", cpu::interrupts_enabled());

    let usable_bytes = boot_info.memory_map.total_bytes(MemoryKind::Usable);

    let Some(fb_info) = boot_info.framebuffer else {
        halt_with_fatal(format_args!(
            "no framebuffer was provided by the bootloader"
        ));
    };
    let mut fb = match Framebuffer::new(fb_info) {
        Ok(fb) => fb,
        Err(e) => halt_with_fatal(format_args!("framebuffer is not usable: {e}")),
    };
    let format = fb.format();
    info!(
        "framebuffer: {}x{} pitch={} bytes, 32 bpp, format={:?}",
        fb.width(),
        fb.height(),
        fb.pitch(),
        format
    );

    let mut footer = StackString::<160>::new();
    let _ = write!(
        footer,
        "{}x{} 32bpp | {} MiB usable | {} {} | {}",
        fb.width(),
        fb.height(),
        usable_bytes / MIB,
        boot_info.bootloader.map_or("unknown", |b| b.name),
        boot_info.bootloader.map_or("", |b| b.version),
        firmware_label(boot_info.firmware),
    );
    if footer.is_truncated() {
        warn!("splash footer text was truncated");
    }
    let mut status = StackString::<64>::new();
    let _ = write!(status, "phase {PHASE} : boot ok");

    debug!("drawing splash screen");
    splash::draw(
        &mut fb,
        &SplashText {
            status: status.as_str(),
            footer: footer.as_str(),
        },
    );
    info!("splash screen drawn ({})", splash::TITLE);

    info!("{BOOT_COMPLETE_MARKER} phase={PHASE}");
    info!("nothing more to do in phase {PHASE}; halting the CPU");
    arch::halt_forever()
}

/// FATAL ログを出して停止する。
fn halt_with_fatal(args: core::fmt::Arguments<'_>) -> ! {
    fatal!("{args}");
    fatal!("system halted.");
    arch::halt_forever()
}

/// カーネルコマンドライン(limine.conf の `cmdline:`)を解釈する。
///
/// 対応オプション:
/// - `loglevel=trace|debug|info|warn|error|fatal` — ログの最小レベル
/// - `selftest=panic` — パニックハンドラの動作確認用に意図的にパニックする
///   (scripts/test-boot.sh の panic ケースで使用)
fn apply_cmdline(cmdline: &str) {
    for option in cmdline.split_ascii_whitespace() {
        match option.split_once('=') {
            Some(("selftest", "panic")) => {
                warn!("selftest=panic: triggering an intentional kernel panic");
                panic!("selftest: intentional panic requested by kernel cmdline");
            }
            Some(("loglevel", value)) => match parse_level(value) {
                Some(level) => {
                    log::set_min_level(level);
                    info!("log level set to {} by cmdline", level.label().trim());
                }
                None => warn!("unknown log level in cmdline: {value}"),
            },
            _ => warn!("unknown cmdline option ignored: {option}"),
        }
    }
}

fn parse_level(s: &str) -> Option<log::Level> {
    use log::Level;
    let level = match s {
        _ if s.eq_ignore_ascii_case("trace") => Level::Trace,
        _ if s.eq_ignore_ascii_case("debug") => Level::Debug,
        _ if s.eq_ignore_ascii_case("info") => Level::Info,
        _ if s.eq_ignore_ascii_case("warn") => Level::Warn,
        _ if s.eq_ignore_ascii_case("error") => Level::Error,
        _ if s.eq_ignore_ascii_case("fatal") => Level::Fatal,
        _ => return None,
    };
    Some(level)
}

fn firmware_label(kind: FirmwareKind) -> &'static str {
    match kind {
        FirmwareKind::Bios => "BIOS",
        FirmwareKind::Uefi32 => "UEFI (32-bit)",
        FirmwareKind::Uefi64 => "UEFI (x86_64)",
        FirmwareKind::Unknown => "unknown firmware",
    }
}

fn log_boot_info(info: &BootInfo) {
    match info.bootloader {
        Some(b) => info!("bootloader: {} {}", b.name, b.version),
        None => warn!("bootloader did not report its name/version"),
    }
    info!("firmware: {}", firmware_label(info.firmware));
    if info.firmware != FirmwareKind::Uefi64 {
        warn!(
            "yumekawaOS targets 64-bit UEFI; running on {}",
            firmware_label(info.firmware)
        );
    }
    info!("hhdm offset: {:#018x}", info.hhdm_offset);
    match info.kernel_address {
        Some(k) => info!(
            "kernel: physical base {:#x}, virtual base {:#x}",
            k.physical_base, k.virtual_base
        ),
        None => warn!("bootloader did not report the kernel address"),
    }
    match info.rsdp_address {
        Some(addr) => debug!("acpi rsdp: {addr:#x}"),
        None => warn!("no ACPI RSDP was provided"),
    }
    if !info.cmdline.is_empty() {
        info!("cmdline: {}", info.cmdline);
    }

    if info.memory_map.is_empty() {
        warn!("memory map is empty");
    }
    info!("memory map: {} regions", info.memory_map.len());
    for region in info.memory_map.iter() {
        debug!(
            "  [{:#014x} - {:#014x}) {:>10} KiB  {}",
            region.base,
            region.end(),
            region.length / 1024,
            region.kind.name()
        );
    }
    info!(
        "memory: usable {} MiB, bootloader-reclaimable {} MiB, kernel+modules {} KiB",
        info.memory_map.total_bytes(MemoryKind::Usable) / MIB,
        info.memory_map
            .total_bytes(MemoryKind::BootloaderReclaimable)
            / MIB,
        info.memory_map.total_bytes(MemoryKind::KernelAndModules) / 1024,
    );
    debug!("framebuffers reported: {}", info.framebuffer_count);
}

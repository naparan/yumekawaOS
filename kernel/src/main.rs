//! yumekawaOS カーネル (Phase 2: Memory / Interrupt)。
//!
//! Limine (UEFI) から higher-half (0xffffffff80000000) にロードされ、`kmain` から実行される。
//!
//! 初期化順序 (docs/architecture.md §1):
//!
//! ```text
//! kmain (Limine のスタック・Limine のページテーブル)
//!   1. 早期シリアル (ロガー)
//!   2. ブート情報 (メモリマップ・文字列をカーネル内へコピー) / cmdline
//!   3. GDT + TSS (IST) / IDT          ← 以降、例外はカーネルのハンドラで処理される
//!   4. CPU 保護機能 (WP / NXE / PGE / SMEP / SMAP / UMIP)
//!   5. 物理フレームアロケータ (usable のみ)
//!   6. カーネルのページテーブルを構築して CR3 切り替え (W^X・HHDM・framebuffer WC)
//!   7. framebuffer を display に登録 (以降パニック画面が出せる)
//!   8. ガードページ付きのメインスタック・IST スタックを確保し、メインスタックへ移る
//! kernel_main (カーネルのスタック)
//!   9. bootloader-reclaimable 領域を回収
//!  10. カーネルヒープ
//!  11. ACPI (RSDP → XSDT → MADT / FADT) → 8259 を無効化 → Local APIC → I/O APIC
//!  12. Local APIC タイマの校正
//!  13. PS/2 キーボード (i8042) → IRQ1 を I/O APIC で有効化
//!  14. スプラッシュ画面 → タイマ開始 → 割り込み許可 (sti)
//!  15. (cmdline の selftest=...) → 起動完了マーカー → アイドルループ (sti; hlt)
//! ```

#![no_std]
#![no_main]

extern crate alloc;

// ログマクロ (`info!` 等) を他モジュールより先に定義する
#[macro_use]
mod log;

mod acpi;
mod arch;
mod boot;
mod drivers;
mod fmtbuf;
mod graphics;
mod interrupts;
mod memory;
mod panic;
mod selftest;
mod sync;
mod time;

use core::fmt::Write;
use core::mem::ManuallyDrop;

use crate::arch::x86_64::cpu::{self, CpuFeatures, CpuIdentity};
use crate::arch::x86_64::{ProtectionFeatures, gdt, idt};
use crate::boot::{BootInfo, FirmwareKind, MemoryKind};
use crate::drivers::{apic, pic, ps2};
use crate::fmtbuf::StackString;
use crate::graphics::splash::{self, SplashText};
use crate::graphics::{Framebuffer, display};
use crate::interrupts::vectors;
use crate::memory::{frame, heap, kernel_space, paging, stack};

/// 起動完了時にシリアルへ出力するマーカー。scripts/test-boot.sh がこれを待つ。
pub const BOOT_COMPLETE_MARKER: &str = "YUMEKAWAOS_BOOT_OK";

/// 現在の開発フェーズ。
const PHASE: u32 = 2;

const MIB: u64 = 1024 * 1024;

/// メインのカーネルスタックの大きさ。
const MAIN_STACK_SIZE: u64 = 256 * 1024;
/// IST スタック (Double Fault / NMI / Machine Check) の大きさ。
/// パニック画面の描画もこの上で行うので余裕を持たせる。
const IST_STACK_SIZE: u64 = 64 * 1024;

/// アイドルループのハートビートの間隔。
const HEARTBEAT_INTERVAL_MS: u64 = 10_000;

/// cmdline から読み取った起動オプション。
#[derive(Debug, Clone, Copy, Default)]
struct BootOptions {
    selftests: selftest::Selection,
}

/// kmain (Limine のスタック) から kernel_main (カーネルのスタック) へ渡す状態。
struct Handoff {
    boot: BootInfo,
    options: BootOptions,
    features: CpuFeatures,
    protection: ProtectionFeatures,
}

/// カーネルのエントリポイント (linker.ld の `ENTRY(kmain)`)。
///
/// Limine により 64bit Long Mode・割り込み無効・Limine が用意したスタックで呼ばれる。
#[unsafe(no_mangle)]
extern "C" fn kmain() -> ! {
    // 1. 早期シリアル
    let serial_result = log::init();
    info!(
        "yumekawaOS kernel v{} (phase {PHASE}: memory / interrupt)",
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
        // シリアルには出ないが、リングバッファ (パニック画面) には残る
        error!("serial console unavailable: {e}");
    }

    // 2. ブート情報
    let boot_info = match boot::take() {
        Ok(info) => info,
        Err(e) => halt_with_fatal(format_args!("failed to read boot information: {e}")),
    };
    let options = apply_cmdline(boot_info.cmdline.as_str());
    log_boot_info(&boot_info);

    let identity = CpuIdentity::read();
    let features = CpuFeatures::detect();
    info!(
        "cpu: vendor={} brand={} signature={:#x}",
        identity.vendor(),
        identity.brand().unwrap_or("<unavailable>"),
        features.signature
    );
    info!(
        "cpu features: apic={} x2apic={} tsc={} invariant_tsc={} pat={} pge={} nx={} 1g_pages={} smep={} smap={} umip={} phys_bits={} virt_bits={}",
        features.apic,
        features.x2apic,
        features.tsc,
        features.invariant_tsc,
        features.pat,
        features.pge,
        features.nx,
        features.page1gb,
        features.smep,
        features.smap,
        features.umip,
        features.phys_addr_bits,
        features.virt_addr_bits
    );

    // 3. GDT / TSS / IDT
    gdt::init();
    idt::init();
    info!(
        "gdt: kernel cs={:#x} ds={:#x}, tss={:#x} (IST: double fault / NMI / machine check); idt: 256 vectors loaded",
        gdt::KERNEL_CODE_SELECTOR,
        gdt::KERNEL_DATA_SELECTOR,
        gdt::TSS_SELECTOR
    );
    debug!(
        "exception table: {} entries",
        arch::x86_64::probe::exception_table_len()
    );

    // 4. CPU の保護機能
    let protection = arch::x86_64::enable_protection(&features);
    info!(
        "cpu protection: CR0.WP={} EFER.NXE={} CR4.PGE={} SMEP={} SMAP={} UMIP={}",
        protection.write_protect,
        protection.nx,
        protection.global_pages,
        protection.smep,
        protection.smap,
        protection.umip
    );
    if !protection.nx {
        warn!("NX is not available: non-executable mappings cannot be enforced");
    }

    let handoff = Handoff {
        boot: boot_info,
        options,
        features,
        protection,
    };
    early_memory_and_switch(handoff)
}

/// 5〜8: メモリ管理を初期化し、カーネルのスタックへ移る。
fn early_memory_and_switch(mut handoff: Handoff) -> ! {
    let boot = &mut handoff.boot;

    // framebuffer の検証 (ページテーブル切り替え前に物理アドレスを求める)
    let Some(fb_info) = boot.framebuffer.take() else {
        halt_with_fatal(format_args!(
            "no framebuffer was provided by the bootloader"
        ));
    };
    let fb_virt = fb_info.address().as_ptr() as u64;
    let fb_len = fb_info.pitch() * fb_info.height();
    let fb = match Framebuffer::new(fb_info) {
        Ok(fb) => fb,
        Err(e) => halt_with_fatal(format_args!("framebuffer is not usable: {e}")),
    };
    let fb_phys = fb_virt.checked_sub(boot.hhdm_offset);
    if fb_phys.is_none() {
        warn!("framebuffer address {fb_virt:#x} is not inside the HHDM");
    }

    // 5. 物理フレームアロケータ
    memory::hhdm::set_offset(boot.hhdm_offset);
    match frame::init(&boot.memory_map) {
        Ok(s) => info!(
            "frames: {} allocatable ({} MiB) in {} regions, bitmap {} KiB; bootloader-reclaimable deferred",
            s.total_frames,
            s.total_frames * 4096 / MIB,
            s.regions,
            s.bitmap_frames * 4
        ),
        Err(e) => halt_with_fatal(format_args!("frame allocator: {e}")),
    }

    // 6. カーネルのページテーブル
    let use_pat = handoff.features.pat && handoff.features.msr;
    paging::configure(
        handoff.protection.nx,
        use_pat,
        handoff.protection.global_pages,
    );
    match kernel_space::build_and_activate(
        boot,
        fb_phys.map(|p| (p, fb_len)),
        handoff.features.page1gb,
        use_pat,
    ) {
        Ok(r) => info!(
            "paging: switched to the kernel page tables (pml4={:#x}); pages 4K={} 2M={} 1G={}, HHDM {} MiB up to {:#x}, {} table frames, PAT={}",
            paging::active_pml4().as_u64(),
            r.pages_4k,
            r.pages_2m,
            r.pages_1g,
            r.hhdm_bytes / MIB,
            r.hhdm_top,
            r.table_frames,
            if use_pat {
                "WB/WT/UC-/UC/WC/WP/UC-/UC"
            } else {
                "unavailable (WC -> UC)"
            }
        ),
        Err(e) => halt_with_fatal(format_args!("cannot build the kernel page tables: {e}")),
    }
    let sections = kernel_space::KernelSections::get();
    debug!(
        "kernel sections: text {:#x}-{:#x} (RX), rodata {:#x}-{:#x} (R), data/bss {:#x}-{:#x} (RW)",
        sections.text.0,
        sections.text.1,
        sections.rodata.0,
        sections.rodata.1,
        sections.data.0,
        sections.data.1
    );

    // 7. framebuffer を display に登録 (以降、パニック時に画面へ表示できる)
    info!(
        "framebuffer: {}x{} pitch={} bytes, 32 bpp, format={:?}",
        fb.width(),
        fb.height(),
        fb.pitch(),
        fb.format()
    );
    display::install(fb);

    // 8. スタック
    for slot in gdt::IstSlot::ALL {
        match stack::allocate(slot.name(), IST_STACK_SIZE) {
            Ok(s) => {
                cpu::without_interrupts(|| gdt::set_ist(slot, s.top()));
                debug!(
                    "stack '{}': {:#x}-{:#x} (guard from {:#x})",
                    slot.name(),
                    s.info().bottom,
                    s.top(),
                    s.info().guard_start
                );
            }
            Err(e) => halt_with_fatal(format_args!(
                "cannot allocate the {} stack: {e}",
                slot.name()
            )),
        }
    }
    let main_stack = match stack::allocate("kernel-main", MAIN_STACK_SIZE) {
        Ok(s) => s,
        Err(e) => halt_with_fatal(format_args!("cannot allocate the kernel stack: {e}")),
    };
    info!(
        "stacks: main {} KiB at {:#x}-{:#x}, IST {} KiB x{}, each with an unmapped guard below; switching stacks",
        MAIN_STACK_SIZE / 1024,
        main_stack.info().bottom,
        main_stack.top(),
        IST_STACK_SIZE / 1024,
        gdt::IstSlot::ALL.len()
    );

    let handoff = ManuallyDrop::new(handoff);
    let arg = (&raw const handoff) as usize;
    // SAFETY: main_stack.top() はマップ済みのスタック (16 バイト境界) の上端。kernel_entry は
    // 戻らない関数で、引数 `arg` が指す Handoff を最初にコピーする。コピーが終わるまで
    // このフレーム (Limine のスタック) は有効なまま (回収は kernel_main の中で行う)。
    unsafe { switch_stack(main_stack.top(), kernel_entry, arg) }
}

/// スタックを切り替えて `entry(arg)` を呼ぶ。
///
/// # Safety
///
/// `top` はマップ済みで他に使われていないスタックの上端 (16 バイト境界) であること。
unsafe fn switch_stack(top: u64, entry: extern "C" fn(usize) -> !, arg: usize) -> ! {
    // SAFETY: 呼び出し元の契約により `top` は有効なスタック。rbp を 0 にしてスタック
    // トレースをここで終端させる。entry は戻らない (戻った場合は ud2 で #UD → パニック)。
    unsafe {
        core::arch::asm!(
            "mov rsp, {top}",
            "xor ebp, ebp",
            "call {entry}",
            "ud2",
            top = in(reg) top,
            entry = in(reg) entry,
            in("rdi") arg,
            options(noreturn),
        )
    }
}

extern "C" fn kernel_entry(arg: usize) -> ! {
    // SAFETY: arg は early_memory_and_switch の ManuallyDrop<Handoff> を指し、
    // そのフレームは戻らない (switch_stack は noreturn) ので値はまだ有効。
    // ここで所有権を移し、以降は元の位置を参照しない。
    let handoff = unsafe { core::ptr::read(arg as *const Handoff) };
    kernel_main(handoff)
}

/// 9〜15: カーネルのスタック上での初期化。
fn kernel_main(h: Handoff) -> ! {
    // 9. bootloader-reclaimable の回収
    // SAFETY: スタックはカーネルのもの、CR3 はカーネルのページテーブル、GDT/IDT もカーネルの
    // もので、ブート情報 (メモリマップ・文字列) は take() でカーネル内にコピー済み。
    // Limine の応答構造体はこれ以降参照しない。
    match unsafe { frame::reclaim(&h.boot.memory_map) } {
        Ok(n) => info!(
            "frames: reclaimed {} bootloader-reclaimable frames ({} MiB)",
            n,
            n * 4096 / MIB
        ),
        Err(e) => error!("frames: failed to reclaim bootloader memory: {e}"),
    }
    if let Some(s) = frame::stats() {
        info!(
            "frames: {} / {} free ({} MiB free)",
            s.free_frames,
            s.total_frames,
            s.free_frames * 4096 / MIB
        );
    }

    // 10. ヒープ
    if let Err(e) = heap::init() {
        panic!("cannot initialize the kernel heap: {e}");
    }
    {
        let mut v = alloc::vec::Vec::with_capacity(4);
        v.extend_from_slice(&[1u32, 2, 3]);
        let s = heap::stats();
        info!(
            "heap: {} KiB mapped at {:#x} (max {} GiB), first allocation ok (used {} bytes)",
            s.mapped / 1024,
            memory::layout::KERNEL_HEAP_START,
            memory::layout::KERNEL_HEAP_MAX / 1024 / MIB,
            s.used
        );
    }

    // 11. ACPI / 割り込みコントローラ
    let acpi = match acpi::parse(h.boot.rsdp_address) {
        Ok(a) => acpi::publish(a),
        Err(e) => panic!("ACPI: {e} (the I/O APIC cannot be located without the MADT)"),
    };
    let madt = &acpi.madt;
    info!(
        "acpi: revision {}, OEM '{}', {} tables; MADT: lapic {:#x}, {} cpu(s), {} I/O APIC(s), {} override(s), 8259 present={}; FADT: rev {}, pm timer {}, 8042 {}, legacy devices {}; HPET {}",
        acpi.revision,
        acpi.oem(),
        acpi.table_count,
        madt.lapic_address,
        madt.cpu_count,
        madt.ioapics().len(),
        madt.overrides().len(),
        madt.pcat_compat,
        acpi.fadt.map_or(0, |f| f.revision),
        match acpi.fadt.and_then(|f| f.pm_timer) {
            Some(p) => {
                let mut s = StackString::<32>::new();
                let _ = write!(
                    s,
                    "port {:#x} ({}-bit)",
                    p.port,
                    if p.is_32bit { 32 } else { 24 }
                );
                s
            }
            None => {
                let mut s = StackString::<32>::new();
                let _ = write!(s, "none");
                s
            }
        },
        match acpi.fadt.and_then(|f| f.has_8042) {
            Some(true) => "yes",
            Some(false) => "no",
            None => "unknown",
        },
        match acpi.fadt.and_then(|f| f.legacy_devices) {
            Some(true) => "yes",
            Some(false) => "no",
            None => "unknown",
        },
        if acpi.hpet_address.is_some() {
            "present"
        } else {
            "absent"
        }
    );
    for o in madt.overrides() {
        debug!(
            "acpi: ISA IRQ {} -> GSI {} (active {}, {}-triggered)",
            o.source,
            o.gsi,
            if o.active_low() { "low" } else { "high" },
            if o.level_triggered() { "level" } else { "edge" }
        );
    }

    pic::remap_and_mask_all();
    info!(
        "8259 PIC: remapped to {:#x}/{:#x} and fully masked",
        vectors::PIC_MASTER_BASE,
        vectors::PIC_SLAVE_BASE
    );
    let lapic = match apic::init_local(madt, h.features.apic, h.features.msr) {
        Ok(l) => l,
        Err(e) => panic!("local APIC: {e}"),
    };
    info!(
        "lapic: id={} version={:#x} base={:#x} mode={} bsp={}; spurious vector {:#x}, error vector {:#x}",
        lapic.id,
        lapic.version & 0xff,
        lapic.phys_base,
        if lapic.x2apic {
            "x2APIC"
        } else {
            "xAPIC (MMIO, UC)"
        },
        lapic.bsp,
        vectors::SPURIOUS,
        vectors::LAPIC_ERROR
    );
    match apic::init_io(madt, |io| {
        info!(
            "ioapic: id={} at {:#x}, version {:#x}, GSI {}-{} (all masked)",
            io.id,
            io.address,
            io.version,
            io.gsi_base,
            io.gsi_base + io.entries - 1
        );
    }) {
        Ok(_) => {}
        Err(e) => panic!("I/O APIC: {e}"),
    }

    // 12. タイマの校正
    let calibration = match time::calibrate(Some(acpi), h.features.tsc) {
        Ok(c) => c,
        Err(e) => panic!("timer calibration failed: {e}"),
    };
    time::set_tsc_hz(calibration.tsc_hz);
    info!(
        "timer: calibrated with {}: local APIC timer {} Hz (bus / {}), samples {:?} Hz, spread {} ppm; TSC {} MHz; {} Hz tick = {} counts",
        calibration.reference,
        calibration.lapic_hz,
        apic::TIMER_DIVISOR,
        calibration.samples,
        calibration.spread_ppm,
        calibration.tsc_hz / 1_000_000,
        time::TIMER_HZ,
        calibration.counts_per_tick
    );

    // 13. キーボード
    let keyboard_ok = match ps2::init(acpi.fadt.and_then(|f| f.has_8042)) {
        Ok(info) => {
            info!(
                "ps2: i8042 initialized (config {:#04x}, dual channel={}, port 2 ok={}); keyboard reset ok, scan code set {} (translated)",
                info.config, info.dual_channel, info.port2_ok, info.scancode_set
            );
            if let Err(e) =
                interrupts::register(vectors::KEYBOARD, ps2::keyboard::interrupt_handler)
            {
                error!("keyboard: cannot register IRQ1 handler: {e}");
                false
            } else {
                match apic::route_isa_irq(madt, 1, vectors::KEYBOARD, lapic.id) {
                    Ok((gsi, low, level)) => {
                        info!(
                            "keyboard: IRQ1 -> GSI {gsi} -> vector {:#x} (active {}, {}) on APIC {}",
                            vectors::KEYBOARD,
                            if low { "low" } else { "high" },
                            if level { "level" } else { "edge" },
                            lapic.id
                        );
                        true
                    }
                    Err(e) => {
                        error!("keyboard: cannot route IRQ1: {e}");
                        false
                    }
                }
            }
        }
        Err(e) => {
            warn!("ps2: keyboard unavailable: {e}");
            false
        }
    };

    // 14. 画面 → タイマ開始 → 割り込み許可
    draw_splash(&h.boot, keyboard_ok);
    if let Err(e) = time::start(calibration) {
        panic!("cannot start the timer: {e}");
    }
    cpu::enable_interrupts();
    info!("interrupts enabled (IF={})", cpu::interrupts_enabled());
    // tick が実際に進むことを確認する (進まなければタイマ割り込みが届いていない)
    let mut deadline = time::Deadline::after(core::time::Duration::from_millis(500));
    while time::ticks() < 5 {
        if deadline.expired() {
            panic!(
                "timer interrupts are not arriving (ticks={} after 500 ms)",
                time::ticks()
            );
        }
        cpu::enable_interrupts_and_halt();
    }
    info!(
        "timer: running, ticks={} uptime={} us",
        time::ticks(),
        time::uptime_ns() / 1000
    );

    if let Some(a) = kernel_space::audit_wx() {
        info!(
            "paging: W^X audit of the kernel half: {} leaves, {} executable, {} writable+executable, {} user",
            a.leaves, a.executable, a.violations, a.user
        );
        if a.violations != 0 {
            error!(
                "paging: W^X violation at {:#x}",
                a.first_violation.unwrap_or(0)
            );
        }
    }

    // 15. セルフテスト → 起動完了
    if h.options.selftests.panic {
        warn!("selftest=panic: triggering an intentional kernel panic");
        panic!("selftest: intentional panic requested by kernel cmdline");
    }
    selftest::run(h.options.selftests, &h.features, &h.protection);

    info!("{BOOT_COMPLETE_MARKER} phase={PHASE}");
    info!("entering the idle loop (sti; hlt)");
    idle_loop()
}

fn draw_splash(boot: &BootInfo, keyboard_ok: bool) {
    let usable_bytes = boot.memory_map.total_bytes(MemoryKind::Usable)
        + boot
            .memory_map
            .total_bytes(MemoryKind::BootloaderReclaimable);
    let drawn = display::with(|fb| {
        let mut footer = StackString::<160>::new();
        let _ = write!(
            footer,
            "{}x{} 32bpp | {} MiB RAM | {} {} | {}",
            fb.width(),
            fb.height(),
            usable_bytes / MIB,
            boot.bootloader
                .as_ref()
                .map_or("unknown", |b| b.name.as_str()),
            boot.bootloader.as_ref().map_or("", |b| b.version.as_str()),
            firmware_label(boot.firmware),
        );
        if footer.is_truncated() {
            warn!("splash footer text was truncated");
        }
        let mut status = StackString::<64>::new();
        let _ = write!(
            status,
            "phase {PHASE} : memory + interrupts ok{}",
            if keyboard_ok { "" } else { " (no keyboard)" }
        );
        splash::draw(
            fb,
            &SplashText {
                status: status.as_str(),
                footer: footer.as_str(),
            },
        );
    });
    match drawn {
        Some(()) => info!("splash screen drawn ({})", splash::TITLE),
        None => warn!("no display available for the splash screen"),
    }
}

/// アイドルループ: 受信したスキャンコードをログに出し、定期的にハートビートを出す。
fn idle_loop() -> ! {
    let mut next_heartbeat = time::uptime_ms() + HEARTBEAT_INTERVAL_MS;
    let mut extended = false;
    loop {
        // 仕事が無いことを割り込み無効で確認してから `sti; hlt` する (取りこぼし防止)
        cpu::disable_interrupts();
        let heartbeat_due = time::uptime_ms() >= next_heartbeat;
        let scancode = ps2::keyboard::pop_scancode();
        if scancode.is_none() && !heartbeat_due {
            cpu::enable_interrupts_and_halt();
            continue;
        }
        cpu::enable_interrupts();

        if let Some(code) = scancode {
            let kind = if code == 0xE0 || code == 0xE1 {
                "prefix"
            } else if code & 0x80 != 0 {
                "break"
            } else {
                "make"
            };
            info!(
                "keyboard: scancode {code:#04x} ({kind}{}) [set 1]",
                if extended && code != 0xE0 {
                    ", extended"
                } else {
                    ""
                }
            );
            extended = code == 0xE0;
        }
        if heartbeat_due {
            next_heartbeat += HEARTBEAT_INTERVAL_MS;
            let k = ps2::keyboard::stats();
            let i = interrupts::stats();
            let f = frame::stats().unwrap_or_default();
            let hs = heap::stats();
            info!(
                "heartbeat: uptime={} ms ticks={} timer_irqs={} kbd_bytes={} kbd_dropped={} kbd_aux={} spurious(lapic/pic)={}/{} unhandled={} frames_free={} heap_used={}",
                time::uptime().as_millis(),
                time::ticks(),
                interrupts::count(vectors::LAPIC_TIMER),
                k.received,
                k.dropped,
                k.aux_dropped,
                i.spurious_lapic,
                i.spurious_pic,
                i.unhandled,
                f.free_frames,
                hs.used
            );
        }
    }
}

/// FATAL ログを出して停止する (画面が使えるようになる前の致命的エラー用)。
fn halt_with_fatal(args: core::fmt::Arguments<'_>) -> ! {
    fatal!("{args}");
    fatal!("system halted.");
    arch::halt_forever()
}

/// カーネルコマンドライン(limine.conf の `cmdline:`)を解釈する。
///
/// 対応オプション:
/// - `loglevel=trace|debug|info|warn|error|fatal` — ログの最小レベル
/// - `selftest=<名前>[,<名前>...]` — 起動時のセルフテスト (scripts/test-boot.sh が使う)。
///   名前は `memory` / `timer` / `exceptions` / `stackoverflow` / `panic`。
fn apply_cmdline(cmdline: &str) -> BootOptions {
    let mut options = BootOptions::default();
    for option in cmdline.split_ascii_whitespace() {
        match option.split_once('=') {
            Some(("selftest", list)) => {
                for name in list.split(',') {
                    if !options.selftests.enable(name) {
                        warn!("unknown selftest '{name}' ignored");
                    }
                }
                info!("selftests requested: {list}");
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
    options
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
        Some(addr) => debug!("acpi rsdp (physical): {addr:#x}"),
        None => warn!("no ACPI RSDP was provided"),
    }
    if !info.cmdline.is_empty() {
        info!("cmdline: {}", info.cmdline);
    }
    if info.cmdline.is_truncated() {
        warn!("cmdline was truncated to {} bytes", boot::CMDLINE_CAPACITY);
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

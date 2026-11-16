//! IDT (Interrupt Descriptor Table)。
//!
//! 256 ベクタすべてに [`super::trap`] のスタブを登録する (未登録ベクタも共通処理で
//! ログを出して扱えるようにするため)。すべて Interrupt Gate (ハンドラ中は IF=0)、DPL=0。
//! Double Fault / NMI / Machine Check は専用の IST スタックで実行する。

use core::arch::asm;
use core::cell::UnsafeCell;
use core::mem::size_of;

use super::gdt::{self, IstSlot};
use super::trap;

/// 例外ベクタ番号 (SDM Table 6-1 の一覧。未使用のものも表として残す)。
#[allow(dead_code)]
pub mod vector {
    /// #DE Divide Error
    pub const DIVIDE_ERROR: u8 = 0;
    /// #DB Debug
    pub const DEBUG: u8 = 1;
    /// NMI
    pub const NMI: u8 = 2;
    /// #BP Breakpoint
    pub const BREAKPOINT: u8 = 3;
    /// #OF Overflow
    pub const OVERFLOW: u8 = 4;
    /// #BR Bound Range Exceeded
    pub const BOUND_RANGE: u8 = 5;
    /// #UD Invalid Opcode
    pub const INVALID_OPCODE: u8 = 6;
    /// #NM Device Not Available
    pub const DEVICE_NOT_AVAILABLE: u8 = 7;
    /// #DF Double Fault
    pub const DOUBLE_FAULT: u8 = 8;
    /// #TS Invalid TSS
    pub const INVALID_TSS: u8 = 10;
    /// #NP Segment Not Present
    pub const SEGMENT_NOT_PRESENT: u8 = 11;
    /// #SS Stack-Segment Fault
    pub const STACK_SEGMENT_FAULT: u8 = 12;
    /// #GP General Protection
    pub const GENERAL_PROTECTION: u8 = 13;
    /// #PF Page Fault
    pub const PAGE_FAULT: u8 = 14;
    /// #MF x87 Floating-Point
    pub const X87_FLOATING_POINT: u8 = 16;
    /// #AC Alignment Check
    pub const ALIGNMENT_CHECK: u8 = 17;
    /// #MC Machine Check
    pub const MACHINE_CHECK: u8 = 18;
    /// #XM SIMD Floating-Point
    pub const SIMD_FLOATING_POINT: u8 = 19;
    /// #VE Virtualization
    pub const VIRTUALIZATION: u8 = 20;
    /// #CP Control Protection
    pub const CONTROL_PROTECTION: u8 = 21;
    /// #HV Hypervisor Injection
    pub const HYPERVISOR_INJECTION: u8 = 28;
    /// #VC VMM Communication
    pub const VMM_COMMUNICATION: u8 = 29;
    /// #SX Security
    pub const SECURITY: u8 = 30;
}

/// 例外ベクタ (0〜31) の名前。
pub const fn exception_name(v: u8) -> &'static str {
    match v {
        0 => "#DE divide error",
        1 => "#DB debug",
        2 => "NMI",
        3 => "#BP breakpoint",
        4 => "#OF overflow",
        5 => "#BR bound range exceeded",
        6 => "#UD invalid opcode",
        7 => "#NM device not available",
        8 => "#DF double fault",
        9 => "coprocessor segment overrun",
        10 => "#TS invalid TSS",
        11 => "#NP segment not present",
        12 => "#SS stack-segment fault",
        13 => "#GP general protection fault",
        14 => "#PF page fault",
        16 => "#MF x87 floating-point",
        17 => "#AC alignment check",
        18 => "#MC machine check",
        19 => "#XM SIMD floating-point",
        20 => "#VE virtualization",
        21 => "#CP control protection",
        28 => "#HV hypervisor injection",
        29 => "#VC VMM communication",
        30 => "#SX security",
        15 | 22..=27 | 31 => "reserved exception",
        _ => "interrupt",
    }
}

#[derive(Clone, Copy)]
#[repr(C)]
struct GateDescriptor {
    offset_low: u16,
    selector: u16,
    ist: u8,
    type_attr: u8,
    offset_mid: u16,
    offset_high: u32,
    reserved: u32,
}

const _: () = assert!(size_of::<GateDescriptor>() == 16);

impl GateDescriptor {
    const MISSING: Self = Self {
        offset_low: 0,
        selector: 0,
        ist: 0,
        type_attr: 0,
        offset_mid: 0,
        offset_high: 0,
        reserved: 0,
    };

    /// Present, DPL0, 64bit Interrupt Gate (type 0xE)。
    fn interrupt_gate(handler: u64, ist: u8) -> Self {
        Self {
            offset_low: handler as u16,
            selector: gdt::KERNEL_CODE_SELECTOR,
            ist: ist & 0x7,
            type_attr: 0x8E,
            offset_mid: (handler >> 16) as u16,
            offset_high: (handler >> 32) as u32,
            reserved: 0,
        }
    }
}

struct Idt(UnsafeCell<[GateDescriptor; 256]>);

// SAFETY: 書き換えは `init` (割り込み無効・単一 CPU・lidt 前) だけで行う。
unsafe impl Sync for Idt {}

static IDT: Idt = Idt(UnsafeCell::new([GateDescriptor::MISSING; 256]));

#[repr(C, packed)]
struct DescriptorTablePointer {
    limit: u16,
    base: u64,
}

/// IDT を構築してロードする。GDT ([`gdt::init`]) の後、割り込み無効の状態で 1 度だけ呼ぶ。
pub fn init() {
    let mut table = [GateDescriptor::MISSING; 256];
    for (v, gate) in table.iter_mut().enumerate() {
        let v = v as u8;
        let ist = match v {
            vector::DOUBLE_FAULT => IstSlot::DoubleFault as u8,
            vector::NMI => IstSlot::Nmi as u8,
            vector::MACHINE_CHECK => IstSlot::MachineCheck as u8,
            _ => 0,
        };
        *gate = GateDescriptor::interrupt_gate(trap::vector_entry(v), ist);
    }
    let idt = IDT.0.get();
    // SAFETY: IDT は静的領域で、この関数は lidt の前に単一 CPU・割り込み無効で 1 度だけ
    // 呼ばれるため、書き込みは排他的。
    unsafe { idt.write(table) };

    let pointer = DescriptorTablePointer {
        limit: (size_of::<[GateDescriptor; 256]>() - 1) as u16,
        base: idt as u64,
    };
    // SAFETY: `pointer` は全ベクタが有効なスタブを指す静的な IDT を指す。
    unsafe {
        asm!("lidt [{}]", in(reg) &raw const pointer, options(readonly, nostack, preserves_flags))
    };
}

//! GDT (Global Descriptor Table) と TSS (Task State Segment)。
//!
//! Limine が用意した GDT は bootloader-reclaimable 領域にあるため、カーネル自身の GDT へ
//! 切り替える。セレクタの配置は将来の `syscall`/`sysret` (Phase 7) の制約に合わせる:
//!
//! | セレクタ | 内容 | 備考 |
//! | --- | --- | --- |
//! | `0x00` | null | |
//! | `0x08` | kernel code (64bit, DPL0) | `STAR[47:32]` = 0x08 → syscall で CS=0x08, SS=0x10 |
//! | `0x10` | kernel data (DPL0) | |
//! | `0x18` | user code (32bit compat, DPL3) | `STAR[63:48]` = 0x18 の基準。sysret(32bit) 用 (未使用) |
//! | `0x20` | user data (DPL3) | sysret で SS = 0x18+8 → `0x23` |
//! | `0x28` | user code (64bit, DPL3) | sysret で CS = 0x18+16 → `0x2B` |
//! | `0x30` | TSS (16 バイト, 2 エントリ分) | |
//!
//! TSS の IST (Interrupt Stack Table) には Double Fault / NMI / Machine Check 専用の
//! スタックを登録する。起動直後は `.bss` 上の静的スタックを使い、メモリ管理の初期化後に
//! ガードページ付きのスタックへ差し替える ([`set_ist`])。

use core::arch::asm;
use core::cell::UnsafeCell;
use core::mem::{offset_of, size_of};

/// カーネルコードセグメントのセレクタ。
pub const KERNEL_CODE_SELECTOR: u16 = 0x08;
/// カーネルデータセグメントのセレクタ。
pub const KERNEL_DATA_SELECTOR: u16 = 0x10;
/// ユーザー (32bit) コードの基準セレクタ (`STAR[63:48]` に設定する値)。
#[allow(dead_code)] // Phase 7 (syscall/sysret) で使用
pub const USER_BASE_SELECTOR: u16 = 0x18;
/// ユーザーデータセグメントのセレクタ (RPL=3)。
#[allow(dead_code)] // Phase 7 で使用
pub const USER_DATA_SELECTOR: u16 = 0x20 | 3;
/// ユーザー 64bit コードセグメントのセレクタ (RPL=3)。
#[allow(dead_code)] // Phase 7 で使用
pub const USER_CODE_SELECTOR: u16 = 0x28 | 3;
/// TSS のセレクタ。
pub const TSS_SELECTOR: u16 = 0x30;

/// IST の用途 (1 始まりの IST 番号)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum IstSlot {
    /// Double Fault (#DF)。スタックオーバーフロー時もここで確実に処理できるようにする
    DoubleFault = 1,
    /// NMI
    Nmi = 2,
    /// Machine Check (#MC)
    MachineCheck = 3,
}

impl IstSlot {
    /// すべての IST スロット。
    pub const ALL: [Self; 3] = [Self::DoubleFault, Self::Nmi, Self::MachineCheck];

    /// 表示名。
    pub const fn name(self) -> &'static str {
        match self {
            Self::DoubleFault => "ist-double-fault",
            Self::Nmi => "ist-nmi",
            Self::MachineCheck => "ist-machine-check",
        }
    }
}

/// 64bit TSS (Intel SDM Vol.3 7.7)。
#[repr(C, packed(4))]
struct TaskStateSegment {
    reserved0: u32,
    /// 特権レベル 0〜2 へ遷移するときのスタック (Phase 7 で RSP0 を設定する)
    privilege_stack: [u64; 3],
    reserved1: u64,
    /// IST1〜IST7
    interrupt_stack: [u64; 7],
    reserved2: u64,
    reserved3: u16,
    iomap_base: u16,
}

const TSS_SIZE: usize = size_of::<TaskStateSegment>();
const _: () = assert!(TSS_SIZE == 104);

/// 起動直後に使う静的な IST スタックの大きさ。
const EARLY_IST_STACK_SIZE: usize = 16 * 1024;

#[repr(C, align(16))]
struct EarlyStack([u8; EARLY_IST_STACK_SIZE]);

/// 初期化時にだけ書き換え、以降は CPU が参照する静的データ。
struct CpuTable<T>(UnsafeCell<T>);

// SAFETY: 書き換えは `init` / `set_ist` だけで行い、どちらも割り込み無効・単一 CPU
// (BSP) で呼ぶ契約にしている。Rust 側から共有参照を作ることはない。
unsafe impl<T> Sync for CpuTable<T> {}

static TSS: CpuTable<TaskStateSegment> = CpuTable(UnsafeCell::new(TaskStateSegment {
    reserved0: 0,
    privilege_stack: [0; 3],
    reserved1: 0,
    interrupt_stack: [0; 7],
    reserved2: 0,
    reserved3: 0,
    // I/O 許可ビットマップ無し (TSS の limit より後ろを指す)
    iomap_base: TSS_SIZE as u16,
}));

static EARLY_IST_STACKS: CpuTable<[EarlyStack; 3]> = CpuTable(UnsafeCell::new(
    [const { EarlyStack([0; EARLY_IST_STACK_SIZE]) }; 3],
));

static GDT: CpuTable<[u64; 8]> = CpuTable(UnsafeCell::new([
    0,                     // 0x00 null
    0x00AF_9A00_0000_FFFF, // 0x08 kernel code: P, DPL0, S, exec/read, L=1
    0x00CF_9200_0000_FFFF, // 0x10 kernel data: P, DPL0, S, read/write
    0x00CF_FA00_0000_FFFF, // 0x18 user code32: P, DPL3, exec/read, D=1
    0x00CF_F200_0000_FFFF, // 0x20 user data: P, DPL3, read/write
    0x00AF_FA00_0000_FFFF, // 0x28 user code64: P, DPL3, exec/read, L=1
    0,                     // 0x30 TSS (下位) — init で設定
    0,                     // 0x38 TSS (上位)
]));

#[repr(C, packed)]
struct DescriptorTablePointer {
    limit: u16,
    base: u64,
}

/// GDT と TSS を設定してロードし、セグメントレジスタを再読み込みする。
///
/// 割り込み無効の状態で 1 度だけ呼ぶこと (kmain の初期化順序で保証)。
pub fn init() {
    let tss_addr = TSS.0.get() as u64;
    let early_stacks = EARLY_IST_STACKS.0.get() as u64;
    for slot in IstSlot::ALL {
        let index = slot as u64 - 1;
        let top = early_stacks + (index + 1) * EARLY_IST_STACK_SIZE as u64;
        set_ist(slot, top);
    }

    let limit = (TSS_SIZE - 1) as u64;
    // 64bit TSS ディスクリプタ (Available 64-bit TSS = type 0x9, P=1)
    let low = (limit & 0xFFFF)
        | ((tss_addr & 0xFF_FFFF) << 16)
        | (0x89 << 40)
        | (((limit >> 16) & 0xF) << 48)
        | (((tss_addr >> 24) & 0xFF) << 56);
    let high = tss_addr >> 32;
    let gdt = GDT.0.get();
    // SAFETY: GDT は静的領域で、この関数は割り込み無効・単一 CPU で 1 度だけ呼ばれるため
    // 書き込みは排他的。まだ lgdt していないので CPU もこのエントリを参照していない。
    unsafe { (*gdt)[6] = low };
    // SAFETY: 同上。
    unsafe { (*gdt)[7] = high };

    let pointer = DescriptorTablePointer {
        limit: (size_of::<[u64; 8]>() - 1) as u16,
        base: gdt as u64,
    };
    // SAFETY: `pointer` は有効な GDT (静的領域・プログラム終了まで有効) を指す。
    // far return で CS をカーネルコード (0x08) に再読み込みし、データセグメントを 0x10、
    // FS/GS を null にする (64bit モードではベースは MSR 側で扱うため問題ない)。
    // 最後に TSS (0x30) を ltr でロードする。どれも同じ GDT 内の正しいディスクリプタを指す。
    unsafe {
        asm!(
            "lgdt [{ptr}]",
            "push {cs}",
            "lea {tmp}, [rip + 2f]",
            "push {tmp}",
            "retfq",
            "2:",
            "mov {tmp:x}, {ds}",
            "mov ds, {tmp:x}",
            "mov es, {tmp:x}",
            "mov ss, {tmp:x}",
            "xor {tmp:e}, {tmp:e}",
            "mov fs, {tmp:x}",
            "mov gs, {tmp:x}",
            "mov {tmp:x}, {tss}",
            "ltr {tmp:x}",
            ptr = in(reg) &raw const pointer,
            cs = const KERNEL_CODE_SELECTOR as u64,
            ds = const KERNEL_DATA_SELECTOR,
            tss = const TSS_SELECTOR,
            tmp = out(reg) _,
            options(preserves_flags),
        );
    }
}

/// IST スタックの先頭 (スタックトップ) を設定する。
///
/// メモリ管理の初期化後、ガードページ付きスタックへ差し替えるのに使う。
/// `top` は 16 バイト境界で、そのスタックは今後解放されないこと
/// (CPU が例外時に直接使うため)。割り込み無効の状態で呼ぶこと。
pub fn set_ist(slot: IstSlot, top: u64) {
    assert!(
        top.is_multiple_of(16),
        "IST stack top must be 16-byte aligned"
    );
    let field = ist_field(slot);
    // SAFETY: `field` は静的な TSS 内の IST エントリを指す。packed 構造体のフィールドなので
    // 非整列の可能性を考慮し write_unaligned で書く。単一 CPU・割り込み無効で呼ばれるため、
    // CPU が IST を読む例外配送と同時に書き換わることは (NMI/MCE を除き) 無く、
    // u64 の書き込み 1 回なので NMI が来ても古い値か新しい値のどちらかを使うだけである。
    unsafe { field.write_unaligned(top) };
}

/// TSS 内の IST エントリへのポインタ。
fn ist_field(slot: IstSlot) -> *mut u64 {
    let index = slot as usize - 1;
    TSS.0
        .get()
        .cast::<u8>()
        .wrapping_add(offset_of!(TaskStateSegment, interrupt_stack) + index * size_of::<u64>())
        .cast::<u64>()
}

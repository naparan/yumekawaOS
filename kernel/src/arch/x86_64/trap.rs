//! 割り込み・例外の入口 (アセンブリ) と、保存されたレジスタ ([`TrapFrame`])。
//!
//! 流れ:
//!
//! ```text
//! CPU: (必要なら IST に切り替え) SS, RSP, RFLAGS, CS, RIP, [エラーコード] を積む
//!  └→ yk_interrupt_vector_N (build.rs が生成): [0 を積む] + ベクタ番号を積む
//!      └→ yk_interrupt_common: 汎用レジスタ 15 個を積み、RSP (= &mut TrapFrame) を引数に
//!          └→ yk_interrupt_dispatch (Rust, crate::interrupts::dispatch)
//!      ←─ レジスタを復元し、ベクタ番号とエラーコードを捨てて iretq
//! ```
//!
//! - 64bit モードでは CPU が例外フレームを積む前に RSP を 16 バイト境界に揃える。
//!   フレーム (176 バイト = 16 の倍数) を積んだ後も 16 バイト境界なので、そのまま `call` できる。
//! - カーネルは soft-float (SSE 無効) でビルドされるため、SSE/x87 レジスタは保存しない。
//!   ユーザー空間 (Phase 7) で FPU を使わせる場合は XSAVE 等の追加が必要。
//! - `swapgs` は Ring 3 からの割り込みが無い Phase 2 では不要。Phase 7 で追加する。
//! - IDT のゲートはすべて Interrupt Gate (ハンドラ実行中は IF=0)。

use core::arch::global_asm;
use core::fmt;

/// 割り込み/例外発生時に保存されたレジスタ。`yk_interrupt_common` が積む順序と一致させる。
#[derive(Clone, Copy, Default)]
#[repr(C)]
pub struct TrapFrame {
    /// r15
    pub r15: u64,
    /// r14
    pub r14: u64,
    /// r13
    pub r13: u64,
    /// r12
    pub r12: u64,
    /// r11
    pub r11: u64,
    /// r10
    pub r10: u64,
    /// r9
    pub r9: u64,
    /// r8
    pub r8: u64,
    /// rbp
    pub rbp: u64,
    /// rdi
    pub rdi: u64,
    /// rsi
    pub rsi: u64,
    /// rdx
    pub rdx: u64,
    /// rcx
    pub rcx: u64,
    /// rbx
    pub rbx: u64,
    /// rax
    pub rax: u64,
    /// ベクタ番号 (スタブが積む)
    pub vector: u64,
    /// エラーコード (CPU が積まない例外では 0)
    pub error_code: u64,
    /// 割り込まれた命令 (例外の種類によっては次の命令)
    pub rip: u64,
    /// CS
    pub cs: u64,
    /// RFLAGS
    pub rflags: u64,
    /// RSP
    pub rsp: u64,
    /// SS
    pub ss: u64,
}

const _: () = assert!(size_of::<TrapFrame>() == 22 * 8);
const _: () = assert!(size_of::<TrapFrame>().is_multiple_of(16));

impl TrapFrame {
    /// 割り込まれたコードの特権レベル (CS の RPL)。
    pub fn privilege_level(&self) -> u8 {
        (self.cs & 3) as u8
    }
}

impl fmt::Debug for TrapFrame {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "vector={} error={:#x} rip={:#018x} cs={:#x} rflags={:#x} rsp={:#018x} ss={:#x}",
            self.vector, self.error_code, self.rip, self.cs, self.rflags, self.rsp, self.ss
        )?;
        writeln!(
            f,
            "rax={:#018x} rbx={:#018x} rcx={:#018x} rdx={:#018x}",
            self.rax, self.rbx, self.rcx, self.rdx
        )?;
        writeln!(
            f,
            "rsi={:#018x} rdi={:#018x} rbp={:#018x} r8 ={:#018x}",
            self.rsi, self.rdi, self.rbp, self.r8
        )?;
        writeln!(
            f,
            "r9 ={:#018x} r10={:#018x} r11={:#018x} r12={:#018x}",
            self.r9, self.r10, self.r11, self.r12
        )?;
        write!(
            f,
            "r13={:#018x} r14={:#018x} r15={:#018x}",
            self.r13, self.r14, self.r15
        )
    }
}

// 各ベクタのスタブ (build.rs が生成)。
global_asm!(include_str!(concat!(
    env!("OUT_DIR"),
    "/interrupt_vectors.s"
)));

// 全ベクタ共通の入口。
global_asm!(
    ".section .text.yk_interrupt_common, \"ax\", @progbits",
    ".global yk_interrupt_common",
    "yk_interrupt_common:",
    "    push rax",
    "    push rbx",
    "    push rcx",
    "    push rdx",
    "    push rsi",
    "    push rdi",
    "    push rbp",
    "    push r8",
    "    push r9",
    "    push r10",
    "    push r11",
    "    push r12",
    "    push r13",
    "    push r14",
    "    push r15",
    "    mov rdi, rsp",
    "    cld",
    "    call yk_interrupt_dispatch",
    "    pop r15",
    "    pop r14",
    "    pop r13",
    "    pop r12",
    "    pop r11",
    "    pop r10",
    "    pop r9",
    "    pop r8",
    "    pop rbp",
    "    pop rdi",
    "    pop rsi",
    "    pop rdx",
    "    pop rcx",
    "    pop rbx",
    "    pop rax",
    // ベクタ番号とエラーコードを捨てる
    "    add rsp, 16",
    "    iretq",
);

unsafe extern "C" {
    /// build.rs が生成するスタブのアドレス表。
    static yk_interrupt_vectors: [u64; 256];
}

/// ベクタ `vector` のエントリスタブのアドレス。
pub fn vector_entry(vector: u8) -> u64 {
    // SAFETY: `yk_interrupt_vectors` は build.rs が生成した 256 要素の読み取り専用の表で、
    // リンク時に解決される。読み取りのみで書き換えは無い。
    unsafe { yk_interrupt_vectors[usize::from(vector)] }
}

/// アセンブリの共通入口から呼ばれる Rust 側の入口。
#[unsafe(no_mangle)]
extern "C" fn yk_interrupt_dispatch(frame: *mut TrapFrame) {
    // SAFETY: `frame` は yk_interrupt_common が現在のスタック上に積んだ TrapFrame を指し、
    // この関数から戻るまで有効で、他から参照されない。
    let frame = unsafe { &mut *frame };
    crate::interrupts::dispatch(frame);
}

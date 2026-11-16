//! 16550 互換 UART (COM1) ドライバ。カーネルログの出力先 (§38)。
//!
//! - 115200 baud, 8N1, FIFO 有効、割り込みは使わない(ポーリング送信)。
//! - 初期化時にループバック試験を行い、UART が存在しない実機では
//!   [`SerialError::NotPresent`] を返す(その場合ログは捨てられるが起動は継続する)。

use core::fmt;
use core::sync::atomic::{AtomicBool, Ordering};

use crate::arch::x86_64::port::Port;

/// COM1 の I/O ベースアドレス。
pub const COM1_BASE: u16 = 0x3F8;

/// 送信バッファ空き待ちの最大ポーリング回数。
/// 壊れた/存在しない UART で無限ループしないための上限。
const TX_SPIN_LIMIT: u32 = 100_000;

/// ループバック試験に使うバイト。
const LOOPBACK_PATTERN: u8 = 0xAE;

/// COM1 が初期化済みで使用可能か(パニック時の緊急出力判定用)。
static COM1_READY: AtomicBool = AtomicBool::new(false);

/// UART 初期化エラー。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SerialError {
    /// ループバック試験に失敗した(UART が存在しない、または故障)。
    NotPresent {
        /// 読み戻した値
        read_back: u8,
    },
}

impl fmt::Display for SerialError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotPresent { read_back } => write!(
                f,
                "UART loopback test failed (wrote {LOOPBACK_PATTERN:#04x}, read {read_back:#04x})"
            ),
        }
    }
}

/// 16550 UART のレジスタ (ベースからのオフセット)。
#[derive(Debug, Clone, Copy)]
#[repr(u16)]
enum Register {
    /// THR/RBR (DLAB=0), 分周値下位 DLL (DLAB=1)
    Data = 0,
    /// IER (DLAB=0), 分周値上位 DLM (DLAB=1)
    InterruptEnable = 1,
    /// FCR
    FifoControl = 2,
    /// LCR
    LineControl = 3,
    /// MCR
    ModemControl = 4,
    /// LSR
    LineStatus = 5,
}

/// 16550 UART の 1 ポート。
#[derive(Debug)]
pub struct SerialPort {
    base: u16,
}

impl SerialPort {
    /// `base` から始まる UART へのハンドルを作る(まだ初期化はしない)。
    ///
    /// # Safety
    ///
    /// `base..base+8` が 16550 互換 UART のレジスタ(または何も無い I/O 空間)であり、
    /// 他のコードが同時に同じ UART を操作しないこと。
    pub const unsafe fn new(base: u16) -> Self {
        Self { base }
    }

    fn reg(&self, reg: Register) -> Port {
        // SAFETY: `SerialPort::new` の契約により、base からのオフセット 0..8 は
        // この UART のレジスタ(または未使用ポート)であり、排他的に使用される。
        unsafe { Port::new(self.base + reg as u16) }
    }

    fn write_reg(&mut self, reg: Register, value: u8) {
        self.reg(reg).write(value);
    }

    fn read_reg(&mut self, reg: Register) -> u8 {
        self.reg(reg).read()
    }

    /// UART を 115200 baud 8N1 で初期化し、ループバック試験を行う。
    pub fn init(&mut self) -> Result<(), SerialError> {
        use Register::*;
        self.write_reg(InterruptEnable, 0x00); // 割り込み無効
        self.write_reg(LineControl, 0x80); // DLAB=1
        self.write_reg(Data, 0x01); // divisor low  = 1 -> 115200 baud
        self.write_reg(InterruptEnable, 0x00); // divisor high = 0
        self.write_reg(LineControl, 0x03); // DLAB=0, 8 data bits, no parity, 1 stop bit
        self.write_reg(FifoControl, 0xC7); // FIFO 有効, クリア, 14 バイト閾値
        self.write_reg(ModemControl, 0x1E); // ループバックモード (RTS, OUT1, OUT2, LOOP)

        self.write_reg(Data, LOOPBACK_PATTERN);
        let read_back = self.read_reg(Data);
        if read_back != LOOPBACK_PATTERN {
            return Err(SerialError::NotPresent { read_back });
        }

        self.write_reg(ModemControl, 0x0F); // 通常モード (DTR, RTS, OUT1, OUT2)
        Ok(())
    }

    fn transmit_empty(&mut self) -> bool {
        self.read_reg(Register::LineStatus) & 0x20 != 0
    }

    /// 1 バイト送信する。送信可能にならない場合は上限回数で諦める。
    pub fn write_byte(&mut self, byte: u8) {
        let mut spins = 0;
        while !self.transmit_empty() {
            spins += 1;
            if spins >= TX_SPIN_LIMIT {
                return;
            }
            core::hint::spin_loop();
        }
        self.write_reg(Register::Data, byte);
    }
}

impl fmt::Write for SerialPort {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for byte in s.bytes() {
            if byte == b'\n' {
                self.write_byte(b'\r');
            }
            self.write_byte(byte);
        }
        Ok(())
    }
}

/// COM1 を初期化して返す。成功時は緊急出力も有効になる。
pub fn init_com1() -> Result<SerialPort, SerialError> {
    // SAFETY: COM1 (0x3F8) は PC 互換機の標準 UART の I/O 範囲である。
    // この関数はロガー初期化時に 1 度だけ呼ばれ、以降は COM1 をロガーが所有する。
    let mut port = unsafe { SerialPort::new(COM1_BASE) };
    port.init()?;
    COM1_READY.store(true, Ordering::Release);
    Ok(port)
}

/// パニック時など、ロガーのロックを取れない状況で COM1 へ直接書き出す。
///
/// COM1 が初期化済みでなければ何もしない。
pub fn emergency_write(args: fmt::Arguments<'_>) {
    if !COM1_READY.load(Ordering::Acquire) {
        return;
    }
    // SAFETY: COM1 は初期化済みの UART である。この関数はパニック経路から
    // 割り込み無効・単一 CPU (AP は未起動) の状態で呼ばれるため、並行に動くのは
    // 中断されたロガー処理だけであり、起こり得るのは出力の混在のみでメモリ安全性は損なわない。
    let mut port = unsafe { SerialPort::new(COM1_BASE) };
    let _ = fmt::Write::write_fmt(&mut port, args);
}

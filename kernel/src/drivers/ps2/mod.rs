//! i8042 (PS/2) コントローラ。
//!
//! 初期化手順 (OSDev "8042 PS/2 Controller" / IBM PC/AT 技術資料に準拠):
//!
//! 1. ACPI FADT の IAPC_BOOT_ARCH が「8042 無し」を示す場合、またはステータスポートが
//!    0xFF (何も無いバス) の場合は「存在しない」として何もしない
//! 2. 両ポートを無効化 (0xAD, 0xA7) し、出力バッファを空にする
//! 3. 設定バイトを読み (0x20)、IRQ を止めた設定を書く (0x60)
//! 4. コントローラ自己診断 (0xAA → 0x55)。一部の機種は自己診断で設定がリセットされるので書き直す
//! 5. 2 ポート目の有無を判定 (0xA8 で有効化して設定バイトの bit5 を見る) — Phase 3 (マウス) 用に記録
//! 6. ポート 1 のインターフェーステスト (0xAB → 0x00)、あれば 2 も (0xA9)
//! 7. ポート 1 を有効化 (0xAE)、キーボードをリセット (0xFF → 0xFA, 0xAA)、
//!    スキャンコードセット 2 を選び (0xF0 0x02)、スキャン有効化 (0xF4)
//! 8. 設定バイトで IRQ1 と**スキャンコード変換 (set 2 → set 1)** を有効にする
//!
//! したがって IRQ1 で受け取るバイトは **スキャンコードセット 1** (変換後) である。
//!
//! すべての待ちにタイムアウト ([`crate::time::Deadline`]) を付け、i8042 が存在しない・
//! 応答しない実機でもハングせずにエラーを返す。
//!
//! ポート 2 (マウス, IRQ12) は Phase 3 で有効化する。そのための
//! [`send_to_port2`] (0xD4 プレフィックス) と [`ControllerInfo::dual_channel`] を用意している。

pub mod keyboard;

use core::fmt;
use core::time::Duration;

use crate::arch::x86_64::port::Port;
use crate::sync::{IrqSpinLock, Once};
use crate::time::Deadline;

const DATA: u16 = 0x60;
const STATUS_CMD: u16 = 0x64;

const STATUS_OUTPUT_FULL: u8 = 1 << 0;
const STATUS_INPUT_FULL: u8 = 1 << 1;
/// 出力バッファのデータが 2 ポート目 (マウス) からのものか
pub(crate) const STATUS_AUX_DATA: u8 = 1 << 5;

const CONFIG_PORT1_IRQ: u8 = 1 << 0;
const CONFIG_PORT2_IRQ: u8 = 1 << 1;
const CONFIG_PORT1_CLOCK_DISABLE: u8 = 1 << 4;
const CONFIG_PORT2_CLOCK_DISABLE: u8 = 1 << 5;
const CONFIG_TRANSLATION: u8 = 1 << 6;

const COMMAND_TIMEOUT: Duration = Duration::from_millis(50);
const RESET_TIMEOUT: Duration = Duration::from_millis(1000);

const ACK: u8 = 0xFA;
const RESEND: u8 = 0xFE;

/// 初期化エラー。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ps2Error {
    /// コントローラが存在しない (FADT またはステータス 0xFF)
    NotPresent(&'static str),
    /// 応答がタイムアウトした
    Timeout(&'static str),
    /// コントローラ自己診断の失敗 (応答値)
    SelfTestFailed(u8),
    /// ポート 1 のインターフェーステスト失敗 (応答値)
    PortTestFailed(u8),
    /// キーボードのリセット (BAT) 失敗 (応答値)
    DeviceResetFailed(u8),
    /// コマンドに ACK が返らない (コマンド, 応答値)
    NoAck(u8, u8),
}

impl fmt::Display for Ps2Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotPresent(why) => write!(f, "i8042 controller not present ({why})"),
            Self::Timeout(what) => write!(f, "timeout while waiting for {what}"),
            Self::SelfTestFailed(v) => write!(
                f,
                "controller self test failed (got {v:#04x}, expected 0x55)"
            ),
            Self::PortTestFailed(v) => write!(f, "port 1 interface test failed (got {v:#04x})"),
            Self::DeviceResetFailed(v) => {
                write!(f, "keyboard reset failed (got {v:#04x}, expected 0xaa)")
            }
            Self::NoAck(cmd, v) => write!(
                f,
                "keyboard did not acknowledge command {cmd:#04x} (got {v:#04x})"
            ),
        }
    }
}

/// コントローラの初期化結果。
#[derive(Debug, Clone, Copy)]
pub struct ControllerInfo {
    /// 2 ポート目 (マウス) があるか
    pub dual_channel: bool,
    /// 2 ポート目のインターフェーステストに通ったか
    pub port2_ok: bool,
    /// 最終的な設定バイト
    pub config: u8,
    /// IRQ1 で届くスキャンコードセット (変換後)
    pub scancode_set: u8,
}

static INFO: Once<ControllerInfo> = Once::new();
/// ポートアクセスの排他 (Phase 3 でマウスのコマンド送信と衝突しないように)。
static IO: IrqSpinLock<()> = IrqSpinLock::new(());

fn port(p: u16) -> Port {
    // SAFETY: 0x60 (データ) / 0x64 (ステータス・コマンド) は i8042 のポートで、
    // このモジュール (と keyboard / 将来の mouse サブモジュール) だけが操作する。
    // i8042 は DMA を行わない。
    unsafe { Port::new(p) }
}

pub(crate) fn read_status() -> u8 {
    port(STATUS_CMD).read()
}

pub(crate) fn read_data() -> u8 {
    port(DATA).read()
}

fn wait_input_clear(what: &'static str) -> Result<(), Ps2Error> {
    let mut d = Deadline::after(COMMAND_TIMEOUT);
    while read_status() & STATUS_INPUT_FULL != 0 {
        if d.expired() {
            return Err(Ps2Error::Timeout(what));
        }
        core::hint::spin_loop();
    }
    Ok(())
}

fn wait_output_full(timeout: Duration, what: &'static str) -> Result<u8, Ps2Error> {
    let mut d = Deadline::after(timeout);
    while read_status() & STATUS_OUTPUT_FULL == 0 {
        if d.expired() {
            return Err(Ps2Error::Timeout(what));
        }
        core::hint::spin_loop();
    }
    Ok(read_data())
}

fn command(cmd: u8) -> Result<(), Ps2Error> {
    wait_input_clear("controller input buffer")?;
    port(STATUS_CMD).write(cmd);
    Ok(())
}

fn command_with_response(cmd: u8, what: &'static str) -> Result<u8, Ps2Error> {
    command(cmd)?;
    wait_output_full(COMMAND_TIMEOUT, what)
}

fn write_config(config: u8) -> Result<(), Ps2Error> {
    command(0x60)?;
    wait_input_clear("config write")?;
    port(DATA).write(config);
    Ok(())
}

fn flush_output() -> u32 {
    let mut n = 0;
    while read_status() & STATUS_OUTPUT_FULL != 0 && n < 64 {
        let _ = read_data();
        n += 1;
    }
    n
}

/// ポート 1 (キーボード) へ 1 バイト送り、ACK を待つ (RESEND なら 3 回まで再送)。
fn send_to_port1(byte: u8) -> Result<(), Ps2Error> {
    for _ in 0..3 {
        wait_input_clear("keyboard input buffer")?;
        port(DATA).write(byte);
        let r = wait_output_full(COMMAND_TIMEOUT, "keyboard ACK")?;
        match r {
            ACK => return Ok(()),
            RESEND => continue,
            other => return Err(Ps2Error::NoAck(byte, other)),
        }
    }
    Err(Ps2Error::NoAck(byte, RESEND))
}

/// ポート 2 (マウス) へ 1 バイト送り、ACK を待つ (Phase 3 用)。
#[allow(dead_code)] // Phase 3 (マウス) で使用
pub fn send_to_port2(byte: u8) -> Result<(), Ps2Error> {
    let _io = IO.lock();
    for _ in 0..3 {
        command(0xD4)?;
        wait_input_clear("mouse input buffer")?;
        port(DATA).write(byte);
        let r = wait_output_full(COMMAND_TIMEOUT, "mouse ACK")?;
        match r {
            ACK => return Ok(()),
            RESEND => continue,
            other => return Err(Ps2Error::NoAck(byte, other)),
        }
    }
    Err(Ps2Error::NoAck(byte, RESEND))
}

/// 初期化結果 (初期化済みなら)。
#[allow(dead_code)] // Phase 3 (マウス) で使用
pub fn info() -> Option<ControllerInfo> {
    INFO.get().copied()
}

/// コントローラとキーボードを初期化する (割り込み無効の状態で呼ぶ)。
///
/// `fadt_has_8042` は FADT の IAPC_BOOT_ARCH の 8042 ビット (FADT rev 2 未満なら `None`)。
pub fn init(fadt_has_8042: Option<bool>) -> Result<ControllerInfo, Ps2Error> {
    if fadt_has_8042 == Some(false) {
        return Err(Ps2Error::NotPresent(
            "ACPI FADT IAPC_BOOT_ARCH says no 8042",
        ));
    }
    let _io = IO.lock();
    if read_status() == 0xFF {
        return Err(Ps2Error::NotPresent("status port reads 0xff"));
    }

    // 2. 両ポート無効化・バッファを空に
    command(0xAD)?;
    command(0xA7)?;
    let flushed = flush_output();
    if flushed > 0 {
        debug!("flushed {flushed} stale byte(s)");
    }

    // 3. 設定バイト: IRQ と変換を止め、ポート 1 のクロックを有効に
    let mut config = command_with_response(0x20, "config byte")?;
    debug!("initial config byte {config:#04x}");
    config &= !(CONFIG_PORT1_IRQ | CONFIG_PORT2_IRQ | CONFIG_TRANSLATION);
    config &= !CONFIG_PORT1_CLOCK_DISABLE;
    write_config(config)?;

    // 4. 自己診断
    let r = command_with_response(0xAA, "controller self test")?;
    if r != 0x55 {
        return Err(Ps2Error::SelfTestFailed(r));
    }
    write_config(config)?;

    // 5. 2 ポート目の判定
    command(0xA8)?;
    let c = command_with_response(0x20, "config byte")?;
    let dual_channel = c & CONFIG_PORT2_CLOCK_DISABLE == 0;
    if dual_channel {
        command(0xA7)?;
    }

    // 6. インターフェーステスト
    let r = command_with_response(0xAB, "port 1 test")?;
    if r != 0x00 {
        return Err(Ps2Error::PortTestFailed(r));
    }
    let port2_ok = if dual_channel {
        let r = command_with_response(0xA9, "port 2 test")?;
        if r != 0 {
            warn!("port 2 interface test failed ({r:#04x}); mouse will be unavailable");
        }
        r == 0
    } else {
        false
    };

    // 7. ポート 1 を有効化してキーボードを初期化
    command(0xAE)?;
    send_to_port1(0xFF)?;
    let bat = wait_output_full(RESET_TIMEOUT, "keyboard self test (BAT)")?;
    if bat != 0xAA {
        return Err(Ps2Error::DeviceResetFailed(bat));
    }
    // リセット後に ID 等が続く機種があるので捨てる
    let _ = flush_output();
    // スキャンコードセット 2 (コントローラの変換で set 1 になる)
    if let Err(e) = send_to_port1(0xF0).and_then(|()| send_to_port1(0x02)) {
        warn!("ps2: cannot select scan code set 2 ({e}); keeping the keyboard default (set 2)");
    }
    send_to_port1(0xF4)?;

    // 8. IRQ1 と変換を有効化 (ポート 2 は Phase 3 まで無効のまま)
    config |= CONFIG_PORT1_IRQ | CONFIG_TRANSLATION;
    config &= !CONFIG_PORT2_IRQ;
    if dual_channel {
        config |= CONFIG_PORT2_CLOCK_DISABLE;
    }
    write_config(config)?;
    let _ = flush_output();

    let info = ControllerInfo {
        dual_channel,
        port2_ok,
        config,
        scancode_set: 1,
    };
    let _ = INFO.set(info);
    Ok(info)
}

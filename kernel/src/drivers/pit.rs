//! 8254 PIT (Programmable Interval Timer) のチャネル 2 を使った短時間の待ち。
//!
//! Local APIC タイマの周波数校正の参照クロック (ACPI PM timer が無い場合の代替) として使う。
//! チャネル 2 はスピーカ用で IRQ を出さず、出力 (OUT2) をポート 0x61 の bit5 で
//! ポーリングできる。チャネル 0 (IRQ0) は使わない (I/O APIC でマスクしたまま)。

use crate::arch::x86_64::port::Port;

/// PIT の入力クロック (Hz)。
pub const FREQUENCY_HZ: u64 = 1_193_182;

/// 待ちのポーリング回数の上限 (PIT が存在しない/動かない場合にハングしないため)。
const POLL_LIMIT: u64 = 200_000_000;

fn port(p: u16) -> Port {
    // SAFETY: 0x42 (チャネル 2 データ), 0x43 (モード/コマンド), 0x61 (システム制御ポート B) は
    // PC 互換機の PIT / スピーカ制御のポートで、操作するのはこのモジュールだけ (校正時のみ)。
    unsafe { Port::new(p) }
}

/// チャネル 2 を mode 0 (ターミナルカウント) で `count` から数え始め、
/// 0 になる (OUT2 が High になる) まで待つ。`count` は 1〜65535。
/// 戻り値は待ちが完了したか (`false` = タイムアウト: PIT が無いか動作していない)。
///
/// 呼び出し中の `before` / `after` コールバックで、開始直後と終了直後に
/// 計測対象 (Local APIC タイマ・TSC) を読む。
pub fn wait_ticks(count: u16, before: impl FnOnce(), after: impl FnOnce()) -> bool {
    let mut ctrl = port(0x61);
    // ゲートを下げ、スピーカ出力 (bit1) は無効にする
    let saved = ctrl.read();
    ctrl.write(saved & !0b11);
    // チャネル 2, lobyte/hibyte, mode 0, binary
    port(0x43).write(0b1011_0000);
    let mut data = port(0x42);
    data.write(count as u8);
    data.write((count >> 8) as u8);
    // ゲートを上げてカウント開始
    let gate_on = (ctrl.read() & !0b10) | 0b01;
    ctrl.write(gate_on);
    before();
    let mut polls = 0u64;
    let ok = loop {
        if ctrl.read() & 0x20 != 0 {
            break true;
        }
        polls += 1;
        if polls >= POLL_LIMIT {
            break false;
        }
        core::hint::spin_loop();
    };
    after();
    ctrl.write(saved & !0b11);
    ok
}

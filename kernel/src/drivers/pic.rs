//! レガシー 8259A PIC (マスター/スレーブ)。
//!
//! yumekawaOS は Local APIC + I/O APIC を使うため、8259 は使わない。
//! ただし BIOS 互換の初期状態ではベクタ 0x08〜0x0F (CPU 例外と衝突) に割り込みを出す
//! 設定のままなので、[`remap_and_mask_all`] で 0x20〜0x2F にリマップしてから全 IRQ を
//! マスクする。マスク後もスプリアス割り込み (IRQ7 = 0x27 / IRQ15 = 0x2F) は届き得るので、
//! 割り込みディスパッチャがそれを識別して無視する。

use crate::arch::x86_64::port::Port;
use crate::interrupts::vectors::{PIC_MASTER_BASE, PIC_SLAVE_BASE};

const MASTER_CMD: u16 = 0x20;
const MASTER_DATA: u16 = 0x21;
const SLAVE_CMD: u16 = 0xA0;
const SLAVE_DATA: u16 = 0xA1;

fn port(p: u16) -> Port {
    // SAFETY: 0x20/0x21/0xA0/0xA1 は PC 互換機の 8259 PIC の I/O ポートで、PIC を操作するのは
    // このモジュールだけ (初期化時と、スプリアス割り込み時の EOI)。PIC は DMA を行わない。
    unsafe { Port::new(p) }
}

/// 古いバスで書き込みの間隔を空けるためのダミー I/O (ポート 0x80: POST コード)。
fn io_wait() {
    // SAFETY: ポート 0x80 は POST 診断用で、書き込んでも副作用は無い (慣例的な I/O 待ち)。
    unsafe { Port::new(0x80) }.write(0);
}

/// PIC を 0x20〜0x2F にリマップし、すべての IRQ をマスクする。
pub fn remap_and_mask_all() {
    // ICW1: 初期化開始, ICW4 あり
    port(MASTER_CMD).write(0x11);
    io_wait();
    port(SLAVE_CMD).write(0x11);
    io_wait();
    // ICW2: ベクタオフセット
    port(MASTER_DATA).write(PIC_MASTER_BASE);
    io_wait();
    port(SLAVE_DATA).write(PIC_SLAVE_BASE);
    io_wait();
    // ICW3: マスターの IRQ2 にスレーブ / スレーブの ID = 2
    port(MASTER_DATA).write(0x04);
    io_wait();
    port(SLAVE_DATA).write(0x02);
    io_wait();
    // ICW4: 8086 モード
    port(MASTER_DATA).write(0x01);
    io_wait();
    port(SLAVE_DATA).write(0x01);
    io_wait();
    // 全マスク
    port(MASTER_DATA).write(0xFF);
    port(SLAVE_DATA).write(0xFF);
}

/// IRQ (0〜15) がその PIC の In-Service Register に立っているか
/// (スプリアス割り込みの判定用: 本物なら ISR のビットが立つ)。
pub fn in_service(irq: u8) -> bool {
    let (cmd, bit) = if irq < 8 {
        (MASTER_CMD, irq)
    } else {
        (SLAVE_CMD, irq - 8)
    };
    let mut p = port(cmd);
    p.write(0x0B); // OCW3: 次の読み出しで ISR
    p.read() & (1 << bit) != 0
}

/// マスター PIC に EOI を送る。
pub fn eoi_master() {
    port(MASTER_CMD).write(0x20);
}

/// スレーブ (と、カスケードのためマスター) に EOI を送る。
pub fn eoi_slave() {
    port(SLAVE_CMD).write(0x20);
    eoi_master();
}

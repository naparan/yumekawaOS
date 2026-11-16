//! 割り込みハンドラ → 通常コンテキストへバイト列を渡すロックフリーのリングバッファ。
//!
//! 生産者 1 (割り込みハンドラ)・消費者 1 (通常コンテキスト) を前提とする
//! (単一 CPU。生産者は割り込み無効の状態で動くため、生産者同士は重ならない)。
//! ロックを使わないので、割り込みハンドラ内で安全に `push` でき、消費者が
//! 割り込みを禁止する必要もない。容量を超えた分は捨てて [`SpscRing::dropped`] に数える。

use core::sync::atomic::{AtomicU8, AtomicU64, AtomicUsize, Ordering};

/// 容量 `N` バイト (`N` は 2 のべき乗) のリングバッファ。
#[derive(Debug)]
pub struct SpscRing<const N: usize> {
    buf: [AtomicU8; N],
    /// 次に書く位置 (単調増加。生産者だけが書く)
    head: AtomicUsize,
    /// 次に読む位置 (単調増加。消費者だけが書く)
    tail: AtomicUsize,
    dropped: AtomicU64,
}

impl<const N: usize> SpscRing<N> {
    const ASSERT_POW2: () = assert!(N.is_power_of_two(), "SpscRing capacity must be 2^n");

    /// 空のバッファ。
    pub const fn new() -> Self {
        let () = Self::ASSERT_POW2;
        Self {
            buf: [const { AtomicU8::new(0) }; N],
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
            dropped: AtomicU64::new(0),
        }
    }

    /// 1 バイト積む (生産者側)。満杯なら捨てて `false`。
    pub fn push(&self, byte: u8) -> bool {
        let head = self.head.load(Ordering::Relaxed);
        let tail = self.tail.load(Ordering::Acquire);
        if head.wrapping_sub(tail) >= N {
            self.dropped.fetch_add(1, Ordering::Relaxed);
            return false;
        }
        self.buf[head % N].store(byte, Ordering::Relaxed);
        self.head.store(head.wrapping_add(1), Ordering::Release);
        true
    }

    /// 1 バイト取り出す (消費者側)。
    pub fn pop(&self) -> Option<u8> {
        let tail = self.tail.load(Ordering::Relaxed);
        let head = self.head.load(Ordering::Acquire);
        if tail == head {
            return None;
        }
        let byte = self.buf[tail % N].load(Ordering::Relaxed);
        self.tail.store(tail.wrapping_add(1), Ordering::Release);
        Some(byte)
    }

    /// 満杯で捨てたバイト数の累計。
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }
}

impl<const N: usize> Default for SpscRing<N> {
    fn default() -> Self {
        Self::new()
    }
}

//! 1 度だけ初期化できるセル。

use core::cell::UnsafeCell;
use core::fmt;
use core::mem::MaybeUninit;
use core::sync::atomic::{AtomicU8, Ordering};

const EMPTY: u8 = 0;
const INITIALIZING: u8 = 1;
const READY: u8 = 2;

/// 起動時に 1 度だけ値を設定し、以降は `&T` で読むためのセル。
///
/// `set` は 1 度だけ成功する。初期化中に別コンテキスト (割り込みハンドラ等) から
/// `get` されても `None` が返るだけでブロックしない。
pub struct Once<T> {
    state: AtomicU8,
    value: UnsafeCell<MaybeUninit<T>>,
}

// SAFETY: 値は READY になった後は共有参照でしか触れず、書き込みは INITIALIZING 状態を
// 獲得した 1 コンテキストだけが行う。`T: Send + Sync` なら共有してよい。
unsafe impl<T: Send + Sync> Sync for Once<T> {}

impl<T> Once<T> {
    /// 空のセル。
    pub const fn new() -> Self {
        Self {
            state: AtomicU8::new(EMPTY),
            value: UnsafeCell::new(MaybeUninit::uninit()),
        }
    }

    /// 値を設定する。既に設定済み (または設定中) なら値を返して `Err`。
    pub fn set(&self, value: T) -> Result<&T, T> {
        if self
            .state
            .compare_exchange(EMPTY, INITIALIZING, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            return Err(value);
        }
        // SAFETY: INITIALIZING を獲得したのはこのコンテキストだけなので、書き込みは排他的。
        // READY になるまで他は `value` に触れない。
        let slot = unsafe { &mut *self.value.get() };
        let r: &T = slot.write(value);
        self.state.store(READY, Ordering::Release);
        Ok(r)
    }

    /// 設定済みなら値への参照。
    pub fn get(&self) -> Option<&T> {
        if self.state.load(Ordering::Acquire) == READY {
            let slot = self.value.get().cast::<T>();
            // SAFETY: READY は `set` が値を書き終えた後にだけ立ち、以降は書き換えられない。
            // MaybeUninit<T> は T と同じレイアウトなので、初期化済みの T として参照できる。
            Some(unsafe { &*slot })
        } else {
            None
        }
    }
}

impl<T> Default for Once<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: fmt::Debug> fmt::Debug for Once<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Once").field(&self.get()).finish()
    }
}

//! 単純なスピンロック。
//!
//! Phase 1 では割り込みが無効・BSP のみで動作するため競合は起きないが、
//! 共有状態 (ロガー等) を `static` に安全に置くために使う。
//!
//! 注意 (Phase 2 以降): 割り込みハンドラからも取得するロックは、取得中に
//! 割り込みを無効化しないとデッドロックし得る。必要になった時点で
//! 割り込み無効化付きのガードを追加すること。

use core::cell::UnsafeCell;
use core::fmt;
use core::ops::{Deref, DerefMut};
use core::sync::atomic::{AtomicBool, Ordering};

/// 値 `T` をスピンロックで保護する。
pub struct SpinLock<T> {
    locked: AtomicBool,
    value: UnsafeCell<T>,
}

// SAFETY: `value` へのアクセスは `locked` によって常に 1 つのガードに限定されるため、
// `T: Send` であれば複数コンテキストから共有してよい(std::sync::Mutex と同じ条件)。
unsafe impl<T: Send> Sync for SpinLock<T> {}

impl<T> SpinLock<T> {
    /// 新しいロックを作る。
    pub const fn new(value: T) -> Self {
        Self {
            locked: AtomicBool::new(false),
            value: UnsafeCell::new(value),
        }
    }

    /// ロックを取得するまでスピンする。
    pub fn lock(&self) -> SpinLockGuard<'_, T> {
        loop {
            if let Some(guard) = self.try_lock() {
                return guard;
            }
            while self.locked.load(Ordering::Relaxed) {
                core::hint::spin_loop();
            }
        }
    }

    /// ロックを 1 回だけ試みる。
    pub fn try_lock(&self) -> Option<SpinLockGuard<'_, T>> {
        self.locked
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .ok()
            .map(|_| SpinLockGuard { lock: self })
    }
}

impl<T> fmt::Debug for SpinLock<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SpinLock")
            .field("locked", &self.locked.load(Ordering::Relaxed))
            .finish()
    }
}

/// [`SpinLock`] のガード。drop 時にロックを解放する。
pub struct SpinLockGuard<'a, T> {
    lock: &'a SpinLock<T>,
}

impl<T> Deref for SpinLockGuard<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        // SAFETY: ガードが存在する間はロックを保持しており、他に参照は存在しない。
        unsafe { &*self.lock.value.get() }
    }
}

impl<T> DerefMut for SpinLockGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: ガードが存在する間はロックを保持しており、`&mut self` により
        // このガード経由の参照も一意である。
        unsafe { &mut *self.lock.value.get() }
    }
}

impl<T> Drop for SpinLockGuard<'_, T> {
    fn drop(&mut self) {
        self.lock.locked.store(false, Ordering::Release);
    }
}

impl<T> fmt::Debug for SpinLockGuard<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SpinLockGuard")
    }
}

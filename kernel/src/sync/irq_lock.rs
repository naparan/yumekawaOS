//! 割り込み安全なスピンロック。
//!
//! 取得時に割り込みを無効化し (RFLAGS.IF を保存して `cli`)、ガードの drop 時にロックを
//! 解放してから元の IF 状態へ戻す。これにより「ロック保持中に割り込みが入り、
//! ハンドラが同じロックを取ろうとしてデッドロックする」ことが単一 CPU 上では起きない。
//!
//! ## 規約 (docs/architecture.md「ロック規約」)
//!
//! - 割り込みハンドラ・例外ハンドラから触れうる共有状態は必ずこのロックで保護する。
//! - NMI / 致命的例外 (パニック経路) からは `lock()` を呼ばない。`try_lock()` か、
//!   パニック経路専用の [`IrqSpinLock::force_lock`] を使う。
//! - ガードは入れ子にしてよい (内側のガードは「割り込み無効」を保存し、外側が元に戻す)。
//!   ただし drop の順序は取得の逆順にすること (通常のスコープでは自然にそうなる)。
//! - 複数ロックの取得順序はモジュールの文書に記載する (例: heap → kernel space → frames)。

use core::cell::UnsafeCell;
use core::fmt;
use core::ops::{Deref, DerefMut};
use core::sync::atomic::{AtomicBool, Ordering};

use crate::arch::x86_64::cpu;

/// 割り込みを無効化して取得するスピンロック。
pub struct IrqSpinLock<T> {
    locked: AtomicBool,
    value: UnsafeCell<T>,
}

// SAFETY: `value` へのアクセスは `locked` によって常に 1 つのガードに限定されるため、
// `T: Send` であれば複数コンテキストから共有してよい(std::sync::Mutex と同じ条件)。
unsafe impl<T: Send> Sync for IrqSpinLock<T> {}

impl<T> IrqSpinLock<T> {
    /// 新しいロックを作る。
    pub const fn new(value: T) -> Self {
        Self {
            locked: AtomicBool::new(false),
            value: UnsafeCell::new(value),
        }
    }

    /// 割り込みを無効化してロックを取得する。
    pub fn lock(&self) -> IrqSpinLockGuard<'_, T> {
        let restore = cpu::interrupts_enabled();
        cpu::disable_interrupts();
        loop {
            if self
                .locked
                .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
                .is_ok()
            {
                return IrqSpinLockGuard {
                    lock: self,
                    restore_interrupts: restore,
                };
            }
            while self.locked.load(Ordering::Relaxed) {
                core::hint::spin_loop();
            }
        }
    }

    /// ロックを 1 回だけ試みる。失敗したら割り込み状態を元に戻して `None`。
    pub fn try_lock(&self) -> Option<IrqSpinLockGuard<'_, T>> {
        let restore = cpu::interrupts_enabled();
        cpu::disable_interrupts();
        if self
            .locked
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
        {
            Some(IrqSpinLockGuard {
                lock: self,
                restore_interrupts: restore,
            })
        } else {
            if restore {
                cpu::enable_interrupts();
            }
            None
        }
    }

    /// 保持者を無視してロックを奪う (**パニック経路専用**)。
    ///
    /// # Safety
    ///
    /// - 呼び出し時点で割り込みが無効であり、この後保持者のコードが二度と実行されないこと
    ///   (パニック処理で CPU を停止する直前など)。
    /// - 他の CPU がこのロックを使っていないこと (現状 BSP のみ。SMP 導入時は
    ///   パニック時に他 CPU を停止してから呼ぶこと)。
    ///
    /// 保持者が更新途中だった値を読む可能性があるため、`T` の不変条件が崩れていても
    /// メモリ安全性を損なわない使い方 (例: 範囲チェック付きの読み出し) に限ること。
    pub unsafe fn force_lock(&self) -> IrqSpinLockGuard<'_, T> {
        self.locked.swap(true, Ordering::Acquire);
        IrqSpinLockGuard {
            lock: self,
            restore_interrupts: false,
        }
    }
}

impl<T> fmt::Debug for IrqSpinLock<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("IrqSpinLock")
            .field("locked", &self.locked.load(Ordering::Relaxed))
            .finish()
    }
}

/// [`IrqSpinLock`] のガード。drop 時にロックを解放し、割り込み状態を元に戻す。
pub struct IrqSpinLockGuard<'a, T> {
    lock: &'a IrqSpinLock<T>,
    restore_interrupts: bool,
}

impl<T> Deref for IrqSpinLockGuard<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        // SAFETY: ガードが存在する間はロックを保持しており、他に参照は存在しない。
        unsafe { &*self.lock.value.get() }
    }
}

impl<T> DerefMut for IrqSpinLockGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: ガードが存在する間はロックを保持しており、`&mut self` により
        // このガード経由の参照も一意である。
        unsafe { &mut *self.lock.value.get() }
    }
}

impl<T> Drop for IrqSpinLockGuard<'_, T> {
    fn drop(&mut self) {
        self.lock.locked.store(false, Ordering::Release);
        if self.restore_interrupts {
            cpu::enable_interrupts();
        }
    }
}

impl<T> fmt::Debug for IrqSpinLockGuard<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("IrqSpinLockGuard")
    }
}

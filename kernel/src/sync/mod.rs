//! カーネル内の同期プリミティブ。

mod spinlock;

pub use self::spinlock::SpinLock;

//! カーネル内の同期プリミティブ。
//!
//! | 型 | 用途 |
//! | --- | --- |
//! | [`IrqSpinLock`] | 割り込みハンドラ・例外ハンドラからも触れうる共有状態 (ロガー、ヒープ、ページテーブル等)。取得中は割り込み禁止 |
//! | [`Once`] | 起動時に 1 度だけ初期化し、以降は読み取り専用の値 |
//! | [`SpscRing`] | 割り込みハンドラ (生産者 1) → 通常コンテキスト (消費者 1) のロックフリーなバイト列キュー |

mod irq_lock;
mod once;
mod ring;

pub use self::irq_lock::{IrqSpinLock, IrqSpinLockGuard};
pub use self::once::Once;
pub use self::ring::SpscRing;

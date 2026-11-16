//! アーキテクチャ依存コード。
//!
//! 上位層はこのモジュールが再エクスポートする safe な API だけを使う。
//! 現在の対象は x86_64 のみ(§4.1)。

pub mod x86_64;

pub use self::x86_64::cpu::{disable_interrupts, halt_forever};

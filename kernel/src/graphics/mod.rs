//! 描画。
//!
//! Phase 1 では、ブートローダが用意した線形 framebuffer への CPU ソフトウェア描画と、
//! 起動確認用のスプラッシュ画面のみを提供する。本格的な 2D レンダラ・フォント
//! レンダラは Phase 3 で実装する(§34)。

pub mod color;
pub mod font;
pub mod framebuffer;
pub mod splash;

pub use self::framebuffer::Framebuffer;

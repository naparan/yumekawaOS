//! 描画。
//!
//! ブートローダが用意した線形 framebuffer への CPU ソフトウェア描画、起動確認用の
//! スプラッシュ画面、Kernel Panic 画面を提供する。framebuffer は [`display`] が
//! グローバルに所有する。本格的な 2D レンダラ・フォントレンダラは Phase 3 で実装する(§34)。

pub mod color;
pub mod display;
pub mod font;
pub mod framebuffer;
pub mod panic_screen;
pub mod splash;

pub use self::framebuffer::Framebuffer;

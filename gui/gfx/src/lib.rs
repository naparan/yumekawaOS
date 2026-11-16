//! yumekawaOS 2D 描画ライブラリ。
//!
//! `no_std` + `alloc`、ハードウェア非依存、`unsafe` なし。カーネル (バックバッファ)、
//! 将来のコンポジタ・ウィジェット・ブラウザから共通に使う。詳しくは `docs/graphics.md`。
//!
//! - [`Color`] / [`blend_over`]: ARGB8888 (straight alpha) と source-over 合成
//! - [`Canvas`] / [`Image`] / [`ImageView`]: 描画面・画像。すべての描画はクリップ矩形を尊重する
//! - 図形 (線・角丸矩形・楕円・グラデーション)、画像転送 (コピー/アルファ/拡縮)
//! - `png` モジュール: PNG デコード (サイズ上限つき)
//! - `text` モジュール: アウトラインフォントの UTF-8 テキスト描画 (グリフキャッシュ・折り返し)
//! - `bitmap` モジュール: ヒープ不要のビットマップフォント描画 (早期コンソール/パニック画面用)
//! - [`Rect`] / [`DamageList`]: 矩形演算とダメージ管理

#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

pub mod bitmap;
pub mod canvas;
pub mod color;
pub mod error;
pub mod image;
pub mod png;
pub mod rect;
mod shapes;
pub mod text;

pub use canvas::{BlendMode, Canvas, Filter, MaskRef};
pub use color::{Color, blend_over, palette};
pub use error::GfxError;
pub use image::{Image, ImageView, Surface};
pub use rect::{DamageList, Rect};

//! Phase 1 用の最小限のビットマップフォント描画。
//!
//! グリフは `font8x8` クレート (パブリックドメインの 8x8 フォント) の
//! ASCII 部分を使う。各行 1 バイトで、LSB が左端のピクセル。
//! 整数倍の拡大のみ対応。本格的なフォントレンダラ (UTF-8 / 日本語) は Phase 3。

use font8x8::legacy::BASIC_LEGACY;

use super::color::Rgb;
use super::framebuffer::Framebuffer;

/// グリフの幅・高さ (ピクセル, 拡大前)。
pub const GLYPH_SIZE: u64 = 8;

/// ASCII 以外の文字に使う四角形グリフ。
const FALLBACK_GLYPH: [u8; 8] = [0x7E, 0x42, 0x42, 0x42, 0x42, 0x42, 0x7E, 0x00];

/// 文字 `c` の 8x8 ビットマップ。
pub fn glyph(c: char) -> &'static [u8; 8] {
    let code = c as usize;
    if code < BASIC_LEGACY.len() {
        &BASIC_LEGACY[code]
    } else {
        &FALLBACK_GLYPH
    }
}

/// 文字列を `scale` 倍で描いたときの幅 (ピクセル)。
pub fn text_width(text: &str, scale: u64) -> u64 {
    text.chars().count() as u64 * GLYPH_SIZE * scale
}

/// 8x8 ビットマップを `scale` 倍で描く。色は `shade(row)` (row = 拡大後の行 0..8*scale)。
pub fn draw_bitmap(
    fb: &mut Framebuffer,
    x: i64,
    y: i64,
    bitmap: &[u8; 8],
    scale: u64,
    shade: &mut impl FnMut(u64) -> Rgb,
) {
    let s = scale as i64;
    for (row, bits) in bitmap.iter().enumerate() {
        for col in 0..8 {
            if bits & (1 << col) != 0 {
                for sy in 0..scale {
                    let r = row as u64 * scale + sy;
                    let color = shade(r);
                    fb.fill_rect(x + col * s, y + r as i64, scale, 1, color);
                }
            }
        }
    }
}

/// 文字列を描く。色は行ごとに `shade(row)` で決める(縦グラデーション文字用)。
pub fn draw_text_shaded(
    fb: &mut Framebuffer,
    x: i64,
    y: i64,
    text: &str,
    scale: u64,
    mut shade: impl FnMut(u64) -> Rgb,
) {
    let advance = (GLYPH_SIZE * scale) as i64;
    for (i, c) in text.chars().enumerate() {
        draw_bitmap(fb, x + i as i64 * advance, y, glyph(c), scale, &mut shade);
    }
}

/// 単色で文字列を描く。
pub fn draw_text(fb: &mut Framebuffer, x: i64, y: i64, text: &str, scale: u64, color: Rgb) {
    draw_text_shaded(fb, x, y, text, scale, |_| color);
}

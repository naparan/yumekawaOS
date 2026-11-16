//! ヒープ不要のビットマップフォント描画 (8x8 など)。
//!
//! 早期コンソールやパニック画面のように、アロケータが使えない/使いたくない場面向け。
//! フォントデータは呼び出し側が渡す (例: `font8x8` の `[u8; 8]`、各行 1 バイト、
//! LSB が左端のピクセル)。この関数群はヒープを確保しない。

use crate::canvas::Canvas;
use crate::color::Color;
use crate::rect::Rect;

/// 8x8 ビットマップ 1 文字を `scale` 倍 (整数) で描く。ビットが立っている画素を `color` で塗る
/// (`scale == 0` は何もしない)。`font8x8` の形式: `bitmap[行]` の bit0 が左端。
pub fn draw_glyph8(
    canvas: &mut Canvas<'_>,
    x: i32,
    y: i32,
    bitmap: &[u8; 8],
    scale: u32,
    color: Color,
) {
    if scale == 0 {
        return;
    }
    for (row, bits) in bitmap.iter().enumerate() {
        let mut col = 0u32;
        while col < 8 {
            if bits & (1 << col) == 0 {
                col += 1;
                continue;
            }
            // 連続する立ちビットを 1 つの矩形にまとめる。
            let start = col;
            while col < 8 && bits & (1 << col) != 0 {
                col += 1;
            }
            let rx = x as i64 + start as i64 * scale as i64;
            let ry = y as i64 + row as i64 * scale as i64;
            canvas.fill_rect(
                Rect::from_ltrb(
                    rx,
                    ry,
                    rx + (col - start) as i64 * scale as i64,
                    ry + scale as i64,
                ),
                color,
            );
        }
    }
}

/// 文字列を 8x8 ビットマップフォントで描く。`lookup` は文字から 8x8 ビットマップを返す
/// (無い文字は `None` で、四角の豆腐を描く)。`\n` は次の行 (`8*scale` 下、x は元に戻る)。
/// 描いた末尾のペン位置 `(x, y)` を返す。
pub fn draw_text8<'f>(
    canvas: &mut Canvas<'_>,
    x: i32,
    y: i32,
    text: &str,
    scale: u32,
    color: Color,
    lookup: impl Fn(char) -> Option<&'f [u8; 8]>,
) -> (i64, i64) {
    const TOFU: [u8; 8] = [0x7E, 0x42, 0x42, 0x42, 0x42, 0x42, 0x7E, 0x00];
    let adv = 8 * scale as i64;
    let (mut px, mut py) = (x as i64, y as i64);
    for ch in text.chars() {
        if ch == '\n' {
            px = x as i64;
            py += adv;
            continue;
        }
        let glyph = lookup(ch).unwrap_or(&TOFU);
        if px >= i32::MIN as i64
            && px <= i32::MAX as i64
            && py >= i32::MIN as i64
            && py <= i32::MAX as i64
        {
            draw_glyph8(canvas, px as i32, py as i32, glyph, scale, color);
        }
        px += adv;
    }
    (px, py)
}

/// `text` を 8x8 フォントで描いたときの幅 (改行を含む場合は最長の行)。
pub fn measure_text8(text: &str, scale: u32) -> u64 {
    text.split('\n')
        .map(|l| l.chars().count() as u64 * 8 * scale as u64)
        .max()
        .unwrap_or(0)
}

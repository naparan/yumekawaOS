//! ヒープ不要のビットマップフォント描画のテスト。

use yk_gfx::bitmap::{draw_glyph8, draw_text8, measure_text8};
use yk_gfx::{Color, Image, Rect};

// 'A' (font8x8 形式: bit0 が左端)
const A: [u8; 8] = [0x0C, 0x1E, 0x33, 0x33, 0x3F, 0x33, 0x33, 0x00];

fn count(i: &Image) -> usize {
    i.pixels().iter().filter(|&&p| p == Color::WHITE.0).count()
}

#[test]
fn glyph_pixels_and_scale() {
    let mut i = Image::filled(32, 32, Color::BLACK).unwrap();
    draw_glyph8(&mut i.canvas(), 0, 0, &A, 1, Color::WHITE);
    let ones: u32 = A.iter().map(|b| b.count_ones()).sum();
    assert_eq!(count(&i) as u32, ones);
    // bit0 が左端: 0x0C = 0b00001100 → x=2,3
    assert_eq!(i.get(2, 0), Some(Color::WHITE));
    assert_eq!(i.get(3, 0), Some(Color::WHITE));
    assert_eq!(i.get(1, 0), Some(Color::BLACK));
    assert_eq!(i.get(4, 0), Some(Color::BLACK));
    // 3 倍: 各画素が 3x3 になる。
    let mut j = Image::filled(32, 32, Color::BLACK).unwrap();
    draw_glyph8(&mut j.canvas(), 2, 3, &A, 3, Color::WHITE);
    assert_eq!(count(&j) as u32, ones * 9);
    assert_eq!(j.get(2 + 2 * 3, 3), Some(Color::WHITE));
    assert_eq!(j.get(2 + 2 * 3 + 5, 3 + 2), Some(Color::WHITE));
    assert_eq!(j.get(2 + 4 * 3, 3), Some(Color::BLACK));
    // scale 0 は何もしない。
    let mut k = Image::filled(8, 8, Color::BLACK).unwrap();
    draw_glyph8(&mut k.canvas(), 0, 0, &A, 0, Color::WHITE);
    assert_eq!(count(&k), 0);
}

#[test]
fn glyph_clips_and_extremes() {
    let mut i = Image::filled(8, 8, Color::BLACK).unwrap();
    draw_glyph8(&mut i.canvas(), -4, -2, &A, 2, Color::WHITE);
    draw_glyph8(&mut i.canvas(), 6, 6, &A, 5, Color::WHITE);
    draw_glyph8(
        &mut i.canvas(),
        i32::MIN,
        i32::MAX,
        &A,
        u32::MAX,
        Color::WHITE,
    );
    draw_glyph8(
        &mut i.canvas(),
        i32::MAX,
        i32::MIN,
        &A,
        u32::MAX,
        Color::WHITE,
    );
    draw_glyph8(&mut i.canvas(), 0, 0, &A, u32::MAX, Color::WHITE); // 巨大スケールでも全面を塗るだけ
    assert!(count(&i) > 0);
}

#[test]
fn text_lookup_tofu_newline_and_measure() {
    let mut i = Image::filled(64, 32, Color::BLACK).unwrap();
    let (x, y) = draw_text8(&mut i.canvas(), 1, 1, "AA\nA", 2, Color::WHITE, |c| {
        (c == 'A').then_some(&A)
    });
    assert_eq!((x, y), (1 + 16, 1 + 16));
    let ones: u32 = A.iter().map(|b| b.count_ones()).sum();
    assert_eq!(count(&i) as u32, ones * 4 * 3);
    // 無い文字は豆腐 (四角の枠)。
    let mut j = Image::filled(16, 16, Color::BLACK).unwrap();
    draw_text8(&mut j.canvas(), 0, 0, "あ", 1, Color::WHITE, |_| None);
    assert_eq!(j.get(1, 1), Some(Color::WHITE));
    assert_eq!(j.get(3, 3), Some(Color::BLACK));
    assert_eq!(measure_text8("abc\nabcde", 2), 5 * 8 * 2);
    assert_eq!(measure_text8("", 3), 0);
    // 極端な位置・巨大な文字列でもパニックしない。
    let mut k = Image::filled(8, 8, Color::BLACK).unwrap();
    draw_text8(
        &mut k.canvas(),
        i32::MAX,
        i32::MIN,
        "abc\ndef",
        u32::MAX,
        Color::WHITE,
        |_| Some(&A),
    );
    let _ = Rect::EMPTY;
}

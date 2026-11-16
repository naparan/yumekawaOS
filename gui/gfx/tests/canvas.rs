//! Canvas / 図形 / 画像転送 / グラデーションのテスト。

// 2 次元配列を座標 (x, y) で添字アクセスするテストが多いので、この lint は無効にする。
#![allow(clippy::needless_range_loop)]

use yk_gfx::color::blend_over;
use yk_gfx::{BlendMode, Canvas, Color, Filter, Image, MaskRef, Rect};

const BG: Color = Color::rgb(10, 20, 30);
const FG: Color = Color::rgb(200, 100, 50);

fn img(w: u32, h: u32) -> Image {
    Image::filled(w, h, BG).unwrap()
}

fn px(i: &Image, x: u32, y: u32) -> Color {
    i.get(x, y).unwrap()
}

/// 画像の ASCII 表示 (FG のピクセルを '#', BG を '.', それ以外を '+')。
fn dump(i: &Image) -> Vec<String> {
    (0..i.height())
        .map(|y| {
            (0..i.width())
                .map(|x| match px(i, x, y) {
                    c if c == FG => '#',
                    c if c == BG => '.',
                    _ => '+',
                })
                .collect()
        })
        .collect()
}

// ---------------------------------------------------------------- 色

#[test]
fn color_basics() {
    let c = Color::argb(1, 2, 3, 4);
    assert_eq!((c.a(), c.r(), c.g(), c.b()), (1, 2, 3, 4));
    assert_eq!(c.0, 0x01020304);
    assert_eq!(Color::rgb(1, 2, 3).a(), 255);
    assert_eq!(Color::from_rgb_u32(0x123456), Color(0xFF123456));
    assert_eq!(c.with_alpha(9).a(), 9);
    assert_eq!(Color::rgb(0, 0, 0).mul_alpha(128).a(), 128);
    assert_eq!(Color::argb(200, 1, 1, 1).mul_alpha(255).a(), 200);
    assert_eq!(Color::argb(200, 1, 1, 1).mul_alpha(0).a(), 0);
}

#[test]
fn lerp_values() {
    let a = Color::argb(0, 0, 100, 255);
    let b = Color::argb(255, 200, 100, 0);
    assert_eq!(a.lerp(b, 0, 10), a);
    assert_eq!(a.lerp(b, 10, 10), b);
    assert_eq!(a.lerp(b, 5, 10), Color::argb(128, 100, 100, 128));
    // t > den は端に丸める。den == 0 でも panic しない。
    assert_eq!(a.lerp(b, 99, 10), b);
    let _ = a.lerp(b, 1, 0);
    // 巨大な den でもオーバーフローしない。
    assert_eq!(a.lerp(b, u32::MAX, u32::MAX), b);
    assert_eq!(a.lerp(b, u32::MAX / 2, u32::MAX).r(), 100);
}

#[test]
fn source_over_numbers() {
    // 不透明 dst
    let black = 0xFF000000;
    assert_eq!(blend_over(black, 0xFFFFFFFF), 0xFFFFFFFF); // 不透明 src
    assert_eq!(blend_over(black, 0x00FFFFFF), black); // 透明 src
    assert_eq!(blend_over(black, 0x80FFFFFF), 0xFF808080); // 50% 白 on 黒
    assert_eq!(blend_over(0xFF0000FF, 0x80FF0000), 0xFF80007F); // 50% 赤 on 青
    // 不透明な背景への合成結果はアルファ 255 のまま。
    for a in 0..=255u32 {
        let r = blend_over(0xFF336699, (a << 24) | 0x00FF8040);
        assert_eq!(r >> 24, 255);
    }
    // 透明 dst → src そのもの
    assert_eq!(blend_over(0x00000000, 0x80FFFFFF), 0x80FFFFFF);
    // 半透明 dst (一般式): a=128 白 over a=128 黒 → a=192, 値 170
    assert_eq!(blend_over(0x80000000, 0x80FFFFFF), 0xC0AAAAAA);
    // 単調性: src のアルファを上げるほど、結果は src に近づく (白 on 黒)
    let mut prev = 0;
    for a in 0..=255u32 {
        let r = blend_over(black, (a << 24) | 0x00FFFFFF) & 0xFF;
        assert!(r >= prev);
        prev = r;
    }
    assert_eq!(prev, 255);
}

#[test]
fn over_method_matches_function() {
    let dst = Color::rgb(1, 2, 3);
    let src = Color::argb(77, 200, 100, 50);
    assert_eq!(src.over(dst).0, blend_over(dst.0, src.0));
}

// ---------------------------------------------------------------- Canvas の構築

#[test]
fn canvas_new_validates_geometry() {
    let mut buf = vec![0u32; 100];
    assert!(Canvas::new(&mut buf, 10, 10, 10).is_ok());
    assert!(Canvas::new(&mut buf, 10, 10, 9).is_err()); // stride < width
    assert!(Canvas::new(&mut buf, 10, 11, 10).is_err()); // 足りない
    assert!(Canvas::new(&mut buf, 10, 9, 12).is_err()); // stride が大きいと足りなくなる
    assert!(Canvas::new(&mut buf, 8, 9, 12).is_err()); // 8 + 12*8 = 104 > 100
    assert!(Canvas::new(&mut buf, 8, 8, 12).is_ok()); // 8 + 12*7 = 92
    assert!(Canvas::new(&mut buf, 8, 2, usize::MAX).is_err()); // 乗算オーバーフロー
    assert!(Canvas::new(&mut buf, u32::MAX, 1, u32::MAX as usize).is_err());
    // 幅 0 / 高さ 0 は空のキャンバスとして有効 (何も描かれない)。
    let mut c = Canvas::new(&mut buf, 0, 5, 0).unwrap();
    c.fill_rect(Rect::new(0, 0, 10, 10), FG);
    assert!(buf.iter().all(|&p| p == 0));
}

#[test]
fn image_constructors_limits() {
    assert!(Image::new(0, 0).is_ok());
    assert!(Image::new(u32::MAX, u32::MAX).is_err());
    assert_eq!(
        Image::new(1 << 20, 1 << 20).unwrap_err(),
        yk_gfx::GfxError::TooLarge
    );
    assert_eq!(
        Image::new(i32::MAX as u32 + 1, 1).unwrap_err(),
        yk_gfx::GfxError::InvalidDimensions
    );
    assert!(Image::from_pixels(2, 2, vec![0; 3]).is_err());
    assert!(Image::from_pixels(2, 2, vec![0; 5]).is_err());
    assert!(Image::from_pixels(2, 2, vec![7; 4]).is_ok());
}

// ---------------------------------------------------------------- 矩形

#[test]
fn fill_rect_exact_pixels() {
    let mut i = img(8, 6);
    i.canvas().fill_rect(Rect::new(2, 1, 4, 3), FG);
    assert_eq!(
        dump(&i),
        [
            "........", "..####..", "..####..", "..####..", "........", "........"
        ]
    );
}

#[test]
fn fill_rect_clipping_cases() {
    // 負の座標: 左上がはみ出す。
    let mut i = img(6, 4);
    i.canvas().fill_rect(Rect::new(-3, -2, 5, 4), FG);
    assert_eq!(dump(&i), ["##....", "##....", "......", "......"]);
    // 右下がはみ出す。
    let mut i = img(6, 4);
    i.canvas().fill_rect(Rect::new(4, 3, 10, 10), FG);
    assert_eq!(dump(&i), ["......", "......", "......", "....##"]);
    // 完全に外 (4 方向)。
    for r in [
        Rect::new(-10, 0, 5, 5),
        Rect::new(6, 0, 5, 5),
        Rect::new(0, -10, 5, 5),
        Rect::new(0, 4, 5, 5),
    ] {
        let mut i = img(6, 4);
        i.canvas().fill_rect(r, FG);
        assert!(dump(&i).iter().all(|l| !l.contains('#')), "{r:?}");
    }
    // 巨大: 全面を塗る。i32 の端でもオーバーフローしない。
    let mut i = img(6, 4);
    i.canvas()
        .fill_rect(Rect::new(i32::MIN, i32::MIN, u32::MAX, u32::MAX), FG);
    assert!(dump(&i).iter().all(|l| l == "######"));
    let mut i = img(6, 4);
    i.canvas()
        .fill_rect(Rect::new(i32::MAX, i32::MAX, u32::MAX, u32::MAX), FG);
    assert!(dump(&i).iter().all(|l| l == "......"));
    let mut i = img(6, 4);
    i.canvas().fill_rect(Rect::new(i32::MAX - 2, 0, 10, 10), FG);
    assert!(dump(&i).iter().all(|l| l == "......"));
    // 空
    let mut i = img(6, 4);
    i.canvas().fill_rect(Rect::new(1, 1, 0, 3), FG);
    assert!(dump(&i).iter().all(|l| l == "......"));
}

#[test]
fn fill_rect_alpha_blends() {
    let mut i = img(2, 1);
    i.canvas()
        .fill_rect(Rect::new(0, 0, 1, 1), Color::argb(128, 255, 255, 255));
    i.canvas()
        .fill_rect(Rect::new(1, 0, 1, 1), Color::argb(0, 255, 255, 255));
    assert_eq!(px(&i, 0, 0), Color::rgb(133, 138, 143)); // 10 + (255-10)*128/255 等
    assert_eq!(px(&i, 1, 0), BG);
}

#[test]
fn clear_rect_replaces_without_blending() {
    let mut i = img(4, 1);
    i.canvas()
        .clear_rect(Rect::new(1, 0, 2, 1), Color::TRANSPARENT);
    assert_eq!(px(&i, 0, 0), BG);
    assert_eq!(px(&i, 1, 0), Color::TRANSPARENT);
    assert_eq!(px(&i, 2, 0), Color::TRANSPARENT);
    i.canvas().clear(Color::rgb(1, 1, 1));
    assert!((0..4).all(|x| px(&i, x, 0) == Color::rgb(1, 1, 1)));
}

#[test]
fn stroke_rect_pixels() {
    let mut i = img(8, 6);
    i.canvas().stroke_rect(Rect::new(1, 1, 6, 4), 1, FG);
    assert_eq!(
        dump(&i),
        [
            "........", ".######.", ".#....#.", ".#....#.", ".######.", "........"
        ]
    );
    let mut i = img(8, 6);
    i.canvas().stroke_rect(Rect::new(0, 0, 8, 6), 2, FG);
    assert_eq!(
        dump(&i),
        [
            "########", "########", "##....##", "##....##", "########", "########"
        ]
    );
    // 厚みが大きければ全面。0 なら何もしない。
    let mut i = img(8, 6);
    i.canvas().stroke_rect(Rect::new(0, 0, 8, 6), 3, FG);
    assert!(dump(&i).iter().all(|l| l == "########"));
    let mut i = img(8, 6);
    i.canvas().stroke_rect(Rect::new(0, 0, 8, 6), 0, FG);
    assert!(dump(&i).iter().all(|l| l == "........"));
    // 枠が画面外にはみ出しても、見えている辺だけが描かれる。
    let mut i = img(8, 6);
    i.canvas().stroke_rect(Rect::new(-4, -3, 8, 6), 1, FG);
    // x = -4..3 の右辺 (x=3) が行 0,1 に、下辺 (y=2) が x=0..3 に見える。
    assert_eq!(
        dump(&i)[..4],
        ["...#....", "...#....", "####....", "........"]
    );
    let mut i = img(5, 5);
    i.canvas().stroke_rect(
        Rect::new(i32::MIN, i32::MIN, u32::MAX, u32::MAX),
        u32::MAX,
        FG,
    );
}

#[test]
fn hline_vline() {
    let mut i = img(6, 4);
    {
        let mut c = i.canvas();
        c.hline(1, 1, 3, FG);
        c.vline(5, -2, 4, FG);
        c.hline(-5, 3, 7, FG);
        c.hline(0, 0, u32::MAX, FG);
    }
    // vline(5, -2, 4) は y=-2..2 なので画面内は行 0,1 のみ。hline(-5, 3, 7) は x=-5..2 なので列 0,1 のみ。
    assert_eq!(dump(&i), ["######", ".###.#", "......", "##...."]);
}

// ---------------------------------------------------------------- ピクセル

#[test]
fn pixel_get_set_blend_with_clip() {
    let mut i = img(4, 4);
    let mut c = i.canvas();
    assert!(c.set_pixel(1, 1, FG));
    assert!(!c.set_pixel(-1, 0, FG));
    assert!(!c.set_pixel(4, 0, FG));
    assert!(!c.set_pixel(0, i32::MAX, FG));
    assert_eq!(c.get_pixel(1, 1), Some(FG));
    assert_eq!(c.get_pixel(-1, 1), None);
    assert_eq!(c.get_pixel(1, 4), None);
    c.set_clip(Rect::new(0, 0, 2, 2));
    assert!(!c.set_pixel(3, 3, FG));
    assert!(c.blend_pixel(0, 0, Color::argb(255, 1, 2, 3)));
    assert!(!c.blend_pixel(2, 2, FG));
    // クリップは get_pixel には影響しない (範囲のみ)。
    assert_eq!(c.get_pixel(3, 3), Some(BG));
    // クリップはキャンバス範囲に切り詰められる。
    c.set_clip(Rect::new(-100, -100, 1000, 1000));
    assert_eq!(c.clip(), Rect::new(0, 0, 4, 4));
    c.set_clip(Rect::new(10, 10, 5, 5));
    assert!(c.clip().is_empty());
    c.fill_rect(Rect::new(0, 0, 4, 4), FG);
    c.reset_clip();
    assert_eq!(c.clip(), Rect::new(0, 0, 4, 4));
    assert_eq!(px(&i, 0, 0), Color::rgb(1, 2, 3));
    assert_eq!(px(&i, 3, 3), BG);
}

#[test]
fn clip_restricts_every_primitive() {
    let mut i = img(10, 10);
    {
        let mut c = i.canvas();
        c.set_clip(Rect::new(3, 3, 4, 4));
        c.fill_rect(Rect::new(0, 0, 10, 10), FG);
        c.line(0, 0, 9, 9, 1, Color::WHITE);
        c.fill_ellipse(Rect::new(-20, -20, 60, 60), Color::WHITE);
        c.fill_round_rect(Rect::new(-20, -20, 60, 60), 8, Color::WHITE);
        c.fill_gradient_v(Rect::new(0, 0, 10, 10), Color::WHITE, Color::BLACK);
    }
    for y in 0..10 {
        for x in 0..10 {
            let inside = (3..7).contains(&x) && (3..7).contains(&y);
            assert_eq!(px(&i, x, y) != BG, inside, "({x},{y})");
        }
    }
}

// ---------------------------------------------------------------- 線

#[test]
fn line_1px_pixel_patterns() {
    let mut i = img(8, 8);
    i.canvas().line(0, 0, 7, 7, 1, FG);
    assert_eq!(
        dump(&i),
        [
            "#.......", ".#......", "..#.....", "...#....", "....#...", ".....#..", "......#.",
            ".......#"
        ]
    );
    // 緩い傾き (0,0)-(10,4): y = round(4i/10)
    let mut i = img(11, 5);
    i.canvas().line(0, 0, 10, 4, 1, FG);
    let ys = [0, 0, 1, 1, 2, 2, 2, 3, 3, 4, 4];
    for (x, &y) in ys.iter().enumerate() {
        for yy in 0..5 {
            assert_eq!(px(&i, x as u32, yy) == FG, yy == y, "x={x} y={yy}");
        }
    }
    // 急な傾き (主軸が y)
    let mut i = img(5, 11);
    i.canvas().line(0, 0, 4, 10, 1, FG);
    for (y, &x) in ys.iter().enumerate() {
        for xx in 0..5 {
            assert_eq!(px(&i, xx, y as u32) == FG, xx == x, "x={xx} y={y}");
        }
    }
    // 点 (x0==x1, y0==y1)
    let mut i = img(3, 3);
    i.canvas().line(1, 1, 1, 1, 1, FG);
    assert_eq!(dump(&i), ["...", ".#.", "..."]);
}

#[test]
fn line_1px_properties_all_directions() {
    // 端点を含み、画素数 = max(|dx|,|dy|)+1、8 連結。
    let pts = [(2, 3), (20, 7), (11, 30), (2, 3), (30, 30)];
    for &(x0, y0) in &pts[..2] {
        for &(x1, y1) in &pts {
            let mut i = img(40, 40);
            i.canvas().line(x0, y0, x1, y1, 1, FG);
            let on: Vec<(i32, i32)> = (0..40)
                .flat_map(|y| (0..40).map(move |x| (x, y)))
                .filter(|&(x, y)| px(&i, x as u32, y as u32) == FG)
                .collect();
            let n = ((x1 - x0).abs().max((y1 - y0).abs()) + 1) as usize;
            assert_eq!(on.len(), n, "{x0},{y0}-{x1},{y1}");
            assert!(on.contains(&(x0, y0)) && on.contains(&(x1, y1)));
        }
    }
}

#[test]
fn line_clipping_and_huge_coordinates() {
    // 両端が画面外でも、通過する部分は描かれる。
    let mut i = img(10, 10);
    i.canvas().line(-1000, -1000, 1000, 1000, 1, FG);
    for k in 0..10 {
        assert_eq!(px(&i, k, k), FG);
    }
    assert_eq!(
        dump(&i)
            .iter()
            .map(|l| l.matches('#').count())
            .sum::<usize>(),
        10
    );
    // 画面外だけを通る線は何も描かない。
    let mut i = img(10, 10);
    i.canvas().line(-50, -50, -10, 100, 1, FG);
    i.canvas().line(20, 0, 20, 99, 1, FG);
    assert!(dump(&i).iter().all(|l| !l.contains('#')));
    // i32 の端から端まで (時間がかからず、パニックしない)。
    let mut i = img(64, 64);
    i.canvas()
        .line(i32::MIN, i32::MIN, i32::MAX, i32::MAX, 1, FG);
    i.canvas().line(i32::MIN, 0, i32::MAX, 63, 1, FG);
    i.canvas().line(0, i32::MIN, 63, i32::MAX, 1, FG);
    i.canvas()
        .line(i32::MAX, i32::MAX, i32::MIN, i32::MIN, 3, FG);
    i.canvas()
        .line_aa(i32::MIN, i32::MIN, i32::MAX, i32::MAX, 5, FG);
    // 主対角線のはず。
    assert_eq!(px(&i, 30, 30), FG);
}

#[test]
fn thick_lines() {
    // 軸平行 (厚み 3): 中心線を挟んだ矩形。
    let mut i = img(10, 7);
    i.canvas().line(1, 3, 8, 3, 3, FG);
    assert_eq!(
        dump(&i),
        [
            "..........",
            "..........",
            ".########.",
            ".########.",
            ".########.",
            "..........",
            ".........."
        ]
    );
    let mut i = img(7, 10);
    i.canvas().line(3, 8, 3, 1, 2, FG);
    // 厚み 2: 中心から左に 0、右に 1 (x = 3 - 0 .. 3+2)
    for y in 1..=8 {
        assert_eq!(
            (px(&i, 2, y), px(&i, 3, y), px(&i, 4, y)),
            (BG, FG, FG),
            "y={y}"
        );
    }
    assert_eq!(px(&i, 3, 0), BG);
    assert_eq!(px(&i, 3, 9), BG);

    // 斜め (AA): 中心は完全に塗られ、3px 以上離れると触れない。上下左右対称。
    let mut i = Image::filled(30, 30, Color::BLACK).unwrap();
    i.canvas().line(5, 5, 24, 24, 4, Color::WHITE);
    assert_eq!(px(&i, 15, 15), Color::WHITE);
    assert_eq!(px(&i, 14, 15), Color::WHITE);
    assert_eq!(px(&i, 15, 20).r(), 0);
    assert_eq!(px(&i, 25, 5).r(), 0);
    // 中心線から 1.5px (斜め距離) 内側のピクセルは明るく、2.5px 外は暗い: 勾配が単調。
    let row: Vec<u8> = (0..30).map(|x| px(&i, x, 15).r()).collect();
    let peak = row.iter().position(|&v| v == 255).unwrap();
    assert!((13..=17).contains(&peak));
    for x in peak..29 {
        if row[x] != 255 {
            assert!(row[x + 1] <= row[x], "非単調: {row:?}");
        }
    }
    // 端点は丸い: 始点の外側 (-1,-1) 方向にも少し塗られる。
    assert!(px(&i, 4, 4).r() > 0);
}

#[test]
fn line_aa_is_translation_invariant_and_symmetric() {
    let mut a = Image::filled(40, 40, Color::BLACK).unwrap();
    let mut b = Image::filled(40, 40, Color::BLACK).unwrap();
    a.canvas().line_aa(5, 7, 30, 18, 3, Color::WHITE);
    b.canvas().line_aa(8, 10, 33, 21, 3, Color::WHITE);
    for y in 0..30 {
        for x in 0..30 {
            assert_eq!(px(&a, x, y), px(&b, x + 3, y + 3));
        }
    }
    // 厚み 0 は何もしない。
    let mut c = Image::filled(10, 10, Color::BLACK).unwrap();
    c.canvas().line_aa(0, 0, 9, 9, 0, Color::WHITE);
    c.canvas().line(0, 0, 9, 9, 0, Color::WHITE);
    assert!(c.pixels().iter().all(|&p| p == Color::BLACK.0));
}

// ---------------------------------------------------------------- 角丸矩形

fn alpha_map(i: &Image) -> Vec<Vec<u8>> {
    (0..i.height())
        .map(|y| (0..i.width()).map(|x| px(i, x, y).r()).collect())
        .collect()
}

fn assert_symmetric(m: &[Vec<u8>], w: usize, h: usize) {
    for y in 0..h {
        for x in 0..w {
            assert_eq!(m[y][x], m[y][w - 1 - x], "左右対称 ({x},{y})");
            assert_eq!(m[y][x], m[h - 1 - y][x], "上下対称 ({x},{y})");
        }
    }
}

#[test]
fn round_rect_fill_shape() {
    let mut i = Image::filled(24, 24, Color::BLACK).unwrap();
    i.canvas()
        .fill_round_rect(Rect::new(2, 2, 20, 20), 6, Color::WHITE);
    let m = alpha_map(&i);
    // 矩形の外は触れない。
    for y in 0..24 {
        for x in 0..24 {
            if !(2..22).contains(&x) || !(2..22).contains(&y) {
                assert_eq!(m[y][x], 0);
            }
        }
    }
    // 辺の中央は完全に塗られ、角の外側 (2,2) は塗られず、角の内側 (8,8) は塗られる。
    assert_eq!(m[2][12], 255);
    assert_eq!(m[12][2], 255);
    assert_eq!(m[2][2], 0);
    assert_eq!(m[21][21], 0);
    assert_eq!(m[8][8], 255);
    // 角に部分的なカバレッジのピクセルがある (アンチエイリアス)。
    let partial = m.iter().flatten().filter(|&&v| v > 0 && v < 255).count();
    assert!(partial >= 12, "partial pixels = {partial}");
    // 4 回対称 (矩形の中心 (12,12) 周り)。
    let sub: Vec<Vec<u8>> = (2..22).map(|y| m[y][2..22].to_vec()).collect();
    assert_symmetric(&sub, 20, 20);
    // 面積は 20*20 - (4 - π) r² ≈ 400 - 0.8584*36 = 369.1 付近。
    let area: f64 = m.iter().flatten().map(|&v| v as f64 / 255.0).sum();
    assert!((area - 369.1).abs() < 3.0, "area = {area}");
}

#[test]
fn round_rect_radius_clamp_and_zero() {
    // radius 0 は fill_rect と同じ。
    let mut a = img(12, 12);
    let mut b = img(12, 12);
    a.canvas().fill_round_rect(Rect::new(1, 2, 9, 7), 0, FG);
    b.canvas().fill_rect(Rect::new(1, 2, 9, 7), FG);
    assert_eq!(a, b);
    // radius が大きすぎる → min(w,h)/2 に制限され、円に近い形 (パニックしない)。
    let mut c = Image::filled(20, 20, Color::BLACK).unwrap();
    c.canvas()
        .fill_round_rect(Rect::new(0, 0, 20, 20), 1_000_000, Color::WHITE);
    let m = alpha_map(&c);
    assert_symmetric(&m, 20, 20);
    assert_eq!(m[0][0], 0);
    assert_eq!(m[10][10], 255);
    let area: f64 = m.iter().flatten().map(|&v| v as f64 / 255.0).sum();
    assert!(
        (area - core::f64::consts::PI * 100.0).abs() < 4.0,
        "area = {area}"
    );
    // 巨大な矩形・巨大な radius。
    let mut d = img(30, 30);
    d.canvas().fill_round_rect(
        Rect::new(i32::MIN, i32::MIN, u32::MAX, u32::MAX),
        u32::MAX,
        FG,
    );
    d.canvas().stroke_round_rect(
        Rect::new(i32::MIN, -5, u32::MAX, 40),
        u32::MAX,
        u32::MAX,
        FG,
    );
    d.canvas()
        .stroke_round_rect(Rect::new(-5, -5, 100, 100), 20, 7, FG);
}

#[test]
fn round_rect_stroke_ring() {
    let mut i = Image::filled(30, 30, Color::BLACK).unwrap();
    i.canvas()
        .stroke_round_rect(Rect::new(1, 1, 28, 28), 8, 3, Color::WHITE);
    let m = alpha_map(&i);
    // 辺の中央: 外側 3px が塗られ、内側は空洞。
    for k in 1..4 {
        assert_eq!(m[k][15], 255, "上辺 {k}");
        assert_eq!(m[15][k], 255, "左辺 {k}");
    }
    assert_eq!(m[4][15], 0);
    assert_eq!(m[15][15], 0);
    assert_eq!(m[0][15], 0);
    assert_eq!(m[1][1], 0); // 角の外
    let sub: Vec<Vec<u8>> = (1..29).map(|y| m[y][1..29].to_vec()).collect();
    assert_symmetric(&sub, 28, 28);
    // 厚み >= 半分 → 塗りつぶしと同じ。
    let mut a = Image::filled(20, 20, Color::BLACK).unwrap();
    let mut b = a.clone();
    a.canvas()
        .stroke_round_rect(Rect::new(0, 0, 20, 20), 5, 10, Color::WHITE);
    b.canvas()
        .fill_round_rect(Rect::new(0, 0, 20, 20), 5, Color::WHITE);
    assert_eq!(a, b);
    // 半透明色: 重なるピクセルが二重に合成されない (帯と角の境目で明るさが飛ばない)。
    let mut c = Image::filled(30, 30, Color::BLACK).unwrap();
    c.canvas().stroke_round_rect(
        Rect::new(1, 1, 28, 28),
        8,
        3,
        Color::argb(128, 255, 255, 255),
    );
    assert!(alpha_map(&c).iter().flatten().all(|&v| v <= 128));
}

#[test]
fn round_rect_gradients() {
    let r = Rect::new(2, 2, 36, 26);
    // 同色のグラデーションは塗りつぶしと完全に一致する (形・AA が同じ)。
    let mut a = Image::filled(40, 30, Color::BLACK).unwrap();
    let mut b = a.clone();
    let mut c = a.clone();
    a.canvas().fill_round_rect(r, 9, Color::WHITE);
    b.canvas()
        .fill_round_rect_gradient_v(r, 9, Color::WHITE, Color::WHITE);
    c.canvas()
        .fill_round_rect_gradient_h(r, 9, Color::WHITE, Color::WHITE);
    assert_eq!(a, b);
    assert_eq!(a, c);
    // 縦グラデーション: 上端の行は top、下端の行は bottom の色 (辺の中央で確認)。
    let mut d = Image::filled(40, 30, Color::BLACK).unwrap();
    d.canvas()
        .fill_round_rect_gradient_v(r, 9, Color::rgb(255, 0, 0), Color::rgb(0, 0, 255));
    assert_eq!(px(&d, 20, 2), Color::rgb(255, 0, 0));
    assert_eq!(px(&d, 20, 27), Color::rgb(0, 0, 255));
    assert_eq!(px(&d, 2, 2).r(), 0); // 角の外
    // 横グラデーション: 左端の列は left、右端の列は right。
    let mut e = Image::filled(40, 30, Color::BLACK).unwrap();
    e.canvas()
        .fill_round_rect_gradient_h(r, 9, Color::rgb(255, 0, 0), Color::rgb(0, 0, 255));
    assert_eq!(px(&e, 2, 15), Color::rgb(255, 0, 0));
    assert_eq!(px(&e, 37, 15), Color::rgb(0, 0, 255));
    // 上だけ角丸のバー: 角丸矩形を下に伸ばして、バーの高さにクリップする。
    let mut f = Image::filled(40, 30, Color::BLACK).unwrap();
    {
        let mut cv = f.canvas();
        cv.set_clip(Rect::new(2, 2, 36, 10));
        cv.fill_round_rect_gradient_h(Rect::new(2, 2, 36, 40), 9, Color::WHITE, Color::WHITE);
    }
    assert_eq!(px(&f, 2, 2).r(), 0); // 上の角は丸い
    assert_eq!(px(&f, 2, 11), Color::WHITE); // 下の端は角ばっている
    assert_eq!(px(&f, 20, 12).r(), 0); // クリップの外
    // 透明なグラデーションは何も描かない。極端な値でもパニックしない。
    let mut g = Image::filled(8, 8, Color::BLACK).unwrap();
    g.canvas().fill_round_rect_gradient_v(
        Rect::new(0, 0, 8, 8),
        3,
        Color::TRANSPARENT,
        Color::TRANSPARENT,
    );
    assert!(g.pixels().iter().all(|&p| p == Color::BLACK.0));
    g.canvas().fill_round_rect_gradient_h(
        Rect::new(i32::MIN, i32::MIN, u32::MAX, u32::MAX),
        u32::MAX,
        Color::WHITE,
        Color::argb(10, 0, 0, 0),
    );
    g.canvas().fill_round_rect_gradient_v(
        Rect::new(i32::MAX - 1, 3, 9, 9),
        4,
        Color::WHITE,
        Color::BLACK,
    );
}

// ---------------------------------------------------------------- 円・楕円

#[test]
fn ellipse_circle_area_and_symmetry() {
    for d in [5u32, 10, 21, 40] {
        let mut i = Image::filled(d + 4, d + 4, Color::BLACK).unwrap();
        i.canvas().fill_ellipse(Rect::new(2, 2, d, d), Color::WHITE);
        let m = alpha_map(&i);
        let sub: Vec<Vec<u8>> = (2..2 + d as usize)
            .map(|y| m[y][2..2 + d as usize].to_vec())
            .collect();
        assert_symmetric(&sub, d as usize, d as usize);
        let area: f64 = m.iter().flatten().map(|&v| v as f64 / 255.0).sum();
        let want = core::f64::consts::PI * (d as f64 / 2.0).powi(2);
        assert!(
            (area - want).abs() / want < 0.02,
            "d={d}: area={area} want={want}"
        );
        // 枠の外は触れない。
        for (y, row) in m.iter().enumerate() {
            for (x, &v) in row.iter().enumerate() {
                if x < 2 || y < 2 || x >= 2 + d as usize || y >= 2 + d as usize {
                    assert_eq!(v, 0);
                }
            }
        }
    }
}

#[test]
fn ellipse_non_square_and_stroke() {
    let mut i = Image::filled(50, 30, Color::BLACK).unwrap();
    i.canvas()
        .fill_ellipse(Rect::new(5, 5, 40, 20), Color::WHITE);
    let area: f64 = alpha_map(&i)
        .iter()
        .flatten()
        .map(|&v| v as f64 / 255.0)
        .sum();
    let want = core::f64::consts::PI * 20.0 * 10.0;
    assert!((area - want).abs() / want < 0.02, "area={area} want={want}");
    assert_eq!(px(&i, 25, 15), Color::WHITE);
    assert_eq!(px(&i, 5, 5).r(), 0);

    // 枠: 中心は空洞、枠上は塗られる、面積は外 - 内。
    let mut j = Image::filled(50, 50, Color::BLACK).unwrap();
    j.canvas()
        .stroke_ellipse(Rect::new(5, 5, 40, 40), 4, Color::WHITE);
    let m = alpha_map(&j);
    assert_eq!(m[25][25], 0);
    assert_eq!(m[25][6], 255);
    assert_eq!(m[6][25], 255);
    assert_eq!(m[25][12], 0);
    let area: f64 = m.iter().flatten().map(|&v| v as f64 / 255.0).sum();
    let want = core::f64::consts::PI * (20.0f64.powi(2) - 16.0f64.powi(2));
    assert!((area - want).abs() / want < 0.03, "area={area} want={want}");
    let sub: Vec<Vec<u8>> = (5..45).map(|y| m[y][5..45].to_vec()).collect();
    assert_symmetric(&sub, 40, 40);
    // 厚みが半径以上 → 塗りつぶしと同じ。
    let mut a = Image::filled(30, 30, Color::BLACK).unwrap();
    let mut b = a.clone();
    a.canvas()
        .stroke_ellipse(Rect::new(0, 0, 30, 30), 99, Color::WHITE);
    b.canvas()
        .fill_ellipse(Rect::new(0, 0, 30, 30), Color::WHITE);
    assert_eq!(a, b);
}

#[test]
fn circle_helpers_and_degenerate() {
    let mut i = Image::filled(20, 20, Color::BLACK).unwrap();
    i.canvas().fill_circle(10, 10, 5, Color::WHITE);
    assert_eq!(px(&i, 10, 10), Color::WHITE);
    assert_eq!(px(&i, 10, 4).r(), 0);
    assert_eq!(px(&i, 10, 5), Color::WHITE);
    // 半径 0・幅 1 の楕円・巨大な楕円でもパニックしない。
    let mut j = img(16, 16);
    {
        let mut c = j.canvas();
        c.fill_circle(5, 5, 0, FG);
        c.fill_ellipse(Rect::new(3, 3, 1, 9), FG);
        c.fill_ellipse(Rect::new(i32::MIN, i32::MIN, u32::MAX, u32::MAX), FG);
        c.stroke_ellipse(
            Rect::new(i32::MIN, i32::MIN, u32::MAX, u32::MAX),
            u32::MAX,
            FG,
        );
        c.stroke_circle(i32::MAX, i32::MAX, u32::MAX, 3, FG);
        c.fill_circle(i32::MIN, 0, u32::MAX, FG);
    }
}

// ---------------------------------------------------------------- グラデーション

#[test]
fn gradient_vertical_and_horizontal() {
    let top = Color::rgb(255, 0, 0);
    let bot = Color::rgb(0, 0, 255);
    let mut i = img(4, 11);
    i.canvas().fill_gradient_v(Rect::new(0, 0, 4, 11), top, bot);
    assert_eq!(px(&i, 0, 0), top);
    assert_eq!(px(&i, 3, 10), bot);
    assert_eq!(px(&i, 2, 5), Color::rgb(128, 0, 128));
    // 行内は一様、行ごとに単調。
    for y in 0..11 {
        assert!((1..4).all(|x| px(&i, x, y) == px(&i, 0, y)));
        if y > 0 {
            assert!(px(&i, 0, y).r() <= px(&i, 0, y - 1).r());
            assert!(px(&i, 0, y).b() >= px(&i, 0, y - 1).b());
        }
    }
    let mut j = img(11, 3);
    j.canvas().fill_gradient_h(Rect::new(0, 0, 11, 3), top, bot);
    assert_eq!(px(&j, 0, 1), top);
    assert_eq!(px(&j, 10, 2), bot);
    assert_eq!(px(&j, 5, 0), Color::rgb(128, 0, 128));
    // 高さ/幅 1 は始点の色。
    let mut k = img(3, 3);
    k.canvas().fill_gradient_v(Rect::new(0, 1, 3, 1), top, bot);
    assert_eq!(px(&k, 1, 1), top);
    k.canvas().fill_gradient_h(Rect::new(1, 0, 1, 3), bot, top);
    assert_eq!(px(&k, 1, 0), bot);
}

#[test]
fn gradient_is_independent_of_clipping() {
    let a = Color::rgb(255, 214, 232);
    let b = Color::rgb(204, 245, 233);
    let r = Rect::new(-20, -10, 120, 80);
    let mut full = Image::filled(100, 70, Color::BLACK).unwrap();
    full.canvas().fill_gradient_v(r, a, b);
    let mut clipped = Image::filled(100, 70, Color::BLACK).unwrap();
    {
        let mut c = clipped.canvas();
        c.set_clip(Rect::new(30, 20, 40, 30));
        c.fill_gradient_v(r, a, b);
    }
    for y in 20..50 {
        for x in 30..70 {
            assert_eq!(px(&full, x, y), px(&clipped, x, y));
        }
    }
    assert_eq!(px(&clipped, 0, 0), Color::BLACK);
    // 横方向も同様。
    let mut f2 = Image::filled(100, 70, Color::BLACK).unwrap();
    f2.canvas().fill_gradient_h(r, a, b);
    let mut c2 = Image::filled(100, 70, Color::BLACK).unwrap();
    {
        let mut c = c2.canvas();
        c.set_clip(Rect::new(30, 20, 40, 30));
        c.fill_gradient_h(r, a, b);
    }
    for y in 20..50 {
        for x in 30..70 {
            assert_eq!(px(&f2, x, y), px(&c2, x, y));
        }
    }
    // 半透明グラデーションは合成される。巨大矩形でもパニックしない。
    let mut d = img(8, 8);
    d.canvas().fill_gradient_h(
        Rect::new(i32::MIN, i32::MIN, u32::MAX, u32::MAX),
        Color::argb(0, 255, 255, 255),
        Color::argb(255, 255, 255, 255),
    );
    d.canvas().fill_gradient_v(
        Rect::new(i32::MIN, i32::MIN, u32::MAX, u32::MAX),
        Color::TRANSPARENT,
        Color::WHITE,
    );
}

// ---------------------------------------------------------------- 画像転送

fn pattern(w: u32, h: u32) -> Image {
    let mut i = Image::new(w, h).unwrap();
    for y in 0..h {
        for x in 0..w {
            i.pixels_mut()[(y * w + x) as usize] =
                Color::argb(255, x as u8 * 10, y as u8 * 10, 7).0;
        }
    }
    i
}

#[test]
fn blit_copy_and_region_and_clipping() {
    let src = pattern(5, 4);
    let mut dst = img(8, 8);
    dst.canvas().blit(&src.view(), 2, 3);
    for y in 0..8u32 {
        for x in 0..8u32 {
            let inside = (2..7).contains(&x) && (3..7).contains(&y);
            let want = if inside { px(&src, x - 2, y - 3) } else { BG };
            assert_eq!(px(&dst, x, y), want, "({x},{y})");
        }
    }
    // 部分矩形: src の (1,1) 3x2 を (0,0) へ。
    let mut dst = img(4, 4);
    dst.canvas()
        .blit_region(&src.view(), Rect::new(1, 1, 3, 2), 0, 0);
    assert_eq!(px(&dst, 0, 0), px(&src, 1, 1));
    assert_eq!(px(&dst, 2, 1), px(&src, 3, 2));
    assert_eq!(px(&dst, 3, 0), BG);
    assert_eq!(px(&dst, 0, 2), BG);
    // 左上にはみ出す配置 / 右下にはみ出す配置。
    let mut dst = img(4, 4);
    dst.canvas().blit(&src.view(), -2, -1);
    assert_eq!(px(&dst, 0, 0), px(&src, 2, 1));
    assert_eq!(px(&dst, 2, 2), px(&src, 4, 3));
    assert_eq!(px(&dst, 3, 3), BG);
    let mut dst = img(4, 4);
    dst.canvas().blit(&src.view(), 3, 3);
    assert_eq!(px(&dst, 3, 3), px(&src, 0, 0));
    // 範囲外・極端な値・src_rect がソース外にはみ出す場合。
    let mut dst = img(4, 4);
    {
        let mut c = dst.canvas();
        c.blit(&src.view(), i32::MAX, i32::MAX);
        c.blit(&src.view(), i32::MIN, i32::MIN);
        c.blit_region(
            &src.view(),
            Rect::new(i32::MIN, i32::MIN, u32::MAX, u32::MAX),
            0,
            0,
        );
        c.blit_region(&src.view(), Rect::new(i32::MAX, 0, 5, 5), 0, 0);
        c.blit_region(&src.view(), Rect::new(-2, -2, 4, 4), 1, 1); // (0,0)-(2,2) が (3,3) へ
    }
    // 巨大な src_rect (原点 i32::MIN) は、ソースの実画素を遠く (x,y = 2^31) に置くので画面には出ない。
    assert_eq!(px(&dst, 0, 0), BG);
    assert_eq!(
        px(&dst, 3, 3),
        px(&src, 0, 0) /* 最後の blit_region */
    );
}

#[test]
fn blit_alpha_and_opacity() {
    let mut src = Image::new(3, 1).unwrap();
    src.pixels_mut()
        .copy_from_slice(&[0xFFFF0000, 0x80FF0000, 0x00FF0000]);
    let mut d = Image::filled(3, 1, Color::rgb(0, 0, 255)).unwrap();
    d.canvas().blit_alpha(&src.view(), 0, 0);
    assert_eq!(px(&d, 0, 0), Color::rgb(255, 0, 0));
    assert_eq!(px(&d, 1, 0), Color::rgb(128, 0, 127));
    assert_eq!(px(&d, 2, 0), Color::rgb(0, 0, 255));
    // 全体アルファ 50%: 不透明の赤が半分になる。
    let mut d = Image::filled(3, 1, Color::rgb(0, 0, 255)).unwrap();
    d.canvas().blit_opacity(&src.view(), 0, 0, 128);
    assert_eq!(px(&d, 0, 0), Color::rgb(128, 0, 127));
    assert_eq!(px(&d, 1, 0), Color::rgb(64, 0, 191));
    // opacity 0 → 何もしない。255 → blit_alpha と同じ。
    let mut e = Image::filled(3, 1, Color::rgb(0, 0, 255)).unwrap();
    e.canvas().blit_opacity(&src.view(), 0, 0, 0);
    assert!(e.pixels().iter().all(|&p| p == Color::rgb(0, 0, 255).0));
    // Copy は不透明コピー (アルファ 0 のピクセルも上書き)。
    let mut f = Image::filled(3, 1, Color::rgb(0, 0, 255)).unwrap();
    f.canvas().blit(&src.view(), 0, 0);
    assert_eq!(f.pixels(), src.pixels());
}

#[test]
fn image_multiply_alpha_and_crop() {
    let mut i = Image::filled(2, 2, Color::argb(200, 10, 20, 30)).unwrap();
    i.multiply_alpha(128);
    assert_eq!(px(&i, 0, 0).a(), 100); // 200*128/255 = 100.39
    i.multiply_alpha(255);
    assert_eq!(px(&i, 1, 1).a(), 100);
    let p = pattern(6, 6);
    let c = p.crop(Rect::new(2, 3, 10, 10)).unwrap();
    assert_eq!((c.width(), c.height()), (4, 3));
    assert_eq!(px(&c, 0, 0), px(&p, 2, 3));
    assert_eq!(px(&c, 3, 2), px(&p, 5, 5));
    let e = p.crop(Rect::new(100, 100, 5, 5)).unwrap();
    assert_eq!((e.width(), e.height()), (0, 0));
}

#[test]
fn copy_within_overlapping_scroll() {
    // 縦スクロール: 下へ 2 行 / 上へ 2 行 (重なりあり)
    let p = pattern(6, 8);
    let mut a = p.clone();
    a.canvas().copy_within(Rect::new(0, 0, 6, 6), 0, 2);
    for y in 2..8 {
        for x in 0..6 {
            assert_eq!(px(&a, x, y), px(&p, x, y - 2));
        }
    }
    assert_eq!(px(&a, 0, 0), px(&p, 0, 0));
    let mut b = p.clone();
    b.canvas().copy_within(Rect::new(0, 2, 6, 6), 0, 0);
    for y in 0..6 {
        for x in 0..6 {
            assert_eq!(px(&b, x, y), px(&p, x, y + 2));
        }
    }
    // 横スクロール (同じ行内の重なり)
    let mut c = p.clone();
    c.canvas().copy_within(Rect::new(0, 0, 5, 8), 1, 0);
    for y in 0..8 {
        for x in 1..6 {
            assert_eq!(px(&c, x, y), px(&p, x - 1, y));
        }
    }
    let mut d = p.clone();
    d.canvas().copy_within(Rect::new(1, 0, 5, 8), 0, 0);
    for y in 0..8 {
        for x in 0..5 {
            assert_eq!(px(&d, x, y), px(&p, x + 1, y));
        }
    }
    // はみ出しや極端な値でもパニックしない。
    let mut e = p.clone();
    e.canvas().copy_within(Rect::new(-5, -5, 100, 100), -3, 4);
    e.canvas()
        .copy_within(Rect::new(0, 0, 6, 8), i32::MAX, i32::MIN);
    e.canvas()
        .copy_within(Rect::new(i32::MIN, i32::MIN, u32::MAX, u32::MAX), 0, 0);
}

#[test]
fn sub_canvas_is_isolated_and_translated() {
    let mut i = img(10, 10);
    {
        let mut c = i.canvas();
        let mut s = c.sub_canvas(Rect::new(3, 4, 4, 3)).unwrap();
        assert_eq!((s.width(), s.height()), (4, 3));
        s.fill_rect(Rect::new(-5, -5, 100, 100), FG); // 子の範囲にクリップされる
        s.set_pixel(0, 0, Color::WHITE);
        assert!(c.sub_canvas(Rect::new(50, 50, 4, 4)).is_none());
        // はみ出す子は親の範囲に切り詰め。
        let s2 = c.sub_canvas(Rect::new(8, 8, 10, 10)).unwrap();
        assert_eq!((s2.width(), s2.height()), (2, 2));
    }
    for y in 0..10 {
        for x in 0..10 {
            let inside = (3..7).contains(&x) && (4..7).contains(&y);
            let want = if (x, y) == (3, 4) {
                Color::WHITE
            } else if inside {
                FG
            } else {
                BG
            };
            assert_eq!(px(&i, x, y), want, "({x},{y})");
        }
    }
}

#[test]
fn scaled_nearest_integer_factors() {
    let src = pattern(3, 2);
    // 3x に拡大
    let up = src.scaled(9, 6, Filter::Nearest).unwrap();
    for y in 0..6 {
        for x in 0..9 {
            assert_eq!(px(&up, x, y), px(&src, x / 3, y / 3));
        }
    }
    // 1/3 に縮小 (中央の画素を採る)
    let big = pattern(9, 6);
    let down = big.scaled(3, 2, Filter::Nearest).unwrap();
    for y in 0..2 {
        for x in 0..3 {
            assert_eq!(px(&down, x, y), px(&big, x * 3 + 1, y * 3 + 1));
        }
    }
    // 等倍は恒等。
    assert_eq!(src.scaled(3, 2, Filter::Nearest).unwrap(), src);
    assert_eq!(src.scaled(3, 2, Filter::Bilinear).unwrap(), src);
}

#[test]
fn scaled_bilinear_properties() {
    // 単色は単色のまま (どの倍率でも)。
    let flat = Image::filled(5, 5, Color::argb(200, 90, 120, 150)).unwrap();
    for (w, h) in [(13, 7), (3, 3), (10, 10), (1, 1)] {
        let s = flat.scaled(w, h, Filter::Bilinear).unwrap();
        assert!(
            s.pixels()
                .iter()
                .all(|&p| p == Color::argb(200, 90, 120, 150).0),
            "{w}x{h}"
        );
    }
    // 2 画素 (黒, 白) を横に 4 倍: 左端は黒、右端は白、単調増加。
    let mut two = Image::new(2, 1).unwrap();
    two.pixels_mut().copy_from_slice(&[0xFF000000, 0xFFFFFFFF]);
    let s = two.scaled(8, 1, Filter::Bilinear).unwrap();
    let r: Vec<u8> = (0..8).map(|x| px(&s, x, 0).r()).collect();
    assert_eq!(r[0], 0);
    assert_eq!(r[7], 255);
    assert!(r.windows(2).all(|w| w[0] <= w[1]), "{r:?}");
    // 透明と不透明の補間でも色が暗くならない (アルファ重み付き)。
    let mut t = Image::new(2, 1).unwrap();
    t.pixels_mut().copy_from_slice(&[0x00000000, 0xFFFF0000]);
    let s = t.scaled(6, 1, Filter::Bilinear).unwrap();
    for x in 0..6 {
        let p = px(&s, x, 0);
        if p.a() > 0 {
            assert_eq!((p.r(), p.g(), p.b()), (255, 0, 0), "x={x} {p:?}");
        }
    }
    // 巨大な目標サイズ (部分表示) はクリップ範囲のみ描く。
    let mut dst = img(10, 10);
    dst.canvas().blit_scaled(
        &two.view(),
        two.bounds(),
        Rect::new(-1_000_000, -5, 3_000_000, 20),
        Filter::Bilinear,
        BlendMode::Copy,
        255,
    );
    assert_eq!(px(&dst, 5, 5), px(&dst, 5, 5));
}

#[test]
fn scaled_blend_modes_and_clipping() {
    let mut src = Image::new(2, 2).unwrap();
    src.pixels_mut().copy_from_slice(&[0x80FF0000; 4]);
    let mut d = Image::filled(8, 8, Color::BLACK).unwrap();
    d.canvas().blit_scaled(
        &src.view(),
        src.bounds(),
        Rect::new(2, 2, 4, 4),
        Filter::Nearest,
        BlendMode::SourceOver,
        255,
    );
    assert_eq!(px(&d, 3, 3), Color::rgb(128, 0, 0));
    assert_eq!(px(&d, 1, 1), Color::BLACK);
    assert_eq!(px(&d, 6, 6), Color::BLACK);
    // 一部だけがクリップ内でもマッピングが保たれる。
    let big = pattern(4, 4);
    let mut full = img(16, 16);
    full.canvas().blit_scaled(
        &big.view(),
        big.bounds(),
        Rect::new(0, 0, 16, 16),
        Filter::Nearest,
        BlendMode::Copy,
        255,
    );
    let mut part = img(16, 16);
    {
        let mut c = part.canvas();
        c.set_clip(Rect::new(5, 6, 6, 5));
        c.blit_scaled(
            &big.view(),
            big.bounds(),
            Rect::new(0, 0, 16, 16),
            Filter::Nearest,
            BlendMode::Copy,
            255,
        );
    }
    for y in 6..11 {
        for x in 5..11 {
            assert_eq!(px(&full, x, y), px(&part, x, y));
        }
    }
    assert_eq!(px(&part, 0, 0), BG);
}

#[test]
fn draw_mask_coverage() {
    let mask = [0u8, 128, 255, 64, 0, 255];
    let mut i = Image::filled(5, 4, Color::BLACK).unwrap();
    i.canvas()
        .draw_mask(1, 1, &MaskRef::new(3, 2, &mask, 3), Color::WHITE);
    assert_eq!(px(&i, 1, 1), Color::BLACK);
    assert_eq!(px(&i, 2, 1), Color::rgb(128, 128, 128));
    assert_eq!(px(&i, 3, 1), Color::WHITE);
    assert_eq!(px(&i, 1, 2), Color::rgb(64, 64, 64));
    assert_eq!(px(&i, 3, 2), Color::WHITE);
    // 左上にはみ出す配置: マスクの対応位置がずれない。
    let mut j = Image::filled(5, 4, Color::BLACK).unwrap();
    j.canvas()
        .draw_mask(-1, 0, &MaskRef::new(3, 2, &mask, 3), Color::WHITE);
    assert_eq!(px(&j, 0, 0), Color::rgb(128, 128, 128));
    assert_eq!(px(&j, 1, 0), Color::WHITE);
    assert_eq!(px(&j, 0, 1), Color::BLACK);
    assert_eq!(px(&j, 1, 1), Color::WHITE);
    // 色のアルファが乗算される。
    let mut k = Image::filled(5, 4, Color::BLACK).unwrap();
    k.canvas().draw_mask(
        1,
        1,
        &MaskRef::new(3, 2, &mask, 3),
        Color::argb(128, 255, 255, 255),
    );
    assert_eq!(px(&k, 3, 1), Color::rgb(128, 128, 128));
    // マスクが小さすぎる / stride が小さすぎる / 巨大寸法 → パニックしない。
    let mut l = Image::filled(5, 4, Color::BLACK).unwrap();
    l.canvas()
        .draw_mask(0, 0, &MaskRef::new(5, 4, &mask, 5), Color::WHITE);
    l.canvas()
        .draw_mask(0, 0, &MaskRef::new(3, 2, &mask, 2), Color::WHITE);
    l.canvas().draw_mask(
        i32::MIN,
        i32::MIN,
        &MaskRef::new(u32::MAX, u32::MAX, &mask, usize::MAX),
        Color::WHITE,
    );
    l.canvas()
        .draw_mask(-3, -3, &MaskRef::new(u32::MAX, 5, &mask, 6), Color::WHITE);
}

// ---------------------------------------------------------------- ガードバンド・ファジング

/// 乱数 (xorshift)。
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn i32_any(&mut self) -> i32 {
        match self.next() % 8 {
            0 => i32::MIN,
            1 => i32::MAX,
            2 => i32::MIN + (self.next() % 100) as i32,
            3 => i32::MAX - (self.next() % 100) as i32,
            4 => (self.next() % 200) as i32 - 100,
            5 => (self.next() as i32) % 1000,
            _ => self.next() as i32,
        }
    }
    fn u32_any(&mut self) -> u32 {
        match self.next() % 6 {
            0 => 0,
            1 => u32::MAX,
            2 => u32::MAX - (self.next() % 50) as u32,
            3 => (self.next() % 80) as u32,
            4 => (self.next() % 5000) as u32,
            _ => self.next() as u32,
        }
    }
    fn rect(&mut self) -> Rect {
        Rect::new(
            self.i32_any(),
            self.i32_any(),
            self.u32_any(),
            self.u32_any(),
        )
    }
    fn color(&mut self) -> Color {
        Color(self.next() as u32)
    }
}

/// 大きなバッファの内側に stride 付きの小さなキャンバスを置き、あらゆる描画 API に極端な引数を
/// 与えても、キャンバスの外側 (ガード領域) が 1 ピクセルも変わらないことを確認する。
#[test]
fn guard_band_never_written_under_random_extreme_arguments() {
    const GUARD: u32 = 0xDEAD_BEEF;
    let (bw, bh) = (48usize, 40usize);
    let (cx, cy, cw, ch) = (7usize, 5usize, 29u32, 23u32);
    let src = pattern(9, 7);
    let mut rng = Rng(0x1234_5678_9ABC_DEF1);
    let mut buf = vec![GUARD; bw * bh];
    let off = cy * bw + cx;
    for round in 0..600 {
        {
            let mut c = Canvas::new(&mut buf[off..], cw, ch, bw).unwrap();
            // 内側は毎回塗り直す (ガードは触れない)。
            c.clear(Color::rgb(1, 2, 3));
            if round % 3 == 0 {
                let r = rng.rect();
                c.set_clip(r);
            } else {
                c.reset_clip();
            }
            for _ in 0..12 {
                let color = rng.color();
                match rng.next() % 18 {
                    0 => c.fill_rect(rng.rect(), color),
                    1 => c.stroke_rect(rng.rect(), rng.u32_any(), color),
                    2 => c.line(
                        rng.i32_any(),
                        rng.i32_any(),
                        rng.i32_any(),
                        rng.i32_any(),
                        rng.u32_any() % 12,
                        color,
                    ),
                    3 => c.line_aa(
                        rng.i32_any(),
                        rng.i32_any(),
                        rng.i32_any(),
                        rng.i32_any(),
                        rng.u32_any() % 12,
                        color,
                    ),
                    4 => {
                        let (r, rad) = (rng.rect(), rng.u32_any());
                        match rng.next() % 3 {
                            0 => c.fill_round_rect(r, rad, color),
                            1 => c.fill_round_rect_gradient_v(
                                r,
                                rad,
                                color,
                                Color(rng.next() as u32),
                            ),
                            _ => c.fill_round_rect_gradient_h(
                                r,
                                rad,
                                color,
                                Color(rng.next() as u32),
                            ),
                        }
                    }
                    5 => c.stroke_round_rect(rng.rect(), rng.u32_any(), rng.u32_any(), color),
                    6 => c.fill_ellipse(rng.rect(), color),
                    7 => c.stroke_ellipse(rng.rect(), rng.u32_any(), color),
                    8 => c.fill_gradient_v(rng.rect(), color, rng.color()),
                    9 => c.fill_gradient_h(rng.rect(), color, rng.color()),
                    10 => c.blit_ex(
                        &src.view(),
                        rng.rect(),
                        rng.i32_any(),
                        rng.i32_any(),
                        BlendMode::SourceOver,
                        rng.next() as u8,
                    ),
                    11 => c.blit_ex(
                        &src.view(),
                        rng.rect(),
                        rng.i32_any(),
                        rng.i32_any(),
                        BlendMode::Copy,
                        255,
                    ),
                    12 => c.copy_within(rng.rect(), rng.i32_any(), rng.i32_any()),
                    13 => {
                        let f = if rng.next().is_multiple_of(2) {
                            Filter::Nearest
                        } else {
                            Filter::Bilinear
                        };
                        let mut d = rng.rect();
                        // 巨大な拡大率でも、時間はクリップ範囲に比例する。
                        d.w = d.w.min(100_000);
                        d.h = d.h.min(100_000);
                        c.blit_scaled(
                            &src.view(),
                            rng.rect(),
                            d,
                            f,
                            BlendMode::SourceOver,
                            rng.next() as u8,
                        );
                    }
                    14 => {
                        let m = [255u8; 64];
                        let (mx, my) = (rng.i32_any(), rng.i32_any());
                        let (mw, mh) = (rng.u32_any() % 9, rng.u32_any() % 9);
                        c.draw_mask(mx, my, &MaskRef::new(mw, mh, &m, 8), color);
                    }
                    15 => {
                        c.set_pixel(rng.i32_any(), rng.i32_any(), color);
                        c.blend_pixel(rng.i32_any(), rng.i32_any(), color);
                    }
                    16 => c.hline(rng.i32_any(), rng.i32_any(), rng.u32_any(), color),
                    _ => c.vline(rng.i32_any(), rng.i32_any(), rng.u32_any(), color),
                }
            }
            if let Some(mut s) = c.sub_canvas(rng.rect()) {
                s.fill_rect(rng.rect(), Color::WHITE);
                s.line(
                    rng.i32_any(),
                    rng.i32_any(),
                    rng.i32_any(),
                    rng.i32_any(),
                    1,
                    Color::WHITE,
                );
            }
        }
        // ガード領域 (キャンバス外すべて) が無傷。
        for y in 0..bh {
            for x in 0..bw {
                let inside =
                    (cx..cx + cw as usize).contains(&x) && (cy..cy + ch as usize).contains(&y);
                if !inside {
                    assert_eq!(
                        buf[y * bw + x],
                        GUARD,
                        "round {round}: guard at ({x},{y}) modified"
                    );
                }
            }
        }
    }
}

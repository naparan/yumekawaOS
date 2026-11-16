//! ホストでの簡易ベンチマーク (参考値)。実機/カーネルの soft-float 環境では、グリフの
//! ラスタライズ (キャッシュミス時) だけが大きく遅くなる。それ以外は整数演算のみ。
//!
//! ```sh
//! cargo run --release -p yk-gfx --example bench --target x86_64-unknown-linux-gnu
//! ```

use std::hint::black_box;
use std::time::Instant;

use yk_gfx::png::{PngLimits, decode_png};
use yk_gfx::text::{Font, TextRenderer, TextStyle};
use yk_gfx::{BlendMode, Color, Filter, Image, Rect, palette};

static REGULAR: &[u8] = include_bytes!("../../../assets/fonts/MPLUSRounded1c-Regular.ttf");

const W: u32 = 1280;
const H: u32 = 800;

fn bench(name: &str, iters: u32, mut f: impl FnMut()) {
    f(); // ウォームアップ
    let mut best = f64::MAX;
    let mut total = 0.0;
    for _ in 0..iters {
        let t = Instant::now();
        f();
        let dt = t.elapsed().as_secs_f64() * 1000.0;
        best = best.min(dt);
        total += dt;
    }
    println!(
        "{name:<52} best {best:>8.3} ms   avg {:>8.3} ms",
        total / iters as f64
    );
}

fn main() {
    println!("yk-gfx bench ({W}x{H} = {} pixels), release build\n", W * H);
    let mut dst = Image::filled(W, H, Color::rgb(255, 214, 232)).unwrap();
    let full = Rect::new(0, 0, W, H);

    bench("fill_rect opaque, full screen", 200, || {
        dst.canvas()
            .fill_rect(black_box(full), Color::rgb(10, 20, 30));
    });
    bench("fill_rect translucent (a=128), full screen", 100, || {
        dst.canvas()
            .fill_rect(black_box(full), Color::argb(128, 255, 255, 255));
    });
    bench("fill_gradient_v, full screen", 200, || {
        dst.canvas()
            .fill_gradient_v(black_box(full), palette::PASTEL_PINK, palette::LAVENDER);
    });
    bench("fill_gradient_h, full screen", 100, || {
        dst.canvas()
            .fill_gradient_h(black_box(full), palette::PASTEL_PINK, palette::LAVENDER);
    });

    // ソース: 全面に不透明 / 半透明 / 透明が混ざる画像
    let mut src = Image::new(W, H).unwrap();
    for (i, p) in src.pixels_mut().iter_mut().enumerate() {
        let a = match i % 7 {
            0 => 0,
            1 | 2 => 255,
            _ => ((i * 37) % 256) as u8,
        };
        *p = Color::argb(a, (i % 251) as u8, (i % 199) as u8, (i % 97) as u8).0;
    }
    let mut opaque_src = src.clone();
    opaque_src
        .pixels_mut()
        .iter_mut()
        .for_each(|p| *p |= 0xFF00_0000);
    bench("blit (opaque copy), full screen", 200, || {
        dst.canvas().blit(&black_box(&opaque_src).view(), 0, 0);
    });
    bench("blit_alpha, source fully opaque, full screen", 100, || {
        dst.canvas()
            .blit_alpha(&black_box(&opaque_src).view(), 0, 0);
    });
    bench(
        "blit_alpha, mixed alpha (1/7 transparent), full screen",
        50,
        || {
            dst.canvas().blit_alpha(&black_box(&src).view(), 0, 0);
        },
    );
    bench("blit_opacity 50%, full screen", 50, || {
        dst.canvas()
            .blit_opacity(&black_box(&opaque_src).view(), 0, 0, 128);
    });
    bench(
        "copy_within (scroll up by 16 rows), full screen",
        100,
        || {
            dst.canvas()
                .copy_within(black_box(Rect::new(0, 16, W, H - 16)), 0, 0);
        },
    );
    let half = Image::filled(W / 2, H / 2, Color::rgb(90, 120, 200)).unwrap();
    bench("blit_scaled nearest 640x400 -> 1280x800", 50, || {
        dst.canvas().blit_scaled(
            &half.view(),
            half.bounds(),
            full,
            Filter::Nearest,
            BlendMode::Copy,
            255,
        );
    });
    bench("blit_scaled bilinear 640x400 -> 1280x800", 20, || {
        dst.canvas().blit_scaled(
            &half.view(),
            half.bounds(),
            full,
            Filter::Bilinear,
            BlendMode::Copy,
            255,
        );
    });

    println!();
    bench("fill_round_rect 600x400 r=24", 200, || {
        dst.canvas().fill_round_rect(
            black_box(Rect::new(100, 100, 600, 400)),
            24,
            palette::LAVENDER,
        );
    });
    bench("stroke_round_rect 600x400 r=24 t=3", 200, || {
        dst.canvas().stroke_round_rect(
            black_box(Rect::new(100, 100, 600, 400)),
            24,
            3,
            palette::HOT_PINK,
        );
    });
    bench("fill_ellipse 600x400", 100, || {
        dst.canvas()
            .fill_ellipse(black_box(Rect::new(100, 100, 600, 400)), palette::MINT);
    });
    bench("line 1px, 1000 px diagonal", 1000, || {
        dst.canvas()
            .line(black_box(10), 10, 1010, 790, 1, palette::INK);
    });
    bench("line_aa thickness 4, 1000 px diagonal", 20, || {
        dst.canvas()
            .line(black_box(10), 10, 1010, 790, 4, palette::INK);
    });

    println!();
    let font = Font::new(REGULAR).unwrap();
    let text = "吾輩は猫である。名前はまだ無い。どこで生れたかとんと見当がつかぬ。何でも薄暗いじめじめした所でニャーニャー泣いていた事だけは記憶している。吾輩はここで始めて人間というものを見た。The quick brown fox jumps over the lazy dog. ゆめかわいいOSへようこそ!";
    let nchars = text.chars().count();
    let rect = Rect::new(20, 20, 560, 760);
    for size in [14.0f32, 24.0] {
        let st = TextStyle::new(size, palette::INK);
        // キャッシュなし (毎回ラスタライズ)
        let mut cold = TextRenderer::with_cache_capacity(0);
        let t = Instant::now();
        let n = 5;
        for _ in 0..n {
            dst.canvas().fill_rect(full, Color::WHITE);
            cold.draw_text_box(&mut dst.canvas(), &font, &st, rect, text);
        }
        let ms = t.elapsed().as_secs_f64() * 1000.0 / n as f64;
        println!(
            "text {size:>4}px, {nchars} chars, wrapped, no cache      {ms:>8.3} ms   ({:.1} us/char)",
            ms * 1000.0 / nchars as f64
        );
        // キャッシュあり: 初回 (ミス) と 2 回目以降 (ヒット)
        let mut tr = TextRenderer::new();
        let t = Instant::now();
        dst.canvas().fill_rect(full, Color::WHITE);
        tr.draw_text_box(&mut dst.canvas(), &font, &st, rect, text);
        let first = t.elapsed().as_secs_f64() * 1000.0;
        println!(
            "text {size:>4}px, {nchars} chars, wrapped, cache cold      {first:>8.3} ms   (misses {})",
            tr.cache_stats().misses
        );
        bench(
            &format!("text {size:>4}px, {nchars} chars, wrapped, cache warm"),
            200,
            || {
                dst.canvas().fill_rect(full, Color::WHITE);
                tr.draw_text_box(&mut dst.canvas(), &font, &st, black_box(rect), text);
            },
        );
        bench(&format!("  (layout only, {size}px)"), 500, || {
            black_box(font.layout(size, black_box(text), Some(560), None));
        });
    }
    // 漢字だけを 1000 字 (常に違うグリフ) 描く: キャッシュミスの上限に近いケース
    let kanji: String = include_str!("../tests/data/joyo_kanji.txt")
        .trim()
        .chars()
        .take(1000)
        .collect();
    let mut tr = TextRenderer::new();
    let t = Instant::now();
    dst.canvas().fill_rect(full, Color::WHITE);
    tr.draw_text_box(
        &mut dst.canvas(),
        &font,
        &TextStyle::new(20.0, palette::INK),
        Rect::new(0, 0, W, H),
        &kanji,
    );
    println!(
        "text 20px, 1000 distinct kanji, cache cold             {:>8.3} ms   ({:.1} us/glyph)",
        t.elapsed().as_secs_f64() * 1000.0,
        t.elapsed().as_secs_f64() * 1e6 / 1000.0
    );

    println!();
    // PNG デコード: 512x512 RGBA (ランダムに近い模様) と 1024x768 RGB (滑らか)
    let mut enc_rgba = Vec::new();
    {
        let mut px = Vec::new();
        let mut s = 1u32;
        for i in 0..512 * 512 {
            s = s.wrapping_mul(1664525).wrapping_add(1013904223);
            px.extend_from_slice(&[
                (i % 256) as u8,
                (s >> 24) as u8 / 4 + (i / 512 % 64) as u8,
                (i / 512) as u8,
                255 - (i % 100) as u8,
            ]);
        }
        let mut e = png::Encoder::new(&mut enc_rgba, 512, 512);
        e.set_color(png::ColorType::Rgba);
        e.set_depth(png::BitDepth::Eight);
        e.write_header().unwrap().write_image_data(&px).unwrap();
    }
    let mut enc_rgb = Vec::new();
    {
        let mut px = Vec::new();
        for y in 0..768u32 {
            for x in 0..1024u32 {
                px.extend_from_slice(&[(x / 4) as u8, (y / 3) as u8, ((x + y) / 8) as u8]);
            }
        }
        let mut e = png::Encoder::new(&mut enc_rgb, 1024, 768);
        e.set_color(png::ColorType::Rgb);
        e.set_depth(png::BitDepth::Eight);
        e.write_header().unwrap().write_image_data(&px).unwrap();
    }
    let lim = PngLimits::default();
    bench(
        &format!(
            "png decode 512x512 RGBA ({} KiB file)",
            enc_rgba.len() / 1024
        ),
        20,
        || {
            black_box(decode_png(black_box(&enc_rgba), &lim).unwrap());
        },
    );
    bench(
        &format!(
            "png decode 1024x768 RGB ({} KiB file)",
            enc_rgb.len() / 1024
        ),
        20,
        || {
            black_box(decode_png(black_box(&enc_rgb), &lim).unwrap());
        },
    );
}

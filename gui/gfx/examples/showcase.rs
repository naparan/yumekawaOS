//! 目視確認用のサンプル画面を描いて PNG に書き出す (ホスト専用)。
//!
//! ```sh
//! cargo run -p yk-gfx --example showcase --target x86_64-unknown-linux-gnu
//! # -> build/gfx/showcase.png (1280x800)
//! ```

use std::fs::File;
use std::io::BufWriter;
use std::path::PathBuf;

use yk_gfx::png::{PngLimits, decode_png};
use yk_gfx::text::{Font, TextAlign, TextRenderer, TextStyle};
use yk_gfx::{BlendMode, Canvas, Color, Filter, Image, Rect, palette};

static REGULAR: &[u8] = include_bytes!("../../../assets/fonts/MPLUSRounded1c-Regular.ttf");
static BOLD: &[u8] = include_bytes!("../../../assets/fonts/MPLUSRounded1c-Bold.ttf");

const W: u32 = 1280;
const H: u32 = 800;

/// ハートのアイコン (描いたものを PNG にエンコードしてデコードし直し、PNG 経路を試す)。
fn make_icon_png() -> Vec<u8> {
    let mut icon = Image::new(48, 48).unwrap();
    {
        let mut c = icon.canvas();
        c.fill_circle(16, 18, 11, palette::HOT_PINK);
        c.fill_circle(32, 18, 11, palette::HOT_PINK);
        // 下向きの三角形 (左右の辺はアンチエイリアス: 端の画素だけ部分カバレッジで合成する)。
        for y in 0..24i32 {
            let half64 = (19 * 64) - y * 53; // 半幅 (1/64 px)
            if half64 <= 0 {
                break;
            }
            let (full, frac) = (half64 / 64, half64 % 64);
            c.fill_rect(
                Rect::new(24 - full, 20 + y, (full * 2) as u32, 1),
                palette::HOT_PINK,
            );
            let edge = palette::HOT_PINK.mul_alpha((frac * 255 / 64) as u8);
            c.blend_pixel(24 - full - 1, 20 + y, edge);
            c.blend_pixel(24 + full, 20 + y, edge);
        }
        c.fill_circle(13, 13, 3, Color::argb(180, 255, 255, 255));
        c.stroke_circle(24, 24, 23, 1, Color::argb(120, 150, 95, 225));
    }
    let mut rgba = Vec::new();
    for &p in icon.pixels() {
        let c = Color(p);
        rgba.extend_from_slice(&[c.r(), c.g(), c.b(), c.a()]);
    }
    let mut out = Vec::new();
    let mut enc = png::Encoder::new(&mut out, 48, 48);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header().unwrap().write_image_data(&rgba).unwrap();
    out
}

fn window(c: &mut Canvas<'_>, r: Rect, title: &str, tr: &mut TextRenderer, bold: &Font<'_>) {
    // 影 (半透明の角丸矩形を少しずらして重ねる)
    for (i, a) in [14u8, 12, 10, 8].iter().enumerate() {
        let g = 3 * (4 - i as i32);
        c.fill_round_rect(
            Rect::new(
                r.x + 4 - g / 2,
                r.y + 8 - g / 2,
                r.w + g as u32,
                r.h + g as u32,
            ),
            22 + g as u32,
            Color::argb(*a, 120, 80, 180),
        );
    }
    c.fill_round_rect(r, 20, Color::argb(235, 255, 255, 255));
    // タイトルバー: ウィンドウ全体の角丸矩形をグラデーションで塗り、バーの高さにクリップする
    // (上の角だけ丸く、下端は直線になる)。
    {
        let mut bar = c.reborrow();
        bar.set_clip(Rect::new(r.x, r.y, r.w, 44));
        bar.fill_round_rect_gradient_h(r, 20, palette::PASTEL_PINK, palette::LAVENDER);
    }
    c.stroke_round_rect(r, 20, 2, palette::RIBBON_PINK);
    for (i, col) in [palette::HOT_PINK, palette::MINT, palette::LAVENDER]
        .iter()
        .enumerate()
    {
        c.fill_circle(r.x + 26 + i as i32 * 24, r.y + 22, 8, *col);
        c.stroke_circle(
            r.x + 26 + i as i32 * 24,
            r.y + 22,
            8,
            1,
            Color::argb(90, 150, 95, 225),
        );
    }
    let st = TextStyle::new(20.0, palette::DREAM_PURPLE).align(TextAlign::Center);
    tr.draw_text_box(
        c,
        bold,
        &st,
        Rect::new(r.x + 90, r.y + 10, r.w - 180, 30),
        title,
    );
}

fn main() {
    let regular = Font::new(REGULAR).expect("regular");
    let bold = Font::new(BOLD).expect("bold");
    let mut tr = TextRenderer::new();
    let mut screen = Image::new(W, H).unwrap();
    let mut c = screen.canvas();

    // ---- 背景: パステルのグラデーションと、半透明の円 ----
    c.fill_gradient_v(
        Rect::new(0, 0, W, H),
        Color::rgb(255, 214, 232),
        Color::rgb(221, 206, 255),
    );
    c.fill_gradient_h(
        Rect::new(0, H as i32 - 220, W, 220),
        Color::argb(0, 204, 245, 233),
        Color::argb(150, 204, 245, 233),
    );
    for (x, y, r, col) in [
        (1120, 90, 130u32, Color::argb(90, 255, 255, 255)),
        (1180, 160, 70, Color::argb(110, 255, 170, 210)),
        (90, 700, 150, Color::argb(80, 204, 245, 233)),
        (230, 760, 60, Color::argb(120, 255, 255, 255)),
        (640, 40, 40, Color::argb(100, 255, 255, 255)),
    ] {
        c.fill_circle(x, y, r, col);
    }
    // 星のきらきら (AA の線で描く)
    for (sx, sy, s) in [
        (60, 60, 14),
        (1230, 420, 10),
        (700, 770, 12),
        (420, 24, 8),
        (1000, 560, 9),
    ] {
        c.line_aa(sx - s, sy, sx + s, sy, 2, Color::argb(200, 255, 255, 255));
        c.line_aa(sx, sy - s, sx, sy + s, 2, Color::argb(200, 255, 255, 255));
        c.line_aa(
            sx - s / 2,
            sy - s / 2,
            sx + s / 2,
            sy + s / 2,
            1,
            Color::argb(160, 255, 255, 255),
        );
        c.line_aa(
            sx - s / 2,
            sy + s / 2,
            sx + s / 2,
            sy - s / 2,
            1,
            Color::argb(160, 255, 255, 255),
        );
    }

    // ---- 左: テキストのウィンドウ ----
    let win = Rect::new(40, 40, 600, 720);
    window(&mut c, win, "ゆめかわ Memo - テキスト表示", &mut tr, &bold);
    let mut y = win.y + 60;
    let ink = TextStyle::new(15.0, palette::INK);
    tr.draw_text(
        &mut c,
        &bold,
        &TextStyle::new(30.0, palette::HOT_PINK),
        win.x + 24,
        y,
        "ゆめかわいいOSへようこそ",
    );
    y += 46;
    for (size, col, text) in [
        (
            12.0,
            palette::INK,
            "12px: The quick brown fox jumps over the lazy dog / ひらがな カタカナ 漢字",
        ),
        (
            16.0,
            palette::INK,
            "16px: The quick brown fox jumps over the lazy dog / ひらがな カタカナ 漢字",
        ),
        (
            24.0,
            palette::DREAM_PURPLE,
            "24px: Aa Bb Cc あいうえお アイウエオ",
        ),
        (36.0, palette::HOT_PINK, "36px: 夢と星とハート ♡"),
        (52.0, palette::DEEP_MINT, "52px 日本語表示"),
    ] {
        let st = TextStyle::new(size, col);
        // 長いものは、はみ出さないように折り返して描く。
        let lay = regular.layout(size, text, Some(win.w - 48), None);
        let bx = Rect::new(win.x + 24, y, win.w - 48, lay.height());
        tr.draw_text_box(&mut c, &regular, &st, bx, text);
        y += lay.height() as i32 + 8;
    }
    y += 6;
    c.fill_round_rect(
        Rect::new(win.x + 24, y, win.w - 48, 4),
        2,
        palette::RIBBON_PINK,
    );
    y += 14;
    let para = "吾輩は猫である。名前はまだ無い。どこで生れたかとんと見当がつかぬ。何でも薄暗いじめじめした所でニャーニャー泣いていた事だけは記憶している。吾輩はここで始めて人間というものを見た。This paragraph is wrapped inside a box: the renderer breaks lines at spaces and between Japanese characters, and obeys basic kinsoku rules (、。」 never start a line).";
    let box_h = regular.measure_wrapped(15.0, para, win.w - 48).height;
    tr.draw_text_box(
        &mut c,
        &regular,
        &ink.line_height(24.0),
        Rect::new(win.x + 24, y, win.w - 48, box_h + 40),
        para,
    );
    y += regular
        .layout(15.0, para, Some(win.w - 48), Some(24.0))
        .height() as i32
        + 16;
    // 中央寄せ / 右寄せ
    for (al, label) in [
        (TextAlign::Left, "左寄せ Left"),
        (TextAlign::Center, "中央寄せ Center"),
        (TextAlign::Right, "右寄せ Right"),
    ] {
        let r = Rect::new(win.x + 24, y, win.w - 48, 30);
        c.fill_round_rect(r, 8, Color::argb(255, 255, 240, 247));
        c.stroke_round_rect(r, 8, 1, palette::RIBBON_PINK);
        tr.draw_text_box(
            &mut c,
            &regular,
            &TextStyle::new(18.0, palette::DREAM_PURPLE).align(al),
            Rect::new(r.x + 8, r.y + 3, r.w - 16, r.h - 6),
            label,
        );
        y += 38;
    }
    // 未収録の文字 (豆腐) とボールド
    tr.draw_text(
        &mut c,
        &regular,
        &TextStyle::new(18.0, palette::INK),
        win.x + 24,
        y + 4,
        "未収録の文字は豆腐: \u{1F600}\u{E000} / ",
    );
    tr.draw_text(
        &mut c,
        &bold,
        &TextStyle::new(18.0, palette::INK),
        win.x + 330,
        y + 4,
        "Bold: 太字の日本語",
    );
    y += 36;
    // キャッシュ統計
    let st = tr.cache_stats();
    let info = format!(
        "glyph cache: {} entries, {} KiB, {} hits / {} misses",
        st.entries,
        st.bytes / 1024,
        st.hits,
        st.misses
    );
    tr.draw_text(
        &mut c,
        &regular,
        &TextStyle::new(12.0, Color::rgb(150, 130, 190)),
        win.x + 24,
        y,
        &info,
    );

    // ---- 右上: 図形のウィンドウ ----
    let win2 = Rect::new(670, 40, 570, 400);
    window(&mut c, win2, "Shapes", &mut tr, &bold);
    let ox = win2.x + 24;
    let oy = win2.y + 64;
    // 線 (1px / 太線 / AA)
    for i in 0..8 {
        c.line(
            ox,
            oy + i * 6,
            ox + 120,
            oy + i * 6 + (i * 4 - 14),
            1,
            palette::DREAM_PURPLE,
        );
    }
    c.line(ox + 140, oy, ox + 250, oy + 60, 6, palette::HOT_PINK);
    c.line(ox + 140, oy + 70, ox + 250, oy + 10, 3, palette::DEEP_MINT);
    c.line(ox + 140, oy + 80, ox + 250, oy + 80, 5, palette::LAVENDER);
    c.line_aa(ox + 270, oy + 5, ox + 290, oy + 75, 1, palette::INK);
    // 矩形・角丸・枠
    c.fill_rect(Rect::new(ox, oy + 100, 80, 50), palette::PASTEL_PINK);
    c.stroke_rect(Rect::new(ox, oy + 100, 80, 50), 3, palette::HOT_PINK);
    c.fill_round_rect(Rect::new(ox + 100, oy + 100, 110, 50), 25, palette::MINT);
    c.stroke_round_rect(
        Rect::new(ox + 100, oy + 100, 110, 50),
        25,
        3,
        palette::DEEP_MINT,
    );
    c.fill_round_rect(Rect::new(ox + 230, oy + 100, 90, 50), 12, palette::LAVENDER);
    c.stroke_round_rect(
        Rect::new(ox + 230, oy + 100, 90, 50),
        12,
        2,
        palette::DREAM_PURPLE,
    );
    // 円・楕円
    c.fill_circle(ox + 40, oy + 215, 34, palette::HOT_PINK);
    c.stroke_circle(ox + 40, oy + 215, 34, 4, palette::DREAM_PURPLE);
    c.fill_ellipse(Rect::new(ox + 90, oy + 180, 120, 70), palette::MINT);
    c.stroke_ellipse(Rect::new(ox + 90, oy + 180, 120, 70), 3, palette::DEEP_MINT);
    c.stroke_ellipse(Rect::new(ox + 230, oy + 180, 50, 70), 1, palette::INK);
    c.fill_ellipse(
        Rect::new(ox + 290, oy + 195, 40, 40),
        Color::argb(140, 255, 105, 180),
    );
    c.fill_ellipse(
        Rect::new(ox + 310, oy + 195, 40, 40),
        Color::argb(140, 150, 95, 225),
    );
    // グラデーションのボタン
    let btn = Rect::new(win2.x + 350, oy + 5, 170, 44);
    c.fill_round_rect_gradient_h(btn, 22, palette::HOT_PINK, palette::DREAM_PURPLE);
    tr.draw_text_box(
        &mut c,
        &bold,
        &TextStyle::new(18.0, Color::WHITE).align(TextAlign::Center),
        Rect::new(btn.x, btn.y + 9, btn.w, 28),
        "グラデボタン",
    );
    c.fill_gradient_v(
        Rect::new(win2.x + 350, oy + 70, 80, 100),
        palette::PASTEL_PINK,
        palette::MINT,
    );
    c.fill_gradient_h(
        Rect::new(win2.x + 440, oy + 70, 80, 100),
        palette::LAVENDER,
        palette::HOT_PINK,
    );
    c.stroke_rect(Rect::new(win2.x + 350, oy + 70, 80, 100), 1, palette::INK);
    c.stroke_rect(Rect::new(win2.x + 440, oy + 70, 80, 100), 1, palette::INK);

    // ---- 右下: 画像のウィンドウ ----
    let win3 = Rect::new(670, 470, 570, 290);
    window(&mut c, win3, "Images (PNG)", &mut tr, &bold);
    let icon = decode_png(&make_icon_png(), &PngLimits::default()).expect("decode icon");
    let ix = win3.x + 24;
    let iy = win3.y + 64;
    c.blit_alpha(&icon.view(), ix, iy);
    c.blit_scaled(
        &icon.view(),
        icon.bounds(),
        Rect::new(ix + 70, iy, 96, 96),
        Filter::Nearest,
        BlendMode::SourceOver,
        255,
    );
    c.blit_scaled(
        &icon.view(),
        icon.bounds(),
        Rect::new(ix + 186, iy, 96, 96),
        Filter::Bilinear,
        BlendMode::SourceOver,
        255,
    );
    c.blit_opacity(&icon.view(), ix + 302, iy, 128);
    c.blit_scaled(
        &icon.view(),
        icon.bounds(),
        Rect::new(ix + 360, iy + 30, 24, 24),
        Filter::Bilinear,
        BlendMode::SourceOver,
        255,
    );
    c.blit_ex(
        &icon.view(),
        Rect::new(0, 0, 24, 24),
        ix + 400,
        iy,
        BlendMode::SourceOver,
        255,
    );
    for (i, label) in ["1x", "nearest 2x", "bilinear 2x", "50%", "24px", "crop"]
        .iter()
        .enumerate()
    {
        let xs = [0, 70, 186, 302, 360, 400];
        tr.draw_text(
            &mut c,
            &regular,
            &TextStyle::new(12.0, palette::INK),
            ix + xs[i],
            iy + 100,
            label,
        );
    }
    // 画面内の別領域のコピー (surface to surface)
    let strip = Rect::new(win3.x + 24, win3.y + 64, 300, 96);
    let mut copy = Image::new(strip.w, strip.h).unwrap();
    copy.canvas().blit_region(&c.as_view(), strip, 0, 0);
    c.blit_scaled(
        &copy.view(),
        copy.bounds(),
        Rect::new(win3.x + 24, win3.y + 198, 225, 72),
        Filter::Bilinear,
        BlendMode::Copy,
        255,
    );
    c.stroke_round_rect(
        Rect::new(win3.x + 24, win3.y + 198, 225, 72),
        6,
        1,
        palette::RIBBON_PINK,
    );
    tr.draw_text(
        &mut c,
        &regular,
        &TextStyle::new(13.0, palette::INK),
        win3.x + 270,
        win3.y + 205,
        "左: ウィンドウ内の領域を",
    );
    tr.draw_text(
        &mut c,
        &regular,
        &TextStyle::new(13.0, palette::INK),
        win3.x + 270,
        win3.y + 225,
        "別 Surface にコピーして",
    );
    tr.draw_text(
        &mut c,
        &regular,
        &TextStyle::new(13.0, palette::INK),
        win3.x + 270,
        win3.y + 245,
        "縮小転送した結果",
    );

    // ---- PNG に書き出す ----
    let out_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../build/gfx");
    std::fs::create_dir_all(&out_dir).unwrap();
    let path = out_dir.join("showcase.png");
    let mut rgb = Vec::with_capacity((W * H * 3) as usize);
    for &p in screen.pixels() {
        let col = Color(p);
        rgb.extend_from_slice(&[col.r(), col.g(), col.b()]);
    }
    let file = BufWriter::new(File::create(&path).unwrap());
    let mut enc = png::Encoder::new(file, W, H);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header().unwrap().write_image_data(&rgb).unwrap();
    println!("wrote {}", path.display());
}

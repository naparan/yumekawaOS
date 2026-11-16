//! 起動スプラッシュ画面 (Phase 1 の「画面表示」)。
//!
//! パステルの縦グラデーション背景 + 雲 + キラキラ + 角丸カード +
//! 「yumekawaOS」ロゴ文字を描く。背景色は座標から計算できるため、
//! 半透明表現も framebuffer を読み戻さずに合成している。

use super::color::{Rgb, palette};
use super::font::{self, GLYPH_SIZE};
use super::framebuffer::Framebuffer;

/// ロゴ文字列。
pub const TITLE: &str = "yumekawaOS";
const SUBTITLE: &str = "~ pastel dreams, written in Rust ~";

/// 8x8 のハート。
const HEART: [u8; 8] = [0x36, 0x7F, 0x7F, 0x7F, 0x3E, 0x1C, 0x08, 0x00];

/// スプラッシュに表示する可変テキスト。
#[derive(Debug, Clone, Copy)]
pub struct SplashText<'a> {
    /// カード内のステータス行(例: "phase 1 : boot ok")
    pub status: &'a str,
    /// 画面下部の情報行(解像度・メモリ量など)
    pub footer: &'a str,
}

/// 背景グラデーション: 上 = ピンク → 中 = ラベンダー → 下 = ミント。
fn background(y: i64, height: i64) -> Rgb {
    let h = height.max(2) - 1;
    let y = y.clamp(0, h);
    let half = h / 2;
    if y <= half {
        palette::PASTEL_PINK.lerp(palette::LAVENDER, y as u32, half.max(1) as u32)
    } else {
        palette::LAVENDER.lerp(palette::MINT, (y - half) as u32, (h - half).max(1) as u32)
    }
}

/// 決定的な疑似乱数 (xorshift64)。描画位置を毎回同じにするため。
struct XorShift(u64);

impl XorShift {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    fn below(&mut self, n: u64) -> u64 {
        if n == 0 { 0 } else { self.next() % n }
    }
}

/// 4 方向に光る小さな星を描く。
fn draw_sparkle(fb: &mut Framebuffer, cx: i64, cy: i64, arm: i64, unit: u64, color: Rgb) {
    let u = unit as i64;
    fb.fill_rect(cx - arm * u, cy, (arm * 2 + 1) as u64 * unit, unit, color);
    fb.fill_rect(cx, cy - arm * u, unit, (arm * 2 + 1) as u64 * unit, color);
    if arm >= 3 {
        // 中心を少し太く
        fb.fill_rect(cx - u, cy - u, unit * 3, unit * 3, color);
    }
}

/// 雲(円の重ね合わせ)を背景に半透明で描く。
fn draw_cloud(fb: &mut Framebuffer, cx: i64, cy: i64, r: u64, height: i64) {
    let parts: [(i64, i64, u64); 4] = [
        (-(r as i64) * 6 / 5, r as i64 / 3, r * 3 / 4),
        (0, 0, r),
        (r as i64 * 6 / 5, r as i64 / 4, r * 4 / 5),
        (r as i64 * 2, r as i64 / 2, r / 2),
    ];
    for (dx, dy, pr) in parts {
        fb.fill_circle_with(cx + dx, cy + dy, pr, |_, y| {
            background(y, height).blend(palette::WHITE, 110)
        });
    }
}

/// スプラッシュ画面全体を描く。
pub fn draw(fb: &mut Framebuffer, text: &SplashText<'_>) {
    let width = fb.width() as i64;
    let height = fb.height() as i64;

    // 1. 背景グラデーション(行ごとに単色)
    for y in 0..height {
        fb.fill_rect(0, y, width as u64, 1, background(y, height));
    }

    // 2. 雲
    let cloud_r = (width / 18).max(8) as u64;
    draw_cloud(fb, width / 8, height * 4 / 5, cloud_r, height);
    draw_cloud(fb, width * 3 / 4, height * 5 / 6, cloud_r * 5 / 4, height);
    draw_cloud(fb, width * 4 / 5, height / 6, cloud_r * 2 / 3, height);

    // 3. キラキラ
    let unit = (width / 640).max(1) as u64;
    let mut rng = XorShift(0x7975_6d65_6b61_7761); // "yumekawa"
    for i in 0..48 {
        let x = rng.below(width as u64) as i64;
        let y = rng.below(height as u64) as i64;
        let arm = 1 + rng.below(4) as i64;
        let color = match i % 3 {
            0 => palette::WHITE,
            1 => palette::WHITE.blend(palette::RIBBON_PINK, 90),
            _ => palette::WHITE.blend(palette::MINT, 120),
        };
        draw_sparkle(fb, x, y, arm, unit, color);
    }

    // 4. レイアウト計算
    let title_scale = ((width as u64 * 55 / 100) / font::text_width(TITLE, 1)).clamp(1, 8);
    let small_scale: u64 = if width >= 1024 { 2 } else { 1 };
    let title_w = font::text_width(TITLE, title_scale);
    let title_h = GLYPH_SIZE * title_scale;
    let heart_scale = (title_scale / 2).max(1);
    let heart_w = GLYPH_SIZE * heart_scale;
    let gap = title_scale * 3;
    let pad = title_scale * 5;
    let line_h = GLYPH_SIZE * small_scale;

    let content_w = title_w
        .max(font::text_width(SUBTITLE, small_scale))
        .max(font::text_width(text.status, small_scale))
        + (heart_w + gap) * 2;
    let card_w = content_w + pad * 2;
    // タイトル / 間隔 / サブタイトル / 1 行空き / ステータス
    let card_h = pad + title_h + gap + line_h * 3 + pad;
    let card_x = (width - card_w as i64) / 2;
    let card_y = (height - card_h as i64) / 2;
    let radius = title_scale * 4;
    let border = small_scale * 2;

    trace!(
        "splash layout: title_scale={title_scale} small_scale={small_scale} card=({card_x},{card_y}) {card_w}x{card_h}"
    );

    // 5. 角丸カード(ピンクのふち + 半透明の白)
    fb.fill_rounded_rect_with(card_x, card_y, card_w, card_h, radius, |_, _| {
        palette::RIBBON_PINK
    });
    fb.fill_rounded_rect_with(
        card_x + border as i64,
        card_y + border as i64,
        card_w - border * 2,
        card_h - border * 2,
        radius.saturating_sub(border),
        |_, y| background(y, height).blend(palette::WHITE, 175),
    );

    // 6. ロゴ(影 + 縦グラデーション文字)とハート
    let title_x = (width - title_w as i64) / 2;
    let title_y = card_y + pad as i64;
    let shadow = (title_scale / 3).max(1) as i64;
    font::draw_text(
        fb,
        title_x + shadow,
        title_y + shadow,
        TITLE,
        title_scale,
        palette::SHADOW_LAVENDER,
    );
    font::draw_text_shaded(fb, title_x, title_y, TITLE, title_scale, |row| {
        palette::HOT_PINK.lerp(palette::DREAM_PURPLE, row as u32, title_h as u32)
    });
    let heart_y = title_y + (title_h as i64 - heart_w as i64) / 2;
    let mut heart_color = |_| palette::HOT_PINK;
    font::draw_bitmap(
        fb,
        title_x - (gap + heart_w) as i64,
        heart_y,
        &HEART,
        heart_scale,
        &mut heart_color,
    );
    font::draw_bitmap(
        fb,
        title_x + (title_w + gap) as i64,
        heart_y,
        &HEART,
        heart_scale,
        &mut heart_color,
    );

    // 7. サブタイトルとステータス
    let sub_y = title_y + (title_h + gap) as i64;
    let sub_x = (width - font::text_width(SUBTITLE, small_scale) as i64) / 2;
    font::draw_text(fb, sub_x, sub_y, SUBTITLE, small_scale, palette::INK);
    let status_y = sub_y + (line_h * 2) as i64;
    let status_x = (width - font::text_width(text.status, small_scale) as i64) / 2;
    font::draw_text(
        fb,
        status_x,
        status_y,
        text.status,
        small_scale,
        palette::DEEP_MINT,
    );

    // 8. 画面下部の情報行
    let margin = (line_h * 2) as i64;
    font::draw_text(
        fb,
        margin,
        height - margin - line_h as i64,
        text.footer,
        small_scale,
        palette::INK,
    );
}

//! 色とアルファ合成。
//!
//! # ピクセル形式
//!
//! 内部表現は **ARGB8888 / 非プリマルチプライド (straight alpha)** の `u32`
//! (`0xAARRGGBB`、A が最上位バイト)。リトルエンディアンのメモリ上ではバイト順が
//! `B, G, R, A` になり、UEFI GOP の一般的な `BGRX` framebuffer とバイト列が一致する
//! (実 framebuffer への転送時の形式変換は呼び出し側の責務。docs/graphics.md 参照)。
//!
//! 合成は整数演算のみで行う (soft-float のカーネルで遅くならないため)。

/// 1 チャネルの線形補間。`a*(den-t) + b*t` は u64 で計算し、巨大な `den` でもオーバーフローしない。
const fn lerp_ch(a: u8, b: u8, t: u32, den: u32) -> u8 {
    ((a as u64 * (den - t) as u64 + b as u64 * t as u64 + den as u64 / 2) / den as u64) as u8
}

/// 一般式のチャネル合成 `(cs*sa + cd*dw) / ao` (四捨五入)。
const fn mix_ch(cs: u32, cd: u32, sa: u32, dw: u32, ao: u32) -> u32 {
    (cs * sa + cd * dw + ao / 2) / ao
}

/// `x / 255` を四捨五入で求める (x <= 255*255 + 255 の範囲で正確)。
#[inline(always)]
pub(crate) const fn div255(x: u32) -> u32 {
    let t = x + 128;
    (t + (t >> 8)) >> 8
}

/// ARGB8888 (straight alpha) の色。
#[derive(Clone, Copy, PartialEq, Eq, Hash, Default)]
#[repr(transparent)]
pub struct Color(pub u32);

impl core::fmt::Debug for Color {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "Color(#{:08x})", self.0)
    }
}

impl Color {
    /// 完全に透明な黒。
    pub const TRANSPARENT: Color = Color(0);
    /// 不透明な黒。
    pub const BLACK: Color = Color::rgb(0, 0, 0);
    /// 不透明な白。
    pub const WHITE: Color = Color::rgb(255, 255, 255);

    /// アルファ付きで色を作る。
    #[inline]
    pub const fn argb(a: u8, r: u8, g: u8, b: u8) -> Color {
        Color(((a as u32) << 24) | ((r as u32) << 16) | ((g as u32) << 8) | b as u32)
    }

    /// 不透明な色を作る。
    #[inline]
    pub const fn rgb(r: u8, g: u8, b: u8) -> Color {
        Color::argb(255, r, g, b)
    }

    /// `0xRRGGBB` から不透明な色を作る (上位 8 ビットは無視)。
    #[inline]
    pub const fn from_rgb_u32(rgb: u32) -> Color {
        Color(0xFF00_0000 | (rgb & 0x00FF_FFFF))
    }

    /// アルファ (0 = 透明, 255 = 不透明)。
    #[inline]
    pub const fn a(self) -> u8 {
        (self.0 >> 24) as u8
    }
    /// 赤。
    #[inline]
    pub const fn r(self) -> u8 {
        (self.0 >> 16) as u8
    }
    /// 緑。
    #[inline]
    pub const fn g(self) -> u8 {
        (self.0 >> 8) as u8
    }
    /// 青。
    #[inline]
    pub const fn b(self) -> u8 {
        self.0 as u8
    }

    /// アルファだけ差し替えた色。
    #[inline]
    pub const fn with_alpha(self, a: u8) -> Color {
        Color((self.0 & 0x00FF_FFFF) | ((a as u32) << 24))
    }

    /// アルファに `a / 255` を掛けた色 (全体の不透明度の調整)。
    #[inline]
    pub const fn mul_alpha(self, a: u8) -> Color {
        let na = div255(self.a() as u32 * a as u32);
        self.with_alpha(na as u8)
    }

    /// 全チャネル (アルファ含む) の線形補間。`t / den` (0..=1) の位置の色。
    /// `t > den` は `den` に、`den == 0` は 1 に丸める。
    pub const fn lerp(self, other: Color, t: u32, den: u32) -> Color {
        let den = if den == 0 { 1 } else { den };
        let t = if t > den { den } else { t };
        Color::argb(
            lerp_ch(self.a(), other.a(), t, den),
            lerp_ch(self.r(), other.r(), t, den),
            lerp_ch(self.g(), other.g(), t, den),
            lerp_ch(self.b(), other.b(), t, den),
        )
    }

    /// `self` (前景) を `dst` (背景) の上に source-over で重ねた色。
    #[inline]
    pub const fn over(self, dst: Color) -> Color {
        Color(blend_over(dst.0, self.0))
    }
}

/// source-over 合成。`dst` に `src` を重ねた結果を返す (どちらも straight alpha の ARGB8888)。
///
/// - `src` のアルファが 255 なら `src`、0 なら `dst` をそのまま返す。
/// - `dst` が不透明 (一般的なバックバッファ) なら除算なしの高速経路。
/// - `dst` が半透明のときは、結果のアルファと色を一般式で求める (チャネルごとに除算あり)。
#[inline]
pub const fn blend_over(dst: u32, src: u32) -> u32 {
    let sa = src >> 24;
    if sa == 255 {
        return src;
    }
    if sa == 0 {
        return dst;
    }
    let da = dst >> 24;
    if da == 255 {
        let ia = 255 - sa;
        let r = div255(((src >> 16) & 0xFF) * sa + ((dst >> 16) & 0xFF) * ia);
        let g = div255(((src >> 8) & 0xFF) * sa + ((dst >> 8) & 0xFF) * ia);
        let b = div255((src & 0xFF) * sa + (dst & 0xFF) * ia);
        return 0xFF00_0000 | (r << 16) | (g << 8) | b;
    }
    if da == 0 {
        return src;
    }
    // 一般式: ao = sa + da*(1-sa); co = (cs*sa + cd*da*(1-sa)) / ao
    let dw = div255(da * (255 - sa)); // 背景の寄与 (0..=255)
    let ao = sa + dw; // 1..=255
    let r = mix_ch((src >> 16) & 0xFF, (dst >> 16) & 0xFF, sa, dw, ao);
    let g = mix_ch((src >> 8) & 0xFF, (dst >> 8) & 0xFF, sa, dw, ao);
    let b = mix_ch(src & 0xFF, dst & 0xFF, sa, dw, ao);
    (ao << 24) | (r << 16) | (g << 8) | b
}

/// `src` に追加のカバレッジ `cov` (0..=255) を掛けたうえで `dst` に source-over する。
/// アンチエイリアスされた図形やグリフの描画に使う。
#[inline]
pub(crate) const fn blend_cov(dst: u32, src: u32, cov: u32) -> u32 {
    if cov == 255 {
        return blend_over(dst, src);
    }
    if cov == 0 {
        return dst;
    }
    let a = div255((src >> 24) * cov);
    blend_over(dst, (src & 0x00FF_FFFF) | (a << 24))
}

/// ゆめかわパレット (§16.2)。カーネルの `graphics::color::palette` と同じ値。
pub mod palette {
    use super::Color;

    /// パステルピンク
    pub const PASTEL_PINK: Color = Color::rgb(255, 214, 232);
    /// ラベンダー
    pub const LAVENDER: Color = Color::rgb(221, 206, 255);
    /// ミント
    pub const MINT: Color = Color::rgb(204, 245, 233);
    /// ホワイト
    pub const WHITE: Color = Color::rgb(255, 255, 255);
    /// 濃いめのピンク
    pub const HOT_PINK: Color = Color::rgb(255, 105, 180);
    /// 紫
    pub const DREAM_PURPLE: Color = Color::rgb(150, 95, 225);
    /// 影色
    pub const SHADOW_LAVENDER: Color = Color::rgb(196, 170, 240);
    /// ふち取りのピンク
    pub const RIBBON_PINK: Color = Color::rgb(255, 170, 210);
    /// 本文テキスト
    pub const INK: Color = Color::rgb(120, 90, 170);
    /// ミント系の強調色
    pub const DEEP_MINT: Color = Color::rgb(40, 160, 130);
}

//! 色の表現とゆめかわパレット。

/// 8bit/チャネルの RGB 色(framebuffer のピクセル形式とは独立)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb {
    /// 赤
    pub r: u8,
    /// 緑
    pub g: u8,
    /// 青
    pub b: u8,
}

impl Rgb {
    /// 色を作る。
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    /// `self` から `other` への線形補間。`t / den` (0..=1) の位置の色を返す。
    pub const fn lerp(self, other: Self, t: u32, den: u32) -> Self {
        let den = if den == 0 { 1 } else { den };
        let t = if t > den { den } else { t };
        Self {
            r: lerp_u8(self.r, other.r, t, den),
            g: lerp_u8(self.g, other.g, t, den),
            b: lerp_u8(self.b, other.b, t, den),
        }
    }

    /// `self` (背景) の上に `fg` を不透明度 `alpha` (0..=255) で重ねた色。
    pub const fn blend(self, fg: Self, alpha: u8) -> Self {
        self.lerp(fg, alpha as u32, 255)
    }
}

const fn lerp_u8(a: u8, b: u8, t: u32, den: u32) -> u8 {
    let a = a as u32;
    let b = b as u32;
    ((a * (den - t) + b * t + den / 2) / den) as u8
}

/// yumekawaOS の基本パレット (§16.2: パステルピンク / ラベンダー / ミント / ホワイト)。
pub mod palette {
    use super::Rgb;

    /// パステルピンク
    pub const PASTEL_PINK: Rgb = Rgb::new(255, 214, 232);
    /// ラベンダー
    pub const LAVENDER: Rgb = Rgb::new(221, 206, 255);
    /// ミント
    pub const MINT: Rgb = Rgb::new(204, 245, 233);
    /// ホワイト
    pub const WHITE: Rgb = Rgb::new(255, 255, 255);
    /// 濃いめのピンク(見出し文字)
    pub const HOT_PINK: Rgb = Rgb::new(255, 105, 180);
    /// 紫(見出し文字のグラデーション終点)
    pub const DREAM_PURPLE: Rgb = Rgb::new(150, 95, 225);
    /// 影色
    pub const SHADOW_LAVENDER: Rgb = Rgb::new(196, 170, 240);
    /// ふち取りのピンク
    pub const RIBBON_PINK: Rgb = Rgb::new(255, 170, 210);
    /// 本文テキスト
    pub const INK: Rgb = Rgb::new(120, 90, 170);
    /// ミント系の強調色
    pub const DEEP_MINT: Rgb = Rgb::new(40, 160, 130);
}

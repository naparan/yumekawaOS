//! バックバッファ (ARGB8888) を実 framebuffer の画素形式へ変換する部品。
//!
//! ブートローダ (Limine) は framebuffer の R/G/B の位置を `shift`/`size` で申告する。
//! 一般的な UEFI GOP は `B=0..8, G=8..16, R=16..24` (`XRGB8888` 相当) で、この場合は
//! 上位 8 ビットを 0 にするだけ。それ以外の配置 (`BGRX` を反転した形式や 565 など) は
//! チャネルごとに丸めて詰め直す。実際の書き込み (volatile など) は呼び出し側の責務。

use crate::color::Color;

/// framebuffer の 1 チャネルの配置。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Channel {
    /// 最下位ビットの位置 (0..32)
    pub shift: u8,
    /// ビット幅 (1..=16。範囲外は丸める)
    pub size: u8,
}

/// framebuffer の 32bit 画素の形式 (R/G/B の配置。使われないビットは 0 にする)。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PixelLayout {
    /// 赤
    pub red: Channel,
    /// 緑
    pub green: Channel,
    /// 青
    pub blue: Channel,
}

impl PixelLayout {
    /// `0x00RRGGBB` (R=16, G=8, B=0, 各 8 ビット)。バックバッファと同じ並び。
    pub const XRGB8888: PixelLayout = PixelLayout {
        red: Channel { shift: 16, size: 8 },
        green: Channel { shift: 8, size: 8 },
        blue: Channel { shift: 0, size: 8 },
    };

    /// バックバッファの画素 (アルファは無視) を、この形式の 32bit 値にする。
    #[inline]
    pub fn encode(&self, argb: u32) -> u32 {
        let c = Color(argb);
        enc(c.r(), self.red) | enc(c.g(), self.green) | enc(c.b(), self.blue)
    }

    /// 1 行ぶんを変換する。`src` と `dst` の短い方の長さまで処理する。
    /// `XRGB8888` のときはマスクだけの高速経路。
    pub fn convert_row(&self, src: &[u32], dst: &mut [u32]) {
        if *self == Self::XRGB8888 {
            for (d, &s) in dst.iter_mut().zip(src) {
                *d = s & 0x00FF_FFFF;
            }
        } else {
            for (d, &s) in dst.iter_mut().zip(src) {
                *d = self.encode(s);
            }
        }
    }
}

#[inline]
fn enc(v: u8, ch: Channel) -> u32 {
    let size = ch.size.clamp(1, 16) as u32;
    let shift = (ch.shift as u32).min(31);
    let max = (1u32 << size) - 1;
    ((v as u32 * max + 127) / 255) << shift
}

//! 線形 framebuffer への安全な書き込み API。
//!
//! - bpp / pitch / RGB マスクをブートローダの申告どおりに扱う。
//! - **32bpp 以外は [`FramebufferError::UnsupportedBpp`] として明示的に拒否する**
//!   (24/16bpp 等への対応は必要になった時点で追加する)。
//! - すべての描画は画面範囲でクリップされ、範囲外へは書き込まない。
//! - framebuffer の内容は読み出さない(読み出しは低速で、初期内容は未定義のため)。

use core::fmt;
use core::ptr::NonNull;

use super::color::Rgb;
use crate::boot::{ChannelMask, FramebufferInfo};

/// framebuffer を使用できない理由。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FramebufferError {
    /// 32bpp 以外は未対応。
    UnsupportedBpp(u16),
    /// RGB 以外のメモリモデルは未対応。
    UnsupportedMemoryModel(u8),
    /// チャネルのマスクが 32bit に収まらない/重なっている/幅 0 等。
    InvalidChannelMask {
        /// "red" / "green" / "blue"
        channel: &'static str,
        /// マスク幅
        size: u8,
        /// シフト量
        shift: u8,
    },
    /// 寸法や pitch・アドレスが不正。
    InvalidGeometry {
        /// 幅
        width: u64,
        /// 高さ
        height: u64,
        /// pitch
        pitch: u64,
    },
}

impl fmt::Display for FramebufferError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedBpp(bpp) => {
                write!(
                    f,
                    "unsupported framebuffer depth: {bpp} bpp (only 32 bpp is supported)"
                )
            }
            Self::UnsupportedMemoryModel(m) => {
                write!(
                    f,
                    "unsupported framebuffer memory model {m} (only RGB is supported)"
                )
            }
            Self::InvalidChannelMask {
                channel,
                size,
                shift,
            } => {
                write!(
                    f,
                    "invalid {channel} channel mask (size={size}, shift={shift})"
                )
            }
            Self::InvalidGeometry {
                width,
                height,
                pitch,
            } => {
                write!(
                    f,
                    "invalid framebuffer geometry {width}x{height}, pitch={pitch}"
                )
            }
        }
    }
}

/// 1 ピクセル (32bit) 内の R/G/B の配置。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PixelFormat {
    red: ChannelMask,
    green: ChannelMask,
    blue: ChannelMask,
}

impl PixelFormat {
    /// マスクを検証して作る。各チャネルは 1..=16 bit、32bit 内に収まり、互いに重ならないこと。
    pub fn new(
        red: ChannelMask,
        green: ChannelMask,
        blue: ChannelMask,
    ) -> Result<Self, FramebufferError> {
        let mut used: u32 = 0;
        for (name, m) in [("red", red), ("green", green), ("blue", blue)] {
            let invalid = FramebufferError::InvalidChannelMask {
                channel: name,
                size: m.size,
                shift: m.shift,
            };
            if m.size == 0 || m.size > 16 || u32::from(m.size) + u32::from(m.shift) > 32 {
                return Err(invalid);
            }
            let bits = mask_bits(m);
            if used & bits != 0 {
                return Err(invalid);
            }
            used |= bits;
        }
        Ok(Self { red, green, blue })
    }

    /// `Rgb` をこの形式の 32bit 値へ変換する(チャネル幅に合わせて丸める)。
    pub fn encode(&self, c: Rgb) -> u32 {
        scale_channel(c.r, self.red)
            | scale_channel(c.g, self.green)
            | scale_channel(c.b, self.blue)
    }
}

fn mask_bits(m: ChannelMask) -> u32 {
    let ones = if m.size >= 32 {
        u32::MAX
    } else {
        (1u32 << m.size) - 1
    };
    ones << m.shift
}

fn scale_channel(v: u8, m: ChannelMask) -> u32 {
    let max = (1u32 << m.size) - 1;
    ((u32::from(v) * max + 127) / 255) << m.shift
}

/// 32bpp 線形 framebuffer。
///
/// [`FramebufferInfo`] (ブートローダが渡す書き込み権) を消費して作るため、
/// カーネル内に同じ framebuffer を指すインスタンスは 1 つしか存在しない。
pub struct Framebuffer {
    base: NonNull<u8>,
    width: usize,
    height: usize,
    pitch: usize,
    format: PixelFormat,
}

const BYTES_PER_PIXEL: usize = 4;

// SAFETY: Framebuffer は framebuffer メモリへの唯一の書き込み権 (FramebufferInfo を消費して作る)
// を表し、スレッド/コンテキスト間で移動しても別名は生じない。共有 (Sync) はしない。
unsafe impl Send for Framebuffer {}

impl Framebuffer {
    /// ブートローダの framebuffer 情報を検証して使用可能にする。
    pub fn new(info: FramebufferInfo) -> Result<Self, FramebufferError> {
        if info.memory_model() != 1 {
            return Err(FramebufferError::UnsupportedMemoryModel(
                info.memory_model(),
            ));
        }
        if info.bpp() != 32 {
            return Err(FramebufferError::UnsupportedBpp(info.bpp()));
        }
        let geometry_error = FramebufferError::InvalidGeometry {
            width: info.width(),
            height: info.height(),
            pitch: info.pitch(),
        };
        let width = usize::try_from(info.width()).map_err(|_| geometry_error)?;
        let height = usize::try_from(info.height()).map_err(|_| geometry_error)?;
        let pitch = usize::try_from(info.pitch()).map_err(|_| geometry_error)?;
        let row_bytes = width.checked_mul(BYTES_PER_PIXEL).ok_or(geometry_error)?;
        let total = pitch.checked_mul(height).ok_or(geometry_error)?;
        if width == 0
            || height == 0
            || pitch < row_bytes
            || !pitch.is_multiple_of(BYTES_PER_PIXEL)
            || total > isize::MAX as usize
            || !(info.address().as_ptr() as usize).is_multiple_of(BYTES_PER_PIXEL)
        {
            return Err(geometry_error);
        }
        let format = PixelFormat::new(info.red(), info.green(), info.blue())?;
        Ok(Self {
            base: info.address(),
            width,
            height,
            pitch,
            format,
        })
    }

    /// 幅 (ピクセル)。
    pub fn width(&self) -> usize {
        self.width
    }

    /// 高さ (ピクセル)。
    pub fn height(&self) -> usize {
        self.height
    }

    /// 1 行のバイト数。
    pub fn pitch(&self) -> usize {
        self.pitch
    }

    /// ピクセル形式。
    pub fn format(&self) -> PixelFormat {
        self.format
    }

    /// 範囲内であることが確認済みの座標へ 32bit 値を書く。
    #[inline]
    fn write_raw(&mut self, x: usize, y: usize, value: u32) {
        assert!(
            x < self.width && y < self.height,
            "framebuffer write out of bounds"
        );
        let offset = y * self.pitch + x * BYTES_PER_PIXEL;
        // SAFETY: x < width, y < height なので offset + 4 <= y*pitch + width*4 <= pitch*height
        // であり、`Framebuffer::new` で pitch*height <= isize::MAX を確認済み。
        // よって加算結果は同じ framebuffer 領域内に留まる。
        let pixel = unsafe { self.base.as_ptr().add(offset) }.cast::<u32>();
        // SAFETY: `pixel` は上記より framebuffer 領域内で、FramebufferInfo の契約により
        // 書き込み可能にマップされている。先頭と pitch が 4 バイト境界であることを
        // `Framebuffer::new` で確認済みなので u32 として整列している。
        // デバイスに見えるメモリなので volatile で書き込む。
        unsafe { pixel.write_volatile(value) };
    }

    /// 矩形の画面内部分を `(x0, x1)` (x1 は含まない) にクリップした範囲。
    fn clip_x(&self, x: i64, w: u64) -> Option<(usize, usize)> {
        let x0 = x.max(0);
        let x1 = x
            .saturating_add(i64::try_from(w).unwrap_or(i64::MAX))
            .min(self.width as i64);
        (x0 < x1).then_some((x0 as usize, x1 as usize))
    }

    fn clip_y(&self, y: i64, h: u64) -> Option<(usize, usize)> {
        let y0 = y.max(0);
        let y1 = y
            .saturating_add(i64::try_from(h).unwrap_or(i64::MAX))
            .min(self.height as i64);
        (y0 < y1).then_some((y0 as usize, y1 as usize))
    }

    /// 単色の矩形を描く(画面外はクリップ)。
    pub fn fill_rect(&mut self, x: i64, y: i64, w: u64, h: u64, color: Rgb) {
        let (Some((x0, x1)), Some((y0, y1))) = (self.clip_x(x, w), self.clip_y(y, h)) else {
            return;
        };
        let v = self.format.encode(color);
        for py in y0..y1 {
            for px in x0..x1 {
                self.write_raw(px, py, v);
            }
        }
    }

    /// 矩形内の各ピクセルの色を `shade(x, y)` で決めて描く(画面外はクリップ)。
    pub fn fill_rect_with(
        &mut self,
        x: i64,
        y: i64,
        w: u64,
        h: u64,
        mut shade: impl FnMut(i64, i64) -> Rgb,
    ) {
        let (Some((x0, x1)), Some((y0, y1))) = (self.clip_x(x, w), self.clip_y(y, h)) else {
            return;
        };
        for py in y0..y1 {
            for px in x0..x1 {
                let v = self.format.encode(shade(px as i64, py as i64));
                self.write_raw(px, py, v);
            }
        }
    }

    /// 角丸矩形を `shade(x, y)` の色で塗る(半径 `radius`、画面外はクリップ)。
    pub fn fill_rounded_rect_with(
        &mut self,
        x: i64,
        y: i64,
        w: u64,
        h: u64,
        radius: u64,
        mut shade: impl FnMut(i64, i64) -> Rgb,
    ) {
        let r = radius.min(w / 2).min(h / 2);
        for row in 0..h {
            // 角の円弧から、この行の左右の食い込み量を求める
            let dy = if row < r {
                r - row
            } else if row >= h - r {
                row + 1 - (h - r)
            } else {
                0
            };
            let inset = if dy == 0 {
                0
            } else {
                r - (r * r - (dy - 1) * (dy - 1)).isqrt()
            };
            if inset * 2 >= w {
                continue;
            }
            let py = y + row as i64;
            self.fill_rect_with(x + inset as i64, py, w - inset * 2, 1, &mut shade);
        }
    }

    /// 円を `shade(x, y)` の色で塗る。
    pub fn fill_circle_with(
        &mut self,
        cx: i64,
        cy: i64,
        radius: u64,
        mut shade: impl FnMut(i64, i64) -> Rgb,
    ) {
        let r = radius as i64;
        for dy in -r..=r {
            let half = ((r * r - dy * dy) as u64).isqrt() as i64;
            self.fill_rect_with(cx - half, cy + dy, (half * 2 + 1) as u64, 1, &mut shade);
        }
    }
}

impl fmt::Debug for Framebuffer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Framebuffer")
            .field("base", &self.base)
            .field("width", &self.width)
            .field("height", &self.height)
            .field("pitch", &self.pitch)
            .field("format", &self.format)
            .finish()
    }
}

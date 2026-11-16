//! 所有バッファの画像 ([`Image`] = [`Surface`]) と、借用の読み取り専用ビュー ([`ImageView`])。

use alloc::vec::Vec;

use crate::canvas::Canvas;
use crate::color::Color;
use crate::error::GfxError;
use crate::rect::Rect;

/// 所有バッファの画像。ピクセルは ARGB8888 (straight alpha)、行は隙間なく並ぶ (stride == 幅)。
///
/// 描画先としては [`Image::canvas`] で [`Canvas`] を借りて使う。[`Surface`] は同じ型の別名。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Image {
    width: u32,
    height: u32,
    data: Vec<u32>,
}

/// 描画先として使う [`Image`] の別名。
pub type Surface = Image;

impl Image {
    /// 画素数の上限 (2^28 = 約 2.7 億画素 = 1 GiB)。これを超える生成は [`GfxError::TooLarge`]。
    pub const MAX_PIXELS: u64 = 1 << 28;

    /// 全面が透明 (`0x00000000`) の画像を作る。
    pub fn new(width: u32, height: u32) -> Result<Image, GfxError> {
        Image::filled(width, height, Color::TRANSPARENT)
    }

    /// 全面を `color` で塗った画像を作る。メモリ確保に失敗したら [`GfxError::OutOfMemory`]。
    pub fn filled(width: u32, height: u32, color: Color) -> Result<Image, GfxError> {
        if width > i32::MAX as u32 || height > i32::MAX as u32 {
            return Err(GfxError::InvalidDimensions);
        }
        let n = width as u64 * height as u64;
        if n > Self::MAX_PIXELS {
            return Err(GfxError::TooLarge);
        }
        let mut data: Vec<u32> = Vec::new();
        data.try_reserve_exact(n as usize)
            .map_err(|_| GfxError::OutOfMemory)?;
        data.resize(n as usize, color.0);
        Ok(Image {
            width,
            height,
            data,
        })
    }

    /// 既存のピクセル列 (行優先、stride == `width`) から作る。長さが `width * height` と違えば
    /// [`GfxError::BufferTooSmall`] / [`GfxError::InvalidDimensions`]。
    pub fn from_pixels(width: u32, height: u32, data: Vec<u32>) -> Result<Image, GfxError> {
        if width > i32::MAX as u32 || height > i32::MAX as u32 {
            return Err(GfxError::InvalidDimensions);
        }
        let n = width as u64 * height as u64;
        if (data.len() as u64) < n {
            return Err(GfxError::BufferTooSmall);
        }
        if data.len() as u64 != n {
            return Err(GfxError::InvalidDimensions);
        }
        Ok(Image {
            width,
            height,
            data,
        })
    }

    /// 幅。
    #[inline]
    pub fn width(&self) -> u32 {
        self.width
    }
    /// 高さ。
    #[inline]
    pub fn height(&self) -> u32 {
        self.height
    }
    /// 画像全体の矩形 (原点 0,0)。
    #[inline]
    pub fn bounds(&self) -> Rect {
        Rect::from_size(self.width, self.height)
    }
    /// ピクセル列 (行優先)。
    #[inline]
    pub fn pixels(&self) -> &[u32] {
        &self.data
    }
    /// ピクセル列 (行優先、書き込み可)。
    #[inline]
    pub fn pixels_mut(&mut self) -> &mut [u32] {
        &mut self.data
    }
    /// ピクセル列を取り出して画像を消費する。
    pub fn into_pixels(self) -> Vec<u32> {
        self.data
    }

    /// 読み取り専用ビュー。
    #[inline]
    pub fn view(&self) -> ImageView<'_> {
        ImageView {
            data: &self.data,
            width: self.width,
            height: self.height,
            stride: self.width as usize,
        }
    }

    /// 描画用の [`Canvas`] を借りる。
    #[inline]
    pub fn canvas(&mut self) -> Canvas<'_> {
        // 寸法とバッファ長は構築時に検証済みなので失敗しない。
        Canvas::from_parts_unchecked(&mut self.data, self.width, self.height, self.width as usize)
    }

    /// (x, y) のピクセル。範囲外は `None`。
    pub fn get(&self, x: u32, y: u32) -> Option<Color> {
        if x >= self.width || y >= self.height {
            return None;
        }
        Some(Color(
            self.data[y as usize * self.width as usize + x as usize],
        ))
    }

    /// 全体のアルファに `opacity / 255` を掛ける (全体アルファ乗算)。
    pub fn multiply_alpha(&mut self, opacity: u8) {
        if opacity == 255 {
            return;
        }
        for p in self.data.iter_mut() {
            *p = Color(*p).mul_alpha(opacity).0;
        }
    }

    /// 矩形部分を切り出して新しい画像にする (範囲は画像内に切り詰める)。
    pub fn crop(&self, rect: Rect) -> Result<Image, GfxError> {
        let r = rect.intersect(&self.bounds());
        let mut out = Image::new(r.w, r.h)?;
        if !r.is_empty() {
            let v = self.view().sub_view(r).ok_or(GfxError::InvalidDimensions)?;
            out.canvas().blit(&v, 0, 0);
        }
        Ok(out)
    }

    /// 最近傍で `new_w` x `new_h` に拡大縮小した新しい画像。
    pub fn scaled(
        &self,
        new_w: u32,
        new_h: u32,
        filter: crate::canvas::Filter,
    ) -> Result<Image, GfxError> {
        let mut out = Image::new(new_w, new_h)?;
        let dst = out.bounds();
        out.canvas().blit_scaled(
            &self.view(),
            self.bounds(),
            dst,
            filter,
            crate::canvas::BlendMode::Copy,
            255,
        );
        Ok(out)
    }
}

/// 借用した読み取り専用のピクセル領域 (幅・高さ・stride 付き)。
#[derive(Clone, Copy, Debug)]
pub struct ImageView<'a> {
    pub(crate) data: &'a [u32],
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) stride: usize,
}

impl<'a> ImageView<'a> {
    /// 検証付きで作る。`stride >= width` かつ `data.len() >= stride*(height-1) + width` が必要。
    pub fn new(
        data: &'a [u32],
        width: u32,
        height: u32,
        stride: usize,
    ) -> Result<ImageView<'a>, GfxError> {
        check_geometry(data.len(), width, height, stride)?;
        Ok(ImageView {
            data,
            width,
            height,
            stride,
        })
    }

    /// 幅。
    #[inline]
    pub fn width(&self) -> u32 {
        self.width
    }
    /// 高さ。
    #[inline]
    pub fn height(&self) -> u32 {
        self.height
    }
    /// stride (1 行の要素数)。
    #[inline]
    pub fn stride(&self) -> usize {
        self.stride
    }
    /// 全体の矩形 (原点 0,0)。
    #[inline]
    pub fn bounds(&self) -> Rect {
        Rect::from_size(self.width, self.height)
    }

    /// (x, y) のピクセル。範囲外は `None`。
    #[inline]
    pub fn get(&self, x: u32, y: u32) -> Option<Color> {
        if x >= self.width || y >= self.height {
            return None;
        }
        Some(Color(self.data[y as usize * self.stride + x as usize]))
    }

    /// 1 行 (幅ぶん)。`y` が範囲外なら `None`。
    #[inline]
    pub fn row(&self, y: u32) -> Option<&'a [u32]> {
        if y >= self.height {
            return None;
        }
        let s = y as usize * self.stride;
        Some(&self.data[s..s + self.width as usize])
    }

    /// 部分矩形のビュー (矩形はビュー内に切り詰める)。空になれば `None`。
    pub fn sub_view(&self, rect: Rect) -> Option<ImageView<'a>> {
        let r = rect.intersect(&self.bounds());
        if r.is_empty() {
            return None;
        }
        let off = r.y as usize * self.stride + r.x as usize;
        Some(ImageView {
            data: &self.data[off..],
            width: r.w,
            height: r.h,
            stride: self.stride,
        })
    }

    /// 所有する [`Image`] にコピーする。
    pub fn to_image(&self) -> Result<Image, GfxError> {
        let mut out = Image::new(self.width, self.height)?;
        out.canvas().blit(self, 0, 0);
        Ok(out)
    }
}

/// `len` 要素のバッファが `width` x `height` / `stride` で安全に使えるかを検証する。
pub(crate) fn check_geometry(
    len: usize,
    width: u32,
    height: u32,
    stride: usize,
) -> Result<(), GfxError> {
    if width > i32::MAX as u32 || height > i32::MAX as u32 {
        return Err(GfxError::InvalidDimensions);
    }
    if width == 0 || height == 0 {
        return Ok(());
    }
    if stride < width as usize {
        return Err(GfxError::InvalidDimensions);
    }
    let need = (height as usize - 1)
        .checked_mul(stride)
        .and_then(|v| v.checked_add(width as usize))
        .ok_or(GfxError::TooLarge)?;
    if len < need {
        return Err(GfxError::BufferTooSmall);
    }
    Ok(())
}

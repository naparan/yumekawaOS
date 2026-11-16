//! 共通のエラー型。

use core::fmt;

/// バッファ・画像の生成に関するエラー。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GfxError {
    /// 幅・高さ・stride が不正 (stride < 幅、`i32::MAX` を超える寸法など)。
    InvalidDimensions,
    /// 渡されたバッファが `stride * (height - 1) + width` 要素に足りない。
    BufferTooSmall,
    /// 画素数が上限 ([`crate::Image::MAX_PIXELS`]) を超える、またはバイト数が `usize` に収まらない。
    TooLarge,
    /// メモリ確保に失敗した (`try_reserve` が失敗)。
    OutOfMemory,
}

impl fmt::Display for GfxError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            GfxError::InvalidDimensions => "invalid width/height/stride",
            GfxError::BufferTooSmall => "pixel buffer is too small for the given geometry",
            GfxError::TooLarge => "image is too large",
            GfxError::OutOfMemory => "out of memory",
        };
        f.write_str(s)
    }
}

impl core::error::Error for GfxError {}

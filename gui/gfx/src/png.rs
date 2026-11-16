//! PNG デコード (`zune-png` を使用。inflate・フィルタ・Adam7 は自作しない)。
//!
//! # 対応範囲
//!
//! | 項目 | 対応 |
//! | --- | --- |
//! | カラータイプ | グレースケール / グレー+アルファ / RGB / RGBA / パレット (tRNS 含む) |
//! | ビット深度 | 1, 2, 4, 8, 16 (16bit は上位 8bit に落とす) |
//! | インターレース | なし / Adam7 |
//! | APNG | 通常の画像 (既定フレーム) だけをデコードし、アニメーションは無視 |
//! | 色空間・ガンマ・ICC | 無視 (チャンクの値をそのまま sRGB として扱う) |
//!
//! # 入力の安全性
//!
//! 展開前に IHDR を自分で検証し、[`PngLimits`] (幅・高さ・総画素数) を超える画像は
//! メモリを確保する前に [`PngError::TooLarge`] で拒否する。不正・切り詰められた入力は
//! `Err` を返す (パニックしないことをホストのファジングテストで確認している)。

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

use zune_core::bytestream::ZCursor;
use zune_core::options::DecoderOptions;
use zune_png::PngDecoder;

use crate::color::Color;
use crate::error::GfxError;
use crate::image::Image;

/// デコード可能なサイズの上限。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PngLimits {
    /// 最大の幅 (px)
    pub max_width: u32,
    /// 最大の高さ (px)
    pub max_height: u32,
    /// 最大の総画素数 (出力は 1 画素 4 バイト。デコード中の一時バッファはさらに最大 約 2x)
    pub max_pixels: u64,
    /// true なら各チャンクの CRC を検証する (壊れた画像を確実に拒否する)。
    pub verify_crc: bool,
}

impl Default for PngLimits {
    /// 8192 x 8192、1600 万画素 (出力 64 MiB)、CRC 検証あり。
    fn default() -> Self {
        PngLimits {
            max_width: 8192,
            max_height: 8192,
            max_pixels: 16 * 1024 * 1024,
            verify_crc: true,
        }
    }
}

/// PNG のヘッダ情報 (IHDR)。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PngInfo {
    /// 幅
    pub width: u32,
    /// 高さ
    pub height: u32,
    /// ビット深度 (1/2/4/8/16)
    pub bit_depth: u8,
    /// カラータイプ (0 グレー, 2 RGB, 3 パレット, 4 グレー+アルファ, 6 RGBA)
    pub color_type: u8,
    /// Adam7 インターレースか
    pub interlaced: bool,
}

/// PNG デコードのエラー。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PngError {
    /// PNG として不正 (シグネチャ、IHDR、データ破損など)。
    Malformed(String),
    /// 未対応の形式。
    Unsupported(&'static str),
    /// 制限 ([`PngLimits`]) を超えている。
    TooLarge {
        /// 画像の幅
        width: u32,
        /// 画像の高さ
        height: u32,
    },
    /// メモリ不足。
    OutOfMemory,
}

impl fmt::Display for PngError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PngError::Malformed(m) => write!(f, "malformed PNG: {m}"),
            PngError::Unsupported(m) => write!(f, "unsupported PNG: {m}"),
            PngError::TooLarge { width, height } => {
                write!(f, "PNG too large: {width}x{height}")
            }
            PngError::OutOfMemory => f.write_str("out of memory while decoding PNG"),
        }
    }
}

impl core::error::Error for PngError {}

impl From<GfxError> for PngError {
    fn from(e: GfxError) -> Self {
        match e {
            GfxError::OutOfMemory => PngError::OutOfMemory,
            _ => PngError::Malformed(String::from("bad dimensions")),
        }
    }
}

const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

/// IHDR だけを解析してヘッダ情報を返す (展開はしない)。
pub fn png_info(data: &[u8]) -> Result<PngInfo, PngError> {
    if data.len() < 8 || data[..8] != SIGNATURE {
        return Err(PngError::Malformed(String::from("bad signature")));
    }
    // 8 (署名) + 4 (長さ) + 4 ("IHDR") + 13 (本体)
    if data.len() < 8 + 8 + 13 {
        return Err(PngError::Malformed(String::from("truncated IHDR")));
    }
    let len = u32::from_be_bytes([data[8], data[9], data[10], data[11]]);
    if len != 13 || &data[12..16] != b"IHDR" {
        return Err(PngError::Malformed(String::from("first chunk is not IHDR")));
    }
    let b = &data[16..29];
    let width = u32::from_be_bytes([b[0], b[1], b[2], b[3]]);
    let height = u32::from_be_bytes([b[4], b[5], b[6], b[7]]);
    let (bit_depth, color_type, compression, filter, interlace) = (b[8], b[9], b[10], b[11], b[12]);
    if width == 0 || height == 0 || width > i32::MAX as u32 || height > i32::MAX as u32 {
        return Err(PngError::Malformed(String::from("invalid dimensions")));
    }
    let depth_ok = match color_type {
        0 => matches!(bit_depth, 1 | 2 | 4 | 8 | 16),
        3 => matches!(bit_depth, 1 | 2 | 4 | 8),
        2 | 4 | 6 => matches!(bit_depth, 8 | 16),
        _ => return Err(PngError::Malformed(String::from("invalid color type"))),
    };
    if !depth_ok {
        return Err(PngError::Malformed(String::from(
            "invalid bit depth for color type",
        )));
    }
    if compression != 0 || filter != 0 {
        return Err(PngError::Unsupported("compression/filter method"));
    }
    if interlace > 1 {
        return Err(PngError::Malformed(String::from(
            "invalid interlace method",
        )));
    }
    Ok(PngInfo {
        width,
        height,
        bit_depth,
        color_type,
        interlaced: interlace == 1,
    })
}

/// PNG をデコードして ARGB8888 の [`Image`] にする。
///
/// 大きすぎる画像 (`limits` 超過) は、確保・展開の前に [`PngError::TooLarge`] を返す。
pub fn decode_png(data: &[u8], limits: &PngLimits) -> Result<Image, PngError> {
    let info = png_info(data)?;
    if info.width > limits.max_width
        || info.height > limits.max_height
        || info.width as u64 * info.height as u64 > limits.max_pixels
    {
        return Err(PngError::TooLarge {
            width: info.width,
            height: info.height,
        });
    }
    // 出力 (Image) の確保に失敗する/上限を超えるなら、先に拒否する。
    let mut out = Image::new(info.width, info.height).map_err(|e| match e {
        GfxError::TooLarge => PngError::TooLarge {
            width: info.width,
            height: info.height,
        },
        other => PngError::from(other),
    })?;

    let options = DecoderOptions::default()
        .set_max_width(limits.max_width as usize)
        .set_max_height(limits.max_height as usize)
        .png_set_strip_to_8bit(true)
        .png_set_confirm_crc(limits.verify_crc)
        .png_set_decode_animated(false);
    let mut dec = PngDecoder::new_with_options(ZCursor::new(data), options);
    let raw: Vec<u8> = dec
        .decode_raw()
        .map_err(|e| PngError::Malformed(alloc::format!("{e:?}").trim().into()))?;

    let comps = dec
        .colorspace()
        .ok_or(PngError::Malformed(String::from("no colorspace")))?
        .num_components();
    let (w, h) = (info.width as usize, info.height as usize);
    if !(1..=4).contains(&comps) || raw.len() < w * h * comps {
        return Err(PngError::Malformed(String::from("unexpected output size")));
    }
    let px = out.pixels_mut();
    match comps {
        1 => {
            for (p, &g) in px.iter_mut().zip(raw.iter()) {
                *p = Color::rgb(g, g, g).0;
            }
        }
        2 => {
            for (p, c) in px.iter_mut().zip(raw.chunks_exact(2)) {
                *p = Color::argb(c[1], c[0], c[0], c[0]).0;
            }
        }
        3 => {
            for (p, c) in px.iter_mut().zip(raw.chunks_exact(3)) {
                *p = Color::rgb(c[0], c[1], c[2]).0;
            }
        }
        _ => {
            for (p, c) in px.iter_mut().zip(raw.chunks_exact(4)) {
                *p = Color::argb(c[3], c[0], c[1], c[2]).0;
            }
        }
    }
    Ok(out)
}

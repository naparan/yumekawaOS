//! [`Canvas`]: 借用したピクセルバッファへの描画面。
//!
//! # 安全性 (§31 バッファ境界 / §32)
//!
//! - `Canvas` は `&mut [u32]` と幅・高さ・stride を保持し、生成時に
//!   `stride >= width` かつ `buf.len() >= stride*(height-1) + width` を検証する。
//! - すべての描画は **クリップ矩形** (常にキャンバス範囲の内側) との交差を `i64` で計算してから
//!   `usize` に落とし、行スライスを安全な添字で取得して書き込む。`unsafe` は使わないので、
//!   範囲外書き込みは構造的に不可能 (論理バグがあってもメモリ破壊ではなくパニックになる。
//!   テストでランダムな巨大座標を与えてパニックしないことを確認している)。
//! - 座標・サイズは `i32` / `u32` で受け取り、内部計算は `i64` (一部 `i128`) で行う。

use alloc::vec::Vec;

use crate::color::{Color, blend_cov, blend_over};
use crate::error::GfxError;
use crate::image::{ImageView, check_geometry};
use crate::rect::Rect;

/// 8bit カバレッジのマスク (グリフなど)。`data` は行優先で、1 行は `stride` 要素。
#[derive(Clone, Copy, Debug)]
pub struct MaskRef<'a> {
    /// 幅
    pub width: u32,
    /// 高さ
    pub height: u32,
    /// 1 行の要素数 (`>= width`)
    pub stride: usize,
    /// カバレッジ (0 = 透明, 255 = 完全)
    pub data: &'a [u8],
}

impl<'a> MaskRef<'a> {
    /// マスクを作る (検証は描画時に行い、足りなければ描かない)。
    pub const fn new(width: u32, height: u32, data: &'a [u8], stride: usize) -> MaskRef<'a> {
        MaskRef {
            width,
            height,
            stride,
            data,
        }
    }
}

/// 転送時の合成方法。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlendMode {
    /// 不透明コピー (アルファも含めて上書き)。`opacity` は無視される。
    Copy,
    /// source-over (アルファ合成)。
    SourceOver,
}

/// 拡大縮小時の補間方法。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Filter {
    /// 最近傍。
    Nearest,
    /// バイリニア (アルファ重み付き)。大きな縮小 (1/2 未満) ではエイリアシングが出る。
    Bilinear,
}

/// 借用バッファへの描画面。
#[derive(Debug)]
pub struct Canvas<'a> {
    buf: &'a mut [u32],
    width: u32,
    height: u32,
    stride: usize,
    clip: Rect,
}

impl<'a> Canvas<'a> {
    /// 検証付きで作る。`stride` は 1 行あたりの要素数 (`>= width`)。
    pub fn new(
        buf: &'a mut [u32],
        width: u32,
        height: u32,
        stride: usize,
    ) -> Result<Canvas<'a>, GfxError> {
        check_geometry(buf.len(), width, height, stride)?;
        Ok(Canvas::from_parts_unchecked(buf, width, height, stride))
    }

    /// 検証済みの値から作る (クレート内部用)。
    pub(crate) fn from_parts_unchecked(
        buf: &'a mut [u32],
        width: u32,
        height: u32,
        stride: usize,
    ) -> Canvas<'a> {
        Canvas {
            buf,
            width,
            height,
            stride,
            clip: Rect::from_size(width, height),
        }
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
    /// キャンバス全体の矩形 (原点 0,0)。
    #[inline]
    pub fn bounds(&self) -> Rect {
        Rect::from_size(self.width, self.height)
    }
    /// 現在のクリップ矩形 (常にキャンバス範囲内)。
    #[inline]
    pub fn clip(&self) -> Rect {
        self.clip
    }
    /// クリップ矩形を設定する。キャンバス範囲との共通部分に切り詰められる。
    pub fn set_clip(&mut self, rect: Rect) {
        self.clip = rect.intersect(&self.bounds());
    }
    /// クリップをキャンバス全体に戻す。
    pub fn reset_clip(&mut self) {
        self.clip = self.bounds();
    }

    /// 同じバッファを短い寿命で借り直す (関数に渡すときに使う)。クリップは引き継ぐ。
    pub fn reborrow(&mut self) -> Canvas<'_> {
        Canvas {
            buf: &mut *self.buf,
            width: self.width,
            height: self.height,
            stride: self.stride,
            clip: self.clip,
        }
    }

    /// 矩形領域だけを切り出した子キャンバス。子の座標原点は `rect` の左上になり、
    /// 子へのクリップは `rect` で自動的に制限される。`rect` はキャンバス範囲に切り詰められ、
    /// 空になる場合は `None`。(親のクリップ矩形は継承しない。バッファ範囲のみ。)
    pub fn sub_canvas(&mut self, rect: Rect) -> Option<Canvas<'_>> {
        let r = rect.intersect(&self.bounds());
        if r.is_empty() {
            return None;
        }
        let off = r.y as usize * self.stride + r.x as usize;
        Some(Canvas {
            buf: &mut self.buf[off..],
            width: r.w,
            height: r.h,
            stride: self.stride,
            clip: Rect::from_size(r.w, r.h),
        })
    }

    /// 読み取り専用ビュー。
    pub fn as_view(&self) -> ImageView<'_> {
        ImageView {
            data: &*self.buf,
            width: self.width,
            height: self.height,
            stride: self.stride,
        }
    }

    // ---- ピクセル ----

    /// (x, y) のピクセル。キャンバス範囲外は `None` (クリップは無関係)。
    #[inline]
    pub fn get_pixel(&self, x: i32, y: i32) -> Option<Color> {
        if x < 0 || y < 0 || x as u32 >= self.width || y as u32 >= self.height {
            return None;
        }
        Some(Color(self.buf[y as usize * self.stride + x as usize]))
    }

    /// (x, y) に `color` をそのまま書き込む (合成しない)。クリップ外なら何もせず `false`。
    #[inline]
    pub fn set_pixel(&mut self, x: i32, y: i32, color: Color) -> bool {
        if !self.clip.contains_point(x, y) {
            return false;
        }
        self.buf[y as usize * self.stride + x as usize] = color.0;
        true
    }

    /// (x, y) に `color` を source-over で合成する。クリップ外なら何もせず `false`。
    #[inline]
    pub fn blend_pixel(&mut self, x: i32, y: i32, color: Color) -> bool {
        if !self.clip.contains_point(x, y) {
            return false;
        }
        let p = &mut self.buf[y as usize * self.stride + x as usize];
        *p = blend_over(*p, color.0);
        true
    }

    /// クリップ内を `color` で上書きする (合成しない)。
    pub fn clear(&mut self, color: Color) {
        let c = self.clip;
        self.clear_rect(c, color);
    }

    // ---- 矩形 ----

    /// 矩形を `rect ∩ クリップ` に切り詰めて (x0, y0, x1, y1) を `usize` で返す。
    #[inline]
    pub(crate) fn clipped(&self, rect: &Rect) -> Option<(usize, usize, usize, usize)> {
        let r = rect.intersect(&self.clip);
        if r.is_empty() {
            return None;
        }
        Some((
            r.x as usize,
            r.y as usize,
            r.x as usize + r.w as usize,
            r.y as usize + r.h as usize,
        ))
    }

    /// 1 行の `[x0, x1)` の可変スライス。呼び出し側がクリップ済みの値を渡すこと
    /// (範囲外なら安全な添字チェックでパニックする = 書き込みは起きない)。
    #[inline]
    pub(crate) fn row_mut(&mut self, y: usize, x0: usize, x1: usize) -> &mut [u32] {
        let s = y * self.stride;
        &mut self.buf[s + x0..s + x1]
    }

    /// 1 行 `y` の `[x0, x1)` (クリップ済み) を `color` で source-over 合成する。
    #[inline]
    pub(crate) fn blend_span(&mut self, y: usize, x0: usize, x1: usize, color: Color) {
        let a = color.a();
        if a == 0 {
            return;
        }
        let row = self.row_mut(y, x0, x1);
        if a == 255 {
            row.fill(color.0);
        } else {
            for p in row {
                *p = blend_over(*p, color.0);
            }
        }
    }

    /// 矩形を `color` で塗る (source-over。`a == 255` なら単純な塗りつぶしの高速経路)。
    pub fn fill_rect(&mut self, rect: Rect, color: Color) {
        let Some((x0, y0, x1, y1)) = self.clipped(&rect) else {
            return;
        };
        for y in y0..y1 {
            self.blend_span(y, x0, x1, color);
        }
    }

    /// 矩形を `color` で上書きする (合成しない。透明色で穴をあけるのにも使える)。
    pub fn clear_rect(&mut self, rect: Rect, color: Color) {
        let Some((x0, y0, x1, y1)) = self.clipped(&rect) else {
            return;
        };
        for y in y0..y1 {
            self.row_mut(y, x0, x1).fill(color.0);
        }
    }

    /// 矩形の枠 (内側に `thickness` px)。`thickness == 0` なら何もしない。
    /// 厚みが矩形の半分以上なら全面を塗る。
    pub fn stroke_rect(&mut self, rect: Rect, thickness: u32, color: Color) {
        if rect.is_empty() || thickness == 0 {
            return;
        }
        if thickness as u64 * 2 >= rect.w.min(rect.h) as u64 {
            self.fill_rect(rect, color);
            return;
        }
        let t = thickness;
        // 上下 (幅いっぱい) と左右 (上下を除いた部分)。重複なし。
        self.fill_rect(Rect::new(rect.x, rect.y, rect.w, t), color);
        let bottom_y = (rect.bottom() - t as i64).clamp(i32::MIN as i64, i32::MAX as i64) as i32;
        self.fill_rect(Rect::new(rect.x, bottom_y, rect.w, t), color);
        let inner_y = (rect.top() + t as i64).clamp(i32::MIN as i64, i32::MAX as i64) as i32;
        let inner_h = rect.h - 2 * t;
        self.fill_rect(Rect::new(rect.x, inner_y, t, inner_h), color);
        let right_x = (rect.right() - t as i64).clamp(i32::MIN as i64, i32::MAX as i64) as i32;
        self.fill_rect(Rect::new(right_x, inner_y, t, inner_h), color);
    }

    // ---- マスク (グリフ等) ----

    /// 8bit カバレッジ `mask` を `color` で `(x, y)` に描く。各ピクセルは `color` のアルファに
    /// マスク値を掛けて source-over する。マスクのデータが足りない部分は描かずに終わる
    /// (パニックしない)。
    pub fn draw_mask(&mut self, x: i32, y: i32, mask: &MaskRef<'_>, color: Color) {
        let (w, h) = (mask.width, mask.height);
        if w == 0 || h == 0 || color.a() == 0 || mask.stride < w as usize {
            return;
        }
        let dst_rect =
            Rect::from_ltrb(x as i64, y as i64, x as i64 + w as i64, y as i64 + h as i64);
        let Some((x0, y0, x1, y1)) = self.clipped(&dst_rect) else {
            return;
        };
        let src = color.0;
        for dy in y0..y1 {
            let my = (dy as i64 - y as i64) as usize;
            let mx0 = (x0 as i64 - x as i64) as usize;
            let n = x1 - x0;
            let Some(mrow) = my
                .checked_mul(mask.stride)
                .and_then(|v| v.checked_add(mx0))
                .and_then(|s| mask.data.get(s..s.checked_add(n)?))
            else {
                return;
            };
            let row = self.row_mut(dy, x0, x1);
            for (p, &m) in row.iter_mut().zip(mrow) {
                if m != 0 {
                    *p = blend_cov(*p, src, m as u32);
                }
            }
        }
    }

    // ---- 画像転送 ----

    /// `src` 全体を `(dx, dy)` に不透明コピーする。
    pub fn blit(&mut self, src: &ImageView<'_>, dx: i32, dy: i32) {
        self.blit_ex(src, src.bounds(), dx, dy, BlendMode::Copy, 255);
    }

    /// `src` の一部 `src_rect` を `(dx, dy)` に不透明コピーする。
    pub fn blit_region(&mut self, src: &ImageView<'_>, src_rect: Rect, dx: i32, dy: i32) {
        self.blit_ex(src, src_rect, dx, dy, BlendMode::Copy, 255);
    }

    /// `src` 全体を `(dx, dy)` にアルファ合成 (source-over) する。
    pub fn blit_alpha(&mut self, src: &ImageView<'_>, dx: i32, dy: i32) {
        self.blit_ex(src, src.bounds(), dx, dy, BlendMode::SourceOver, 255);
    }

    /// `src` 全体を、全体アルファ `opacity` (0..=255) を掛けて合成する。
    pub fn blit_opacity(&mut self, src: &ImageView<'_>, dx: i32, dy: i32, opacity: u8) {
        self.blit_ex(src, src.bounds(), dx, dy, BlendMode::SourceOver, opacity);
    }

    /// 転送の一般形。`src_rect` (ソース内に切り詰められる) を `(dx, dy)` へ。
    /// `BlendMode::SourceOver` では `opacity` を各ピクセルのアルファに掛ける。
    pub fn blit_ex(
        &mut self,
        src: &ImageView<'_>,
        src_rect: Rect,
        dx: i32,
        dy: i32,
        mode: BlendMode,
        opacity: u8,
    ) {
        let sr = src_rect.intersect(&src.bounds());
        if sr.is_empty() {
            return;
        }
        // sr の左上が着地する座標 (i64)。
        let ox = dx as i64 + (sr.x as i64 - src_rect.x as i64);
        let oy = dy as i64 + (sr.y as i64 - src_rect.y as i64);
        let l = ox.max(self.clip.left());
        let t = oy.max(self.clip.top());
        let r = (ox + sr.w as i64).min(self.clip.right());
        let b = (oy + sr.h as i64).min(self.clip.bottom());
        if r <= l || b <= t {
            return;
        }
        let n = (r - l) as usize;
        if mode == BlendMode::SourceOver && opacity == 0 {
            return;
        }
        for y in t..b {
            let sy = (sr.y as i64 + (y - oy)) as usize;
            let sx = (sr.x as i64 + (l - ox)) as usize;
            let sstart = sy * src.stride + sx;
            let srow = &src.data[sstart..sstart + n];
            let drow = self.row_mut(y as usize, l as usize, r as usize);
            match mode {
                BlendMode::Copy => drow.copy_from_slice(srow),
                BlendMode::SourceOver => {
                    if opacity == 255 {
                        for (d, &s) in drow.iter_mut().zip(srow) {
                            *d = blend_over(*d, s);
                        }
                    } else {
                        for (d, &s) in drow.iter_mut().zip(srow) {
                            *d = blend_over(*d, Color(s).mul_alpha(opacity).0);
                        }
                    }
                }
            }
        }
    }

    /// 同じキャンバス内で矩形 `src_rect` を `(dx, dy)` へコピーする (重なっていてもよい。
    /// スクロールに使う)。ソースはキャンバス範囲、書き込み先はクリップに切り詰められる。
    pub fn copy_within(&mut self, src_rect: Rect, dx: i32, dy: i32) {
        let sr = src_rect.intersect(&self.bounds());
        if sr.is_empty() {
            return;
        }
        let ox = dx as i64 + (sr.x as i64 - src_rect.x as i64);
        let oy = dy as i64 + (sr.y as i64 - src_rect.y as i64);
        let l = ox.max(self.clip.left());
        let t = oy.max(self.clip.top());
        let r = (ox + sr.w as i64).min(self.clip.right());
        let b = (oy + sr.h as i64).min(self.clip.bottom());
        if r <= l || b <= t {
            return;
        }
        let n = (r - l) as usize;
        let sx = (sr.x as i64 + (l - ox)) as usize;
        let sy0 = (sr.y as i64 + (t - oy)) as usize;
        let rows = (b - t) as usize;
        let dxu = l as usize;
        let dy0 = t as usize;
        let do_row = |i: usize, buf: &mut [u32], stride: usize| {
            let s = (sy0 + i) * stride + sx;
            let d = (dy0 + i) * stride + dxu;
            buf.copy_within(s..s + n, d);
        };
        let stride = self.stride;
        if dy0 > sy0 {
            for i in (0..rows).rev() {
                do_row(i, self.buf, stride);
            }
        } else {
            for i in 0..rows {
                do_row(i, self.buf, stride);
            }
        }
    }

    /// `src` の `src_rect` を `dst_rect` に拡大縮小して転送する。`dst_rect` の一部だけが
    /// クリップ内にあるときは、その部分だけを (マッピングを保ったまま) 描く。
    /// `src_rect` はまずソース内に切り詰められる。
    pub fn blit_scaled(
        &mut self,
        src: &ImageView<'_>,
        src_rect: Rect,
        dst_rect: Rect,
        filter: Filter,
        mode: BlendMode,
        opacity: u8,
    ) {
        let sr = src_rect.intersect(&src.bounds());
        if sr.is_empty() || dst_rect.is_empty() {
            return;
        }
        if mode == BlendMode::SourceOver && opacity == 0 {
            return;
        }
        let vis = dst_rect.intersect(&self.clip);
        if vis.is_empty() {
            return;
        }
        let dw = dst_rect.w as u128;
        let dh = dst_rect.h as u128;
        let sw = sr.w as u128;
        let sh = sr.h as u128;

        // 列ごとの参照テーブル (x0, x1, frac)。
        let xs: Vec<(u32, u32, u32)> = (0..vis.w as i64)
            .map(|i| {
                let idx = (vis.x as i64 + i - dst_rect.x as i64) as u128;
                sample_pos(idx, dw, sw, filter)
            })
            .collect();
        let x_dst0 = vis.x as usize;
        let x_dst1 = x_dst0 + vis.w as usize;

        for j in 0..vis.h as i64 {
            let idy = (vis.y as i64 + j - dst_rect.y as i64) as u128;
            let (y0, y1, fy) = sample_pos(idy, dh, sh, filter);
            let dy = (vis.y as i64 + j) as usize;
            let row0 = (sr.y as usize + y0 as usize) * src.stride + sr.x as usize;
            let row1 = (sr.y as usize + y1 as usize) * src.stride + sr.x as usize;
            let src_data = src.data;
            let drow = self.row_mut(dy, x_dst0, x_dst1);
            for (d, &(xa, xb, fx)) in drow.iter_mut().zip(xs.iter()) {
                let s = match filter {
                    Filter::Nearest => src_data[row0 + xa as usize],
                    Filter::Bilinear => bilerp(
                        src_data[row0 + xa as usize],
                        src_data[row0 + xb as usize],
                        src_data[row1 + xa as usize],
                        src_data[row1 + xb as usize],
                        fx,
                        fy,
                    ),
                };
                match mode {
                    BlendMode::Copy => *d = s,
                    BlendMode::SourceOver => {
                        let s = if opacity == 255 {
                            s
                        } else {
                            Color(s).mul_alpha(opacity).0
                        };
                        *d = blend_over(*d, s);
                    }
                }
            }
        }
    }
}

/// 出力座標 `idx` (0..dst_len) に対応するソースの座標 (`i0`, `i1`, 補間係数 0..=255)。
/// ピクセル中心どうしを対応づける: `src = (idx + 0.5) * src_len / dst_len - 0.5`。
fn sample_pos(idx: u128, dst_len: u128, src_len: u128, filter: Filter) -> (u32, u32, u32) {
    match filter {
        Filter::Nearest => {
            let i = ((2 * idx + 1) * src_len) / (2 * dst_len);
            let i = i.min(src_len - 1) as u32;
            (i, i, 0)
        }
        Filter::Bilinear => {
            // 1/256 ピクセル単位の符号付き位置。
            let pos = (((2 * idx + 1) * src_len * 256) / (2 * dst_len)) as i128 - 128;
            let i0 = pos >> 8;
            let frac = (pos & 255) as u32;
            let last = src_len as i128 - 1;
            let a = i0.clamp(0, last) as u32;
            let b = (i0 + 1).clamp(0, last) as u32;
            (a, b, frac)
        }
    }
}

/// 4 ピクセルのバイリニア補間 (アルファ重み付き)。`fx`, `fy` は 0..=255。
fn bilerp(p00: u32, p01: u32, p10: u32, p11: u32, fx: u32, fy: u32) -> u32 {
    let w00 = (256 - fx) * (256 - fy);
    let w01 = fx * (256 - fy);
    let w10 = (256 - fx) * fy;
    let w11 = fx * fy;
    let a = [p00 >> 24, p01 >> 24, p10 >> 24, p11 >> 24];
    let ws = [w00, w01, w10, w11];
    let ps = [p00, p01, p10, p11];
    if a == [255; 4] {
        let ch = |sh: u32| -> u32 {
            let mut acc = 0u32;
            for k in 0..4 {
                acc += ws[k] * ((ps[k] >> sh) & 0xFF);
            }
            (acc + 32768) >> 16
        };
        return 0xFF00_0000 | (ch(16) << 16) | (ch(8) << 8) | ch(0);
    }
    let mut asum = 0u64;
    for k in 0..4 {
        asum += ws[k] as u64 * a[k] as u64;
    }
    if asum == 0 {
        return 0;
    }
    let ch = |sh: u32| -> u32 {
        let mut acc = 0u64;
        for k in 0..4 {
            acc += ws[k] as u64 * a[k] as u64 * ((ps[k] >> sh) & 0xFF) as u64;
        }
        ((acc + asum / 2) / asum) as u32
    };
    let oa = ((asum + 32768) >> 16) as u32;
    (oa << 24) | (ch(16) << 16) | (ch(8) << 8) | ch(0)
}

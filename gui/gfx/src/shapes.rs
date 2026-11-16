//! 図形の描画 (線・角丸矩形・円/楕円・グラデーション)。`Canvas` の `impl` の続き。
//!
//! 内側ループはすべて整数演算。アンチエイリアスは 1/16 ピクセル精度の整数距離計算
//! (角丸・太線) と、縦 4 サブサンプル x 横 1/16 精度の面積カバレッジ (楕円) で行う。

use alloc::vec::Vec;

use crate::canvas::Canvas;
use crate::color::{Color, blend_cov, div255};
use crate::rect::Rect;

/// 角丸の半径の上限 (内部の 1/16 精度の距離計算が `u64` に収まるようにするための制限)。
const MAX_RADIUS: u32 = 1 << 24;
/// 太線の太さの上限。
const MAX_THICKNESS: u32 = 1 << 16;

impl Canvas<'_> {
    // ---- 水平/垂直線 ----

    /// 水平線: `(x, y)` から右へ `len` ピクセル (1px 幅)。
    pub fn hline(&mut self, x: i32, y: i32, len: u32, color: Color) {
        self.fill_rect(Rect::new(x, y, len, 1), color);
    }

    /// 垂直線: `(x, y)` から下へ `len` ピクセル (1px 幅)。
    pub fn vline(&mut self, x: i32, y: i32, len: u32, color: Color) {
        self.fill_rect(Rect::new(x, y, 1, len), color);
    }

    // ---- 線 ----

    /// `(x0, y0)` から `(x1, y1)` への線 (両端のピクセルを含む)。
    ///
    /// - `thickness <= 1`: アンチエイリアスなしの Bresenham 線 (1px)。
    /// - 軸に平行で `thickness >= 2`: 角ばった端点の矩形 (整数ピクセル)。
    /// - 斜めで `thickness >= 2`: 丸い端点・アンチエイリアス付き ([`Canvas::line_aa`] と同じ)。
    ///
    /// 画面外にはみ出す線は切り取られる。巨大な座標でも、描くピクセル数はクリップ範囲で
    /// 頭打ちになる (時間は画面サイズに比例)。
    pub fn line(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, thickness: u32, color: Color) {
        if thickness == 0 || color.a() == 0 {
            return;
        }
        if thickness == 1 {
            self.line_1px(x0, y0, x1, y1, color);
            return;
        }
        let t = thickness.min(MAX_THICKNESS) as i64;
        if y0 == y1 || x0 == x1 {
            // 軸平行: 中心線から両側に振り分けた矩形。
            let (lx, rx) = (x0.min(x1) as i64, x0.max(x1) as i64);
            let (ty, by) = (y0.min(y1) as i64, y0.max(y1) as i64);
            let (a, b) = if y0 == y1 {
                (lx, by - (t - 1) / 2)
            } else {
                (lx - (t - 1) / 2, ty)
            };
            let r = if y0 == y1 {
                Rect::from_ltrb(a, b, rx + 1, b + t)
            } else {
                Rect::from_ltrb(a, b, a + t, by + 1)
            };
            self.fill_rect(r, color);
            return;
        }
        self.line_aa(x0, y0, x1, y1, thickness, color);
    }

    /// 1px の Bresenham 線 (アンチエイリアスなし)。クリップ範囲のぶんだけを反復する。
    fn line_1px(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, color: Color) {
        let (x0, y0, x1, y1) = (x0 as i64, y0 as i64, x1 as i64, y1 as i64);
        let (dx, dy) = (x1 - x0, y1 - y0);
        let (adx, ady) = (dx.abs(), dy.abs());
        let clip = self.clip();
        if clip.is_empty() {
            return;
        }
        // 主軸を決め、主軸方向に走査する。副軸の位置は閉じた式
        // `round(i * |d_minor| / |d_major|)` (半分は切り上げ) で各点ごとに求める。
        let x_major = adx >= ady;
        let (major_len, minor_len) = if x_major { (adx, ady) } else { (ady, adx) };
        let (m0, mn0, m_step, mn_step) = if x_major {
            (x0, y0, dx.signum(), dy.signum())
        } else {
            (y0, x0, dy.signum(), dx.signum())
        };
        // 主軸方向のクリップ範囲 (i は 0..=major_len の経過ステップ数)。
        let (mc0, mc1) = if x_major {
            (clip.left(), clip.right() - 1)
        } else {
            (clip.top(), clip.bottom() - 1)
        };
        // m = m0 + m_step * i が [mc0, mc1] に入る i の範囲。
        let (mut i_lo, mut i_hi) = (0i64, major_len);
        if m_step > 0 {
            i_lo = i_lo.max(mc0 - m0);
            i_hi = i_hi.min(mc1 - m0);
        } else if m_step < 0 {
            i_lo = i_lo.max(m0 - mc1);
            i_hi = i_hi.min(m0 - mc0);
        } else {
            // major_len == 0 (点)。
            if !(mc0..=mc1).contains(&m0) {
                return;
            }
        }
        for i in i_lo..=i_hi {
            let minor_off = if major_len == 0 {
                0
            } else if major_len < (1 << 30) {
                (2 * minor_len * i + major_len) / (2 * major_len)
            } else {
                ((2 * minor_len as i128 * i as i128 + major_len as i128) / (2 * major_len as i128))
                    as i64
            };
            let m = m0 + m_step * i;
            let mn = mn0 + mn_step * minor_off;
            let (px, py) = if x_major { (m, mn) } else { (mn, m) };
            if clip.contains_point_i64(px, py) {
                self.blend_pixel(px as i32, py as i32, color);
            }
        }
    }

    /// アンチエイリアス付きの太線 (丸い端点)。`thickness` はピクセル幅 (1 以上、0 は何もしない)。
    /// 端点は `(x, y)` のピクセル中心。時間は線の外接矩形 (クリップ後) の面積に比例する。
    pub fn line_aa(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, thickness: u32, color: Color) {
        if thickness == 0 || color.a() == 0 {
            return;
        }
        let t = thickness.min(MAX_THICKNESS) as i64;
        let pad = t / 2 + 2;
        let (lx, rx) = ((x0.min(x1)) as i64 - pad, (x0.max(x1)) as i64 + pad + 1);
        let (ty, by) = ((y0.min(y1)) as i64 - pad, (y0.max(y1)) as i64 + pad + 1);
        let area = Rect::from_ltrb(lx, ty, rx, by);
        let Some((cx0, cy0, cx1, cy1)) = self.clipped(&area) else {
            return;
        };
        // 1/16 ピクセル単位。端点はピクセル中心。距離計算は i128 (巨大座標でもオーバーフローしない)。
        let (ax, ay) = (x0 as i128 * 16, y0 as i128 * 16);
        let (dx, dy) = (
            (x1 as i128 - x0 as i128) * 16,
            (y1 as i128 - y0 as i128) * 16,
        );
        let len2 = (dx * dx + dy * dy) as u128;
        let len16 = len2.isqrt() as i128;
        let hw16 = t as i128 * 8;
        for y in cy0..cy1 {
            for x in cx0..cx1 {
                let (qx, qy) = (x as i128 * 16 - ax, y as i128 * 16 - ay);
                let dot = qx * dx + qy * dy;
                let dist16: i128 = if len2 == 0 || dot <= 0 {
                    ((qx * qx + qy * qy) as u128).isqrt() as i128
                } else if dot as u128 >= len2 {
                    let (ex, ey) = (qx - dx, qy - dy);
                    ((ex * ex + ey * ey) as u128).isqrt() as i128
                } else {
                    (qx * dy - qy * dx).abs() / len16
                };
                // 距離が (半幅 + 0.5px) 以下なら 1 ピクセルぶんの傾斜でカバレッジを決める。
                let c16 = (hw16 + 8 - dist16).clamp(0, 16);
                if c16 > 0 {
                    let cov = (c16 * 16).min(255) as u32;
                    let row = self.row_mut(y, x, x + 1);
                    row[0] = blend_cov(row[0], color.0, cov);
                }
            }
        }
    }

    // ---- 角丸矩形 ----

    /// 角丸矩形を塗る (角はアンチエイリアス付き)。`radius` は `min(w, h) / 2` に制限される。
    pub fn fill_round_rect(&mut self, rect: Rect, radius: u32, color: Color) {
        self.fill_round_rect_paint(rect, radius, Paint::Solid(color));
    }

    /// 縦方向のグラデーションで角丸矩形を塗る (上端が `top`、下端が `bottom`)。
    /// 色は矩形全体に対して決まるので、クリップを使えば「上だけ角丸のタイトルバー」も作れる。
    pub fn fill_round_rect_gradient_v(
        &mut self,
        rect: Rect,
        radius: u32,
        top: Color,
        bottom: Color,
    ) {
        let paint = Paint::V {
            top,
            bottom,
            origin: rect.top(),
            len: rect.h as u64,
        };
        self.fill_round_rect_paint(rect, radius, paint);
    }

    /// 横方向のグラデーションで角丸矩形を塗る (左端が `left`、右端が `right`)。
    pub fn fill_round_rect_gradient_h(
        &mut self,
        rect: Rect,
        radius: u32,
        left: Color,
        right: Color,
    ) {
        let paint = Paint::H {
            left,
            right,
            origin: rect.left(),
            len: rect.w as u64,
        };
        self.fill_round_rect_paint(rect, radius, paint);
    }

    fn fill_round_rect_paint(&mut self, rect: Rect, radius: u32, paint: Paint) {
        if rect.is_empty() || paint.is_invisible() {
            return;
        }
        let r = radius.min(rect.w / 2).min(rect.h / 2).min(MAX_RADIUS) as i64;
        let Some((x0, y0, x1, y1)) = self.clipped(&rect) else {
            return;
        };
        let (l, t, rr, b) = (rect.left(), rect.top(), rect.right(), rect.bottom());
        for y in y0..y1 {
            let yi = y as i64;
            let dyc = if r == 0 { 0 } else { corner_dist(yi, t, b, r) };
            if dyc == 0 {
                self.blend_span_paint(y, x0, x1, &paint);
                continue;
            }
            // 中央の直線部分。
            let ml = (x0 as i64).max(l + r);
            let mr = (x1 as i64).min(rr - r);
            if mr > ml {
                self.blend_span_paint(y, ml as usize, mr as usize, &paint);
            }
            // 左右の角。
            let left_end = (x1 as i64).min(l + r);
            for x in x0 as i64..left_end.max(x0 as i64) {
                self.blend_corner_px(x, y, l + r - x, dyc, r, &paint);
            }
            let right_start = (x0 as i64).max(rr - r);
            for x in right_start..x1 as i64 {
                self.blend_corner_px(x, y, x - (rr - r) + 1, dyc, r, &paint);
            }
        }
    }

    /// 1 行の `[x0, x1)` を `paint` で source-over 合成する。
    fn blend_span_paint(&mut self, y: usize, x0: usize, x1: usize, paint: &Paint) {
        match *paint {
            Paint::Solid(c) => self.blend_span(y, x0, x1, c),
            Paint::V { .. } => self.blend_span(y, x0, x1, paint.at(x0 as i64, y as i64)),
            Paint::H { .. } => {
                for x in x0..x1 {
                    let c = paint.at(x as i64, y as i64);
                    self.blend_span(y, x, x + 1, c);
                }
            }
        }
    }

    #[inline]
    fn blend_corner_px(&mut self, x: i64, y: usize, dxc: i64, dyc: i64, r: i64, paint: &Paint) {
        let cov = circle_cov(dxc, dyc, r);
        if cov > 0 {
            let color = paint.at(x, y as i64);
            let row = self.row_mut(y, x as usize, x as usize + 1);
            row[0] = blend_cov(row[0], color.0, cov);
        }
    }

    /// 角丸矩形の枠 (内側に `thickness` px、角はアンチエイリアス付き)。
    pub fn stroke_round_rect(&mut self, rect: Rect, radius: u32, thickness: u32, color: Color) {
        if rect.is_empty() || thickness == 0 || color.a() == 0 {
            return;
        }
        if thickness as u64 * 2 >= rect.w.min(rect.h) as u64 {
            self.fill_round_rect(rect, radius, color);
            return;
        }
        let r = radius.min(rect.w / 2).min(rect.h / 2).min(MAX_RADIUS) as i64;
        let t = thickness as i64;
        let r_in = (r - t).max(0);
        let Some((x0, y0, x1, y1)) = self.clipped(&rect) else {
            return;
        };
        let (l, top, rr, b) = (rect.left(), rect.top(), rect.right(), rect.bottom());
        let ring = |x: i64, y: i64| -> u32 {
            let oc = rr_cov(l, top, rr, b, r, x, y);
            if oc == 0 {
                return 0;
            }
            let ic = rr_cov(l + t, top + t, rr - t, b - t, r_in, x, y);
            div255(oc * (255 - ic))
        };
        for y in y0..y1 {
            let yi = y as i64;
            let hollow_row = yi >= top + t && yi < b - t;
            if hollow_row {
                // 左右の帯 (厚み + 内側の角) だけ評価し、中央の空洞は飛ばす。
                let skip_l = l + t + r_in;
                let skip_r = rr - t - r_in;
                let (xa, xb) = (x0 as i64, x1 as i64);
                let (e0, e1) = if skip_l < skip_r {
                    (skip_l.clamp(xa, xb), skip_r.clamp(xa, xb))
                } else {
                    (xb, xb)
                };
                for x in xa..e0 {
                    self.blend_ring_px(x, yi, color, &ring);
                }
                for x in e1..xb {
                    self.blend_ring_px(x, yi, color, &ring);
                }
            } else {
                // 上下の帯: 中央の直線部分は全面、左右の角だけ評価。
                let (xa, xb) = (x0 as i64, x1 as i64);
                let ml = xa.max(l + r);
                let mr = xb.min(rr - r);
                if mr > ml {
                    self.blend_span(y, ml as usize, mr as usize, color);
                }
                for x in xa..xb.min(l + r).max(xa) {
                    self.blend_ring_px(x, yi, color, &ring);
                }
                for x in xa.max(rr - r)..xb {
                    self.blend_ring_px(x, yi, color, &ring);
                }
            }
        }
    }

    #[inline]
    fn blend_ring_px(&mut self, x: i64, y: i64, color: Color, ring: &impl Fn(i64, i64) -> u32) {
        let cov = ring(x, y);
        if cov > 0 {
            let row = self.row_mut(y as usize, x as usize, x as usize + 1);
            row[0] = blend_cov(row[0], color.0, cov);
        }
    }

    // ---- 円・楕円 ----

    /// 矩形に内接する楕円を塗る (アンチエイリアス付き)。
    pub fn fill_ellipse(&mut self, rect: Rect, color: Color) {
        self.ellipse_impl(rect, 0, color);
    }

    /// 矩形に内接する楕円の枠 (内側に `thickness` px)。
    pub fn stroke_ellipse(&mut self, rect: Rect, thickness: u32, color: Color) {
        if thickness == 0 {
            return;
        }
        self.ellipse_impl(rect, thickness, color);
    }

    /// 中心 `(cx, cy)`、半径 `r` の円を塗る (直径は `2r`)。
    pub fn fill_circle(&mut self, cx: i32, cy: i32, r: u32, color: Color) {
        self.fill_ellipse(circle_rect(cx, cy, r), color);
    }

    /// 中心 `(cx, cy)`、半径 `r` の円の枠。
    pub fn stroke_circle(&mut self, cx: i32, cy: i32, r: u32, thickness: u32, color: Color) {
        self.stroke_ellipse(circle_rect(cx, cy, r), thickness, color);
    }

    /// `thickness == 0` なら塗り、それ以外なら枠。
    fn ellipse_impl(&mut self, rect: Rect, thickness: u32, color: Color) {
        if rect.is_empty() || color.a() == 0 {
            return;
        }
        let Some((x0, y0, x1, y1)) = self.clipped(&rect) else {
            return;
        };
        // 1/16 ピクセル単位。中心 = 矩形の中央、半径 = 幅/高さの半分。
        let cx16 = rect.x as i128 * 16 + rect.w as i128 * 8;
        let cy16 = rect.y as i128 * 16 + rect.h as i128 * 8;
        let rx16 = rect.w as i128 * 8;
        let ry16 = rect.h as i128 * 8;
        let t16 = thickness.min(MAX_THICKNESS) as i128 * 16;
        let inner = if thickness > 0 && rx16 > t16 && ry16 > t16 {
            Some((rx16 - t16, ry16 - t16))
        } else {
            None
        };
        // inner が None で thickness > 0 → 厚みが半径以上なので全面塗り。
        for y in y0..y1 {
            let mut outer = [None; 4];
            let mut hole = [None; 4];
            for k in 0..4 {
                let sy = y as i128 * 16 + 2 + 4 * k as i128;
                outer[k] = half_width16(sy - cy16, rx16, ry16).map(|h| (cx16 - h, cx16 + h));
                if let Some((irx, iry)) = inner {
                    hole[k] = half_width16(sy - cy16, irx, iry).map(|h| (cx16 - h, cx16 + h));
                }
            }
            if outer.iter().all(|o| o.is_none()) {
                continue;
            }
            let min_l = outer.iter().flatten().map(|s| s.0).min().unwrap_or(0);
            let max_r = outer.iter().flatten().map(|s| s.1).max().unwrap_or(0);
            let px_lo = (min_l.div_euclid(16)).max(x0 as i128);
            let px_hi = ((max_r + 15).div_euclid(16)).min(x1 as i128);
            if px_hi <= px_lo {
                continue;
            }
            // 4 サブ行すべてで全面カバーされる x 範囲 (ピクセル単位) と、全面が空洞の範囲。
            let all_outer = outer.iter().all(|o| o.is_some());
            let solid_o: Option<(i128, i128)> = if all_outer {
                let a = outer.iter().flatten().map(|s| s.0).max().unwrap_or(0);
                let b = outer.iter().flatten().map(|s| s.1).min().unwrap_or(0);
                (b > a).then(|| ((a + 15).div_euclid(16), b.div_euclid(16)))
            } else {
                None
            };
            let all_hole = inner.is_some() && hole.iter().all(|o| o.is_some());
            let hole_range: Option<(i128, i128)> = if all_hole {
                // 空洞が全サブ行で重なる範囲 (この範囲のピクセルは外側の塗りも内側に隠れる)。
                let a = hole.iter().flatten().map(|s| s.0).max().unwrap_or(0);
                let b = hole.iter().flatten().map(|s| s.1).min().unwrap_or(0);
                (b > a).then(|| ((a + 15).div_euclid(16), b.div_euclid(16)))
            } else {
                None
            };
            // 全面ソリッド範囲: 枠なしなら solid_o。枠ありなら、solid_o のうち空洞の外側にある
            // 左右の帯 [solid_o.0, 全サブ行で空洞が始まる位置) と (空洞が終わる位置, solid_o.1)。
            let mut px = px_lo;
            while px < px_hi {
                // 空洞の完全な内側 → 飛ばす。
                if let Some((ha, hb)) = hole_range
                    && px >= ha
                    && px < hb
                {
                    px = hb;
                    continue;
                }
                // 完全に塗られる範囲 → まとめて塗る。
                if inner.is_none()
                    && let Some((sa, sb)) = solid_o
                    && px >= sa
                    && px < sb
                {
                    let end = sb.min(px_hi);
                    self.blend_span(y, px as usize, end as usize, color);
                    px = end;
                    continue;
                }
                let cov = ellipse_px_cov(px, &outer, &hole);
                if cov > 0 {
                    let row = self.row_mut(y, px as usize, px as usize + 1);
                    row[0] = blend_cov(row[0], color.0, cov);
                }
                px += 1;
            }
        }
    }

    // ---- グラデーション ----

    /// 縦方向の線形グラデーション (上端が `top`、下端が `bottom`)。
    /// 色はクリップ前の矩形に対して決まる (クリップで色がずれない)。
    pub fn fill_gradient_v(&mut self, rect: Rect, top: Color, bottom: Color) {
        let Some((x0, y0, x1, y1)) = self.clipped(&rect) else {
            return;
        };
        let den = (rect.h as u64 - 1).max(1);
        let n = rect.h as u64;
        for y in y0..y1 {
            let j = (y as i64 - rect.y as i64) as u64;
            let c = lerp64(top, bottom, if n == 1 { 0 } else { j }, den);
            self.blend_span(y, x0, x1, c);
        }
    }

    /// 横方向の線形グラデーション (左端が `left`、右端が `right`)。
    pub fn fill_gradient_h(&mut self, rect: Rect, left: Color, right: Color) {
        let Some((x0, y0, x1, y1)) = self.clipped(&rect) else {
            return;
        };
        let n = rect.w as u64;
        let den = (n - 1).max(1);
        // 1 行ぶんの色を先に作り、全行で使い回す。
        let colors: Vec<u32> = (x0..x1)
            .map(|x| {
                let i = (x as i64 - rect.x as i64) as u64;
                lerp64(left, right, if n == 1 { 0 } else { i }, den).0
            })
            .collect();
        let opaque = colors.iter().all(|c| c >> 24 == 255);
        for y in y0..y1 {
            let row = self.row_mut(y, x0, x1);
            if opaque {
                row.copy_from_slice(&colors);
            } else {
                for (p, &c) in row.iter_mut().zip(&colors) {
                    *p = crate::color::blend_over(*p, c);
                }
            }
        }
    }
}

/// 塗りの色の決め方。
#[derive(Clone, Copy)]
enum Paint {
    Solid(Color),
    V {
        top: Color,
        bottom: Color,
        origin: i64,
        len: u64,
    },
    H {
        left: Color,
        right: Color,
        origin: i64,
        len: u64,
    },
}

impl Paint {
    fn is_invisible(&self) -> bool {
        match *self {
            Paint::Solid(c) => c.a() == 0,
            Paint::V { top, bottom, .. } => top.a() == 0 && bottom.a() == 0,
            Paint::H { left, right, .. } => left.a() == 0 && right.a() == 0,
        }
    }

    /// 画素 `(x, y)` の色。
    fn at(&self, x: i64, y: i64) -> Color {
        match *self {
            Paint::Solid(c) => c,
            Paint::V {
                top,
                bottom,
                origin,
                len,
            } => grad_color(top, bottom, (y - origin) as u64, len),
            Paint::H {
                left,
                right,
                origin,
                len,
            } => grad_color(left, right, (x - origin) as u64, len),
        }
    }
}

/// `len` 個の画素に渡るグラデーションの `i` 番目 (0 始まり) の色。`len == 1` は始点の色。
fn grad_color(a: Color, b: Color, i: u64, len: u64) -> Color {
    if len <= 1 {
        return a;
    }
    lerp64(a, b, i, len - 1)
}

fn circle_rect(cx: i32, cy: i32, r: u32) -> Rect {
    let (cx, cy, r) = (cx as i64, cy as i64, r as i64);
    Rect::from_ltrb(cx - r, cy - r, cx + r, cy + r)
}

/// `t / den` (u64 で計算) の位置の色。
fn lerp64(a: Color, b: Color, t: u64, den: u64) -> Color {
    let t = t.min(den);
    let ch = |x: u8, y: u8| -> u8 { ((x as u64 * (den - t) + y as u64 * t + den / 2) / den) as u8 };
    Color::argb(
        ch(a.a(), b.a()),
        ch(a.r(), b.r()),
        ch(a.g(), b.g()),
        ch(a.b(), b.b()),
    )
}

/// 角丸矩形の縦方向で、角の円中心からのピクセル数 (0 なら直線部分)。
#[inline]
fn corner_dist(y: i64, top: i64, bottom: i64, r: i64) -> i64 {
    if y < top + r {
        top + r - y
    } else if y >= bottom - r {
        y - (bottom - r) + 1
    } else {
        0
    }
}

/// 角の円 (半径 `r`) に対するピクセルのカバレッジ (0..=255)。
/// `dxc`/`dyc` は円中心から、そのピクセルの遠い側の辺までのピクセル数 (>= 1)。
#[inline]
fn circle_cov(dxc: i64, dyc: i64, r: i64) -> u32 {
    // ピクセル中心までの距離を 1/16 px で。
    let dx16 = (16 * dxc - 8) as u64;
    let dy16 = (16 * dyc - 8) as u64;
    let d2 = dx16 * dx16 + dy16 * dy16;
    let inner = (16 * r - 24).max(0) as u64; // これより内側は確実に全面
    if d2 <= inner * inner {
        return 255;
    }
    let dist16 = d2.isqrt() as i64;
    let c16 = (16 * r + 8 - dist16).clamp(0, 16);
    (c16 * 16).min(255) as u32
}

/// 角丸矩形 `[l, r) x [t, b)` (角半径 `rad`) の `(px, py)` でのカバレッジ。
#[inline]
fn rr_cov(l: i64, t: i64, r: i64, b: i64, rad: i64, px: i64, py: i64) -> u32 {
    if px < l || px >= r || py < t || py >= b {
        return 0;
    }
    if rad == 0 {
        return 255;
    }
    let dxc = if px < l + rad {
        l + rad - px
    } else if px >= r - rad {
        px - (r - rad) + 1
    } else {
        0
    };
    let dyc = corner_dist(py, t, b, rad);
    if dxc == 0 || dyc == 0 {
        return 255;
    }
    circle_cov(dxc, dyc, rad)
}

/// 楕円 (半径 `rx16`, `ry16`) の中心から `dy` (1/16 px) の高さでの半幅 (1/16 px)。
fn half_width16(dy: i128, rx16: i128, ry16: i128) -> Option<i128> {
    if ry16 <= 0 || rx16 <= 0 || dy.abs() >= ry16 {
        return None;
    }
    let rem = (ry16 * ry16 - dy * dy) as u128;
    Some(rx16 * (rem.isqrt() as i128) / ry16)
}

/// ピクセル `px` の、4 サブ行 x (外側 - 空洞) のカバレッジ (0..=255)。
fn ellipse_px_cov(
    px: i128,
    outer: &[Option<(i128, i128)>; 4],
    hole: &[Option<(i128, i128)>; 4],
) -> u32 {
    let (a, b) = (px * 16, px * 16 + 16);
    let overlap = |s: (i128, i128)| -> i128 { (s.1.min(b) - s.0.max(a)).max(0) };
    let mut total = 0i128; // 最大 4 * 16
    for k in 0..4 {
        if let Some(o) = outer[k] {
            let mut v = overlap(o);
            if let Some(h) = hole[k] {
                v -= overlap(h);
            }
            total += v.max(0);
        }
    }
    ((total * 255 + 32) / 64).min(255) as u32
}

impl Rect {
    /// `i64` 座標の点を含むか (クリップ判定用)。
    #[inline]
    pub(crate) fn contains_point_i64(&self, x: i64, y: i64) -> bool {
        !self.is_empty()
            && x >= self.left()
            && x < self.right()
            && y >= self.top()
            && y < self.bottom()
    }
}

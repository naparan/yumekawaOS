//! 矩形と、ダメージ (dirty rect) 管理。
//!
//! 座標は符号付き 32 ビット、幅・高さは符号なし 32 ビット。端点 (`x + w` など) の計算は
//! すべて `i64` で行うので、巨大な値でもオーバーフローやパニックは起きない。

use alloc::vec::Vec;

/// 軸に平行な矩形。`(x, y)` が左上、`w`/`h` が幅/高さ。右端・下端は含まない (半開区間)。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Hash)]
pub struct Rect {
    /// 左端
    pub x: i32,
    /// 上端
    pub y: i32,
    /// 幅
    pub w: u32,
    /// 高さ
    pub h: u32,
}

impl Rect {
    /// 空の矩形 (0x0, 原点)。
    pub const EMPTY: Rect = Rect::new(0, 0, 0, 0);

    /// 矩形を作る。
    #[inline]
    pub const fn new(x: i32, y: i32, w: u32, h: u32) -> Rect {
        Rect { x, y, w, h }
    }

    /// 原点に置いた `w` x `h` の矩形。
    #[inline]
    pub const fn from_size(w: u32, h: u32) -> Rect {
        Rect::new(0, 0, w, h)
    }

    /// 左・上・右・下 (右下は含まない) から作る。各値は `i32` の範囲に飽和させる
    /// (したがって幅・高さは最大 `u32::MAX`)。右 <= 左 または下 <= 上なら空。
    pub const fn from_ltrb(l: i64, t: i64, r: i64, b: i64) -> Rect {
        let (l, t, r, b) = (sat_i32(l), sat_i32(t), sat_i32(r), sat_i32(b));
        if r <= l || b <= t {
            return Rect::new(l, t, 0, 0);
        }
        Rect::new(
            l,
            t,
            (r as i64 - l as i64) as u32,
            (b as i64 - t as i64) as u32,
        )
    }

    /// 幅または高さが 0 なら空。
    #[inline]
    pub const fn is_empty(&self) -> bool {
        self.w == 0 || self.h == 0
    }

    /// 左端 (`x`)。
    #[inline]
    pub const fn left(&self) -> i64 {
        self.x as i64
    }
    /// 上端 (`y`)。
    #[inline]
    pub const fn top(&self) -> i64 {
        self.y as i64
    }
    /// 右端 (含まない)。
    #[inline]
    pub const fn right(&self) -> i64 {
        self.x as i64 + self.w as i64
    }
    /// 下端 (含まない)。
    #[inline]
    pub const fn bottom(&self) -> i64 {
        self.y as i64 + self.h as i64
    }

    /// 面積 (`u64`、オーバーフローしない)。
    #[inline]
    pub const fn area(&self) -> u64 {
        self.w as u64 * self.h as u64
    }

    /// 点 `(px, py)` を含むか。空の矩形は何も含まない。
    #[inline]
    pub const fn contains_point(&self, px: i32, py: i32) -> bool {
        !self.is_empty()
            && px as i64 >= self.left()
            && (px as i64) < self.right()
            && py as i64 >= self.top()
            && (py as i64) < self.bottom()
    }

    /// `other` を完全に含むか。`other` が空なら常に true。
    pub const fn contains(&self, other: &Rect) -> bool {
        if other.is_empty() {
            return true;
        }
        !self.is_empty()
            && other.left() >= self.left()
            && other.top() >= self.top()
            && other.right() <= self.right()
            && other.bottom() <= self.bottom()
    }

    /// 重なる部分があるか (辺が接するだけなら false)。
    pub const fn intersects(&self, other: &Rect) -> bool {
        !self.is_empty()
            && !other.is_empty()
            && self.left() < other.right()
            && other.left() < self.right()
            && self.top() < other.bottom()
            && other.top() < self.bottom()
    }

    /// 共通部分。重ならなければ空の矩形 (`is_empty()`)。
    pub const fn intersect(&self, other: &Rect) -> Rect {
        let l = max_i64(self.left(), other.left());
        let t = max_i64(self.top(), other.top());
        let r = min_i64(self.right(), other.right());
        let b = min_i64(self.bottom(), other.bottom());
        if self.is_empty() || other.is_empty() || r <= l || b <= t {
            return Rect::EMPTY;
        }
        Rect::from_ltrb(l, t, r, b)
    }

    /// `bounds` の内側に切り詰めた矩形 (= `intersect`)。
    #[inline]
    pub const fn clamp_to(&self, bounds: &Rect) -> Rect {
        self.intersect(bounds)
    }

    /// 両方を含む最小の矩形 (外接矩形)。空の矩形は無視する。
    /// 座標は `i32` の範囲に飽和する。
    pub const fn union(&self, other: &Rect) -> Rect {
        if self.is_empty() {
            return *other;
        }
        if other.is_empty() {
            return *self;
        }
        Rect::from_ltrb(
            min_i64(self.left(), other.left()),
            min_i64(self.top(), other.top()),
            max_i64(self.right(), other.right()),
            max_i64(self.bottom(), other.bottom()),
        )
    }

    /// `(dx, dy)` だけ平行移動 (`i32` の範囲に飽和)。
    pub const fn translate(&self, dx: i32, dy: i32) -> Rect {
        Rect::new(
            sat_i32(self.x as i64 + dx as i64),
            sat_i32(self.y as i64 + dy as i64),
            self.w,
            self.h,
        )
    }

    /// 四辺を `d` だけ外側に広げる (`d < 0` なら縮める。縮めすぎると空)。
    pub const fn inflate(&self, d: i32) -> Rect {
        let d = d as i64;
        Rect::from_ltrb(
            self.left() - d,
            self.top() - d,
            self.right() + d,
            self.bottom() + d,
        )
    }
}

const fn sat_i32(v: i64) -> i32 {
    if v > i32::MAX as i64 {
        i32::MAX
    } else if v < i32::MIN as i64 {
        i32::MIN
    } else {
        v as i32
    }
}

const fn max_i64(a: i64, b: i64) -> i64 {
    if a > b { a } else { b }
}
const fn min_i64(a: i64, b: i64) -> i64 {
    if a < b { a } else { b }
}

/// 複数の dirty 矩形を蓄積する小さなリスト。コンポジタが「再描画が必要な領域」を集めるのに使う。
///
/// - 空の矩形は無視する。
/// - 既存の矩形に完全に含まれる矩形は追加しない。新しい矩形に含まれる既存の矩形は取り除く。
/// - 重なる/接する矩形は、外接矩形にまとめても無駄な面積が 25% 以内ならまとめる
///   (描画回数が減る。そうでなければ別々のまま保持する)。
/// - 矩形数が `max_rects` を超えたら、全体を 1 つの外接矩形にまとめる (上限を超えて増えない)。
#[derive(Clone, Debug)]
pub struct DamageList {
    rects: Vec<Rect>,
    max_rects: usize,
}

impl DamageList {
    /// 既定の上限 (16 個) で作る。
    pub fn new() -> DamageList {
        DamageList::with_max_rects(16)
    }

    /// 矩形数の上限を指定して作る (0 は 1 に丸める)。
    pub fn with_max_rects(max_rects: usize) -> DamageList {
        DamageList {
            rects: Vec::new(),
            max_rects: max_rects.max(1),
        }
    }

    /// dirty 矩形を追加する。
    pub fn add(&mut self, rect: Rect) {
        if rect.is_empty() {
            return;
        }
        let mut new = rect;
        // 既存に完全に含まれるなら何もしない。
        if self.rects.iter().any(|r| r.contains(&new)) {
            return;
        }
        // 新しい矩形と重なる/接する既存矩形は取り込んで、繰り返し (連鎖的にまとまる場合がある)。
        loop {
            let before = self.rects.len();
            let mut merged = new;
            self.rects.retain(|r| {
                if touches(r, &merged) {
                    merged = merged.union(r);
                    false
                } else {
                    true
                }
            });
            new = merged;
            if self.rects.len() == before {
                break;
            }
        }
        self.rects.push(new);
        if self.rects.len() > self.max_rects {
            let all = self.bounds();
            self.rects.clear();
            self.rects.push(all);
        }
    }

    /// 全体の外接矩形 (空なら `Rect::EMPTY`)。
    pub fn bounds(&self) -> Rect {
        self.rects.iter().fold(Rect::EMPTY, |acc, r| acc.union(r))
    }

    /// 蓄積された矩形。
    pub fn rects(&self) -> &[Rect] {
        &self.rects
    }

    /// 矩形が無いか。
    pub fn is_empty(&self) -> bool {
        self.rects.is_empty()
    }

    /// 蓄積をすべて取り出して空にする。
    pub fn take(&mut self) -> Vec<Rect> {
        core::mem::take(&mut self.rects)
    }

    /// 蓄積を捨てる。
    pub fn clear(&mut self) {
        self.rects.clear();
    }

    /// すべての矩形を `bounds` に切り詰める (空になったものは捨てる)。
    pub fn clip_to(&mut self, bounds: &Rect) {
        let old = core::mem::take(&mut self.rects);
        for r in old {
            self.add(r.intersect(bounds));
        }
    }
}

impl Default for DamageList {
    fn default() -> Self {
        DamageList::new()
    }
}

/// 重なる/接していて、外接矩形にまとめても無駄な面積が 25% 以内なら true。
/// (L 字に交差する細長い 2 矩形などを巨大な外接矩形にしてしまうのを避ける。)
fn touches(a: &Rect, b: &Rect) -> bool {
    if a.is_empty() || b.is_empty() {
        return false;
    }
    let adjacent = a.left() <= b.right()
        && b.left() <= a.right()
        && a.top() <= b.bottom()
        && b.top() <= a.bottom();
    if !adjacent {
        return false;
    }
    let u = a.union(b).area() as u128;
    let sum = a.area() as u128 + b.area() as u128;
    u * 4 <= sum * 5
}

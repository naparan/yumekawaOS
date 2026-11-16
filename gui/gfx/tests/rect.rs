//! Rect / DamageList のテスト。

use yk_gfx::{DamageList, Rect};

#[test]
fn basic_edges_and_empty() {
    let r = Rect::new(10, 20, 30, 40);
    assert_eq!((r.left(), r.top(), r.right(), r.bottom()), (10, 20, 40, 60));
    assert_eq!(r.area(), 1200);
    assert!(!r.is_empty());
    assert!(Rect::new(0, 0, 0, 5).is_empty());
    assert!(Rect::new(0, 0, 5, 0).is_empty());
    assert!(Rect::EMPTY.is_empty());
}

#[test]
fn contains_point_is_half_open() {
    let r = Rect::new(1, 1, 3, 2);
    assert!(r.contains_point(1, 1));
    assert!(r.contains_point(3, 2));
    assert!(!r.contains_point(4, 2));
    assert!(!r.contains_point(3, 3));
    assert!(!r.contains_point(0, 1));
    assert!(!Rect::EMPTY.contains_point(0, 0));
}

#[test]
fn intersect_union_contains() {
    let a = Rect::new(0, 0, 10, 10);
    let b = Rect::new(5, 5, 10, 10);
    assert_eq!(a.intersect(&b), Rect::new(5, 5, 5, 5));
    assert!(a.intersects(&b));
    assert_eq!(a.union(&b), Rect::new(0, 0, 15, 15));
    // 辺が接するだけなら交差しない。
    let c = Rect::new(10, 0, 5, 5);
    assert!(!a.intersects(&c));
    assert!(a.intersect(&c).is_empty());
    // 包含
    assert!(a.contains(&Rect::new(2, 2, 3, 3)));
    assert!(a.contains(&a));
    assert!(!a.contains(&b));
    assert!(a.contains(&Rect::EMPTY));
    // 空は union で無視される
    assert_eq!(a.union(&Rect::EMPTY), a);
    assert_eq!(Rect::new(7, 7, 0, 3).union(&a), a);
}

#[test]
fn clamp_to_bounds() {
    let screen = Rect::from_size(1280, 800);
    assert_eq!(
        Rect::new(-10, -20, 100, 100).clamp_to(&screen),
        Rect::new(0, 0, 90, 80)
    );
    assert_eq!(
        Rect::new(1270, 790, 100, 100).clamp_to(&screen),
        Rect::new(1270, 790, 10, 10)
    );
    assert!(Rect::new(2000, 0, 5, 5).clamp_to(&screen).is_empty());
}

#[test]
fn extreme_values_do_not_overflow() {
    let big = Rect::new(i32::MIN, i32::MIN, u32::MAX, u32::MAX);
    assert_eq!(big.right(), i32::MIN as i64 + u32::MAX as i64);
    let far = Rect::new(i32::MAX, i32::MAX, u32::MAX, u32::MAX);
    assert!(far.right() > i32::MAX as i64);
    // 交差・合併・拡大・平行移動は panic しない。
    let i = big.intersect(&far);
    assert!(i.is_empty());
    let _ = big.union(&far);
    let _ = big.inflate(i32::MAX);
    let _ = big.inflate(i32::MIN);
    let t = far.translate(i32::MAX, i32::MAX);
    assert_eq!((t.x, t.y), (i32::MAX, i32::MAX));
    let t = big.translate(i32::MIN, i32::MIN);
    assert_eq!((t.x, t.y), (i32::MIN, i32::MIN));
    assert!(big.contains_point(0, 0));
    assert_eq!(big.area(), u32::MAX as u64 * u32::MAX as u64);
    // 画面との交差は正しく原点付近を返す。
    let screen = Rect::from_size(100, 100);
    assert_eq!(big.intersect(&screen), screen);
}

#[test]
fn inflate_and_translate() {
    let r = Rect::new(10, 10, 20, 20);
    assert_eq!(r.inflate(5), Rect::new(5, 5, 30, 30));
    assert_eq!(r.inflate(-5), Rect::new(15, 15, 10, 10));
    assert!(r.inflate(-10).is_empty());
    assert!(r.inflate(-100).is_empty());
    assert_eq!(r.translate(-10, 5), Rect::new(0, 15, 20, 20));
}

#[test]
fn damage_ignores_empty_and_contained() {
    let mut d = DamageList::new();
    d.add(Rect::EMPTY);
    d.add(Rect::new(5, 5, 0, 10));
    assert!(d.is_empty());
    d.add(Rect::new(0, 0, 100, 100));
    d.add(Rect::new(10, 10, 5, 5)); // 含まれる
    assert_eq!(d.rects(), &[Rect::new(0, 0, 100, 100)]);
    // 新しい矩形が既存を含むと、既存が置き換わる。
    d.add(Rect::new(-10, -10, 200, 200));
    assert_eq!(d.rects(), &[Rect::new(-10, -10, 200, 200)]);
}

#[test]
fn damage_merges_overlap_but_not_wasteful_unions() {
    let mut d = DamageList::new();
    d.add(Rect::new(0, 0, 10, 10));
    d.add(Rect::new(5, 0, 10, 10)); // 重なる → 合併しても無駄なし
    assert_eq!(d.rects(), &[Rect::new(0, 0, 15, 10)]);
    // 離れた矩形は別々のまま。
    d.add(Rect::new(100, 100, 10, 10));
    assert_eq!(d.rects().len(), 2);
    // L 字に交差する細長い 2 枚は、巨大な外接矩形にしない。
    let mut d = DamageList::new();
    d.add(Rect::new(0, 0, 10, 1000));
    d.add(Rect::new(0, 0, 1000, 10));
    assert_eq!(d.rects().len(), 2);
}

#[test]
fn damage_caps_rect_count() {
    let mut d = DamageList::with_max_rects(4);
    for i in 0..10 {
        d.add(Rect::new(i * 100, 0, 10, 10));
    }
    assert!(d.rects().len() <= 4);
    // どの入力も、結果のどれかに含まれている (取りこぼしなし)。
    for i in 0..10 {
        let r = Rect::new(i * 100, 0, 10, 10);
        assert!(d.rects().iter().any(|x| x.contains(&r)), "lost {r:?}");
    }
    let taken = d.take();
    assert!(!taken.is_empty());
    assert!(d.is_empty());
}

#[test]
fn damage_clip_to_and_bounds() {
    let mut d = DamageList::new();
    d.add(Rect::new(-50, -50, 100, 100));
    d.add(Rect::new(900, 700, 500, 500));
    d.add(Rect::new(5000, 5000, 10, 10)); // 画面外
    d.clip_to(&Rect::from_size(1000, 800));
    assert_eq!(d.rects().len(), 2);
    assert_eq!(d.bounds(), Rect::new(0, 0, 1000, 800));
    d.clear();
    assert_eq!(d.bounds(), Rect::EMPTY);
}

#[test]
fn damage_random_coverage_property() {
    // 乱数で追加した全矩形の全画素が、最終結果のどれかに含まれる。
    let mut s = 0x1234_5678u32;
    let mut next = || {
        s ^= s << 13;
        s ^= s >> 17;
        s ^= s << 5;
        s
    };
    for max in [1usize, 3, 16] {
        let mut d = DamageList::with_max_rects(max);
        let mut added = Vec::new();
        for _ in 0..200 {
            let r = Rect::new(
                (next() % 200) as i32 - 50,
                (next() % 200) as i32 - 50,
                next() % 60,
                next() % 60,
            );
            d.add(r);
            added.push(r);
            assert!(d.rects().len() <= max);
        }
        for r in added {
            if r.is_empty() {
                continue;
            }
            // 矩形の 4 隅 (含まれる点) が、いずれかの結果に入っていること。
            for (x, y) in [(r.x, r.y), (r.right() as i32 - 1, r.bottom() as i32 - 1)] {
                assert!(
                    d.rects().iter().any(|q| q.contains_point(x, y)),
                    "lost point in {r:?}"
                );
            }
        }
    }
}

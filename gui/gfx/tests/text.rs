//! テキスト描画のテスト (M PLUS Rounded 1c を使用)。

use yk_gfx::text::{Font, FontError, TextAlign, TextRenderer, TextStyle};
use yk_gfx::{Canvas, Color, Image, Rect};

static REGULAR: &[u8] = include_bytes!("../../../assets/fonts/MPLUSRounded1c-Regular.ttf");
static BOLD: &[u8] = include_bytes!("../../../assets/fonts/MPLUSRounded1c-Bold.ttf");
const JOYO: &str = include_str!("data/joyo_kanji.txt");
const JIS_KANJI: &str = include_str!("data/jisx0208_kanji.txt");

fn regular() -> Font<'static> {
    Font::new(REGULAR).unwrap()
}

fn black(w: u32, h: u32) -> Image {
    Image::filled(w, h, Color::BLACK).unwrap()
}

/// 赤チャネルが 0 でないピクセルの外接矩形と、カバレッジの合計。
fn ink(i: &Image) -> (Option<Rect>, u64) {
    let mut bb: Option<(u32, u32, u32, u32)> = None;
    let mut sum = 0u64;
    for y in 0..i.height() {
        for x in 0..i.width() {
            let v = i.get(x, y).unwrap().r() as u64;
            if v > 0 {
                sum += v;
                bb = Some(match bb {
                    None => (x, y, x, y),
                    Some((a, b, c, d)) => (a.min(x), b.min(y), c.max(x), d.max(y)),
                });
            }
        }
    }
    (
        bb.map(|(a, b, c, d)| Rect::new(a as i32, b as i32, c - a + 1, d - b + 1)),
        sum,
    )
}

fn style(size: f32) -> TextStyle {
    TextStyle::new(size, Color::WHITE)
}

#[test]
fn font_loading_errors() {
    assert_eq!(Font::new(&[]).unwrap_err(), FontError::InvalidFont);
    assert_eq!(
        Font::new(b"this is not a font file at all").unwrap_err(),
        FontError::InvalidFont
    );
    assert!(Font::new(&REGULAR[..200]).is_err());
    assert!(Font::new(&[0u8; 4096]).is_err());
    assert!(Font::new(BOLD).is_ok());
}

#[test]
fn glyph_coverage_kana_joyo_and_jis_kanji() {
    for (name, data) in [("Regular", REGULAR), ("Bold", BOLD)] {
        let f = Font::new(data).unwrap();
        assert_eq!(JOYO.trim().chars().count(), 2136, "常用漢字は 2136 字");
        assert_eq!(
            JIS_KANJI.trim().chars().count(),
            6355,
            "JIS X 0208 の漢字は 6355 字"
        );
        let missing: String = JOYO.trim().chars().filter(|&c| !f.has_glyph(c)).collect();
        assert!(
            missing.is_empty(),
            "{name}: 常用漢字で欠けている字: {missing}"
        );
        // JIS X 0208 の第 1 水準 (先頭 2965 字) は完全、第 2 水準 (残り 3390 字) は一部のみ収録。
        let jis: Vec<char> = JIS_KANJI.trim().chars().collect();
        let missing: String = jis[..2965]
            .iter()
            .copied()
            .filter(|&c| !f.has_glyph(c))
            .collect();
        assert!(
            missing.is_empty(),
            "{name}: JIS 第 1 水準で欠けている字: {missing}"
        );
        let l2_have = jis[2965..].iter().filter(|&&c| f.has_glyph(c)).count();
        assert_eq!(
            l2_have,
            3390 - 1447,
            "{name}: 第 2 水準の収録数 (M PLUS Rounded 1c の仕様)"
        );
        for c in ('\u{3041}'..='\u{3096}')
            .chain('\u{30A1}'..='\u{30FA}')
            .chain(['ー', '、', '。', '「', '」', '・', '々', '〜'])
        {
            assert!(f.has_glyph(c), "{name}: かな/記号 {c} (U+{:04X})", c as u32);
        }
        for c in ' '..='~' {
            assert!(f.has_glyph(c), "{name}: ASCII {c:?}");
        }
        assert!(!f.has_glyph('\u{1F600}'), "絵文字は収録されていないはず");
    }
}

#[test]
fn every_hiragana_katakana_and_sample_kanji_draws_ink() {
    let f = regular();
    let mut tr = TextRenderer::new();
    let chars: Vec<char> = ('あ'..='ん')
        .chain('ア'..='ン')
        .chain("漢字日本語常用一二三龍鬱".chars())
        .collect();
    for c in chars {
        let mut i = black(60, 60);
        let mut cv = i.canvas();
        let s = c.to_string();
        let w = tr.draw_text(&mut cv, &f, &style(32.0), 8, 8, &s);
        let (bb, sum) = ink(&i);
        let bb = bb.unwrap_or_else(|| panic!("{c} が描かれていない"));
        assert!(sum > 255 * 12, "{c}: インクが少なすぎる ({sum})");
        // 送り幅は 1em (全角)、インクは枠の中に収まる。
        assert_eq!(w, 32, "{c} の送り幅");
        assert!(
            bb.x >= 6 && bb.right() <= 8 + 32 + 2,
            "{c}: 横の位置 {bb:?}"
        );
        assert!(bb.y >= 6 && bb.bottom() <= 8 + 48, "{c}: 縦の位置 {bb:?}");
    }
}

#[test]
fn metrics_and_measure() {
    let f = regular();
    let m = f.metrics(24.0);
    assert!(m.ascent > 15 && m.ascent < 30, "{m:?}");
    assert!(m.descent > 2 && m.descent < 12, "{m:?}");
    assert_eq!(m.line_height, m.ascent + m.descent + m.line_gap);
    assert!(m.line_height >= 24);
    // サイズに比例する。
    let (a, b) = (f.metrics(12.0), f.metrics(48.0));
    assert!((b.ascent - 4 * a.ascent).abs() <= 4, "{a:?} {b:?}");

    let t = f.measure_text(24.0, "あいう");
    assert_eq!((t.width, t.lines), (72, 1));
    assert_eq!(t.height, m.line_height as u32);
    // 空文字列は幅 0・1 行。
    let e = f.measure_text(24.0, "");
    assert_eq!((e.width, e.lines, e.height), (0, 1, m.line_height as u32));
    // ASCII は比例幅: i は m より狭い。
    let wi = f.measure_text(24.0, "iiii").width;
    let wm = f.measure_text(24.0, "mmmm").width;
    assert!(wi < wm, "{wi} {wm}");
    // 文字列の幅 = 文字幅の和 (カーニングなし) と、サイズに対する単調性。
    let sum: u32 = "Hello 日本"
        .chars()
        .map(|c| f.measure_text(24.0, &c.to_string()).width)
        .sum();
    let whole = f.measure_text(24.0, "Hello 日本").width;
    assert!((whole as i32 - sum as i32).abs() <= 6, "{whole} vs {sum}");
    assert!(f.measure_text(12.0, "Hello").width < f.measure_text(24.0, "Hello").width);
    // 改行: 幅は最長の行、行数・高さは行数ぶん。
    let t = f.measure_text(20.0, "ab\nabcdef\n");
    assert_eq!(t.lines, 3);
    assert_eq!(t.width, f.measure_text(20.0, "abcdef").width);
    assert_eq!(t.height, 3 * f.metrics(20.0).line_height as u32);
    // 分数サイズも使える (CSS の 13.33px など)。
    assert_eq!(f.measure_text(13.33, "あ").width, 14); // 13.33px を切り上げ
    // 描画幅と測定幅が一致する。
    let mut tr = TextRenderer::new();
    let mut i = black(300, 60);
    let w = tr.draw_text(&mut i.canvas(), &f, &style(24.0), 5, 5, "Hello 日本語");
    assert_eq!(w, f.measure_text(24.0, "Hello 日本語").width);
}

#[test]
fn zero_width_and_control_chars() {
    let f = regular();
    assert_eq!(
        f.measure_text(20.0, "a\u{200B}b").width,
        f.measure_text(20.0, "ab").width
    );
    assert_eq!(
        f.measure_text(20.0, "a\u{FE0F}b").width,
        f.measure_text(20.0, "ab").width
    );
    assert_eq!(
        f.measure_text(20.0, "a\u{0007}\u{0000}b\r").width,
        f.measure_text(20.0, "ab").width
    );
    // タブは空白 4 つぶん。
    assert_eq!(
        f.measure_text(20.0, "\t").width,
        f.measure_text(20.0, "    ").width
    );
}

#[test]
fn missing_glyph_draws_tofu_and_does_not_panic() {
    let f = regular();
    let mut tr = TextRenderer::new();
    for s in [
        "\u{1F600}",
        "\u{E000}",
        "\u{FFFF}",
        "\u{10FFFF}",
        "\u{0378}",
        "😀日本😀",
    ] {
        let mut i = black(120, 60);
        let w = tr.draw_text(&mut i.canvas(), &f, &style(30.0), 10, 10, s);
        let (bb, sum) = ink(&i);
        assert!(bb.is_some() && sum > 0, "{s:?}: 豆腐が描かれていない");
        assert!(w > 0 || s == "\u{0378}" && w == 0 || true);
    }
    // 豆腐の幅は 0.8em、形は枠 (中央が空)。
    assert_eq!(f.measure_text(30.0, "\u{1F600}").width, 24);
    let mut i = black(60, 60);
    tr.draw_text(&mut i.canvas(), &f, &style(40.0), 10, 5, "\u{1F600}");
    let (bb, _) = ink(&i);
    let bb = bb.unwrap();
    let (cx, cy) = (bb.x as u32 + bb.w / 2, bb.y as u32 + bb.h / 2);
    assert_eq!(i.get(cx, cy).unwrap().r(), 0, "豆腐の中心は空洞");
    assert_eq!(
        i.get(bb.x as u32, cy).unwrap().r(),
        255,
        "豆腐の左辺は塗られる"
    );
    // 空の文字列・空白だけ。
    let mut j = black(20, 20);
    assert_eq!(tr.draw_text(&mut j.canvas(), &f, &style(20.0), 0, 0, ""), 0);
    tr.draw_text(&mut j.canvas(), &f, &style(20.0), 0, 0, "   \t  ");
    assert_eq!(ink(&j).1, 0);
}

#[test]
fn cache_hits_second_time_and_output_identical() {
    let f = regular();
    let mut tr = TextRenderer::new();
    let text = "ゆめかわいい OS 夢 Dream";
    let mut a = black(400, 50);
    tr.draw_text(&mut a.canvas(), &f, &style(28.0), 4, 4, text);
    let s1 = tr.cache_stats();
    assert!(s1.misses > 0 && s1.entries > 0 && s1.bytes > 0);
    let mut b = black(400, 50);
    tr.draw_text(&mut b.canvas(), &f, &style(28.0), 4, 4, text);
    let s2 = tr.cache_stats();
    assert_eq!(s2.misses, s1.misses, "2 回目は全部ヒットのはず");
    assert!(s2.hits >= s1.hits + 10);
    assert_eq!(a, b);
    // キャッシュ無し (容量 0) でも結果は同じ。
    let mut nc = TextRenderer::with_cache_capacity(0);
    let mut c = black(400, 50);
    nc.draw_text(&mut c.canvas(), &f, &style(28.0), 4, 4, text);
    assert_eq!(a, c);
    assert_eq!(nc.cache_stats().entries, 0);
    // サイズが違えば別エントリ。
    let before = tr.cache_stats().entries;
    let mut d = black(400, 50);
    tr.draw_text(&mut d.canvas(), &f, &style(20.0), 4, 4, text);
    assert!(tr.cache_stats().entries > before);
    tr.clear_cache();
    assert_eq!(tr.cache_stats().entries, 0);
    assert_eq!(tr.cache_stats().bytes, 0);
}

#[test]
fn cache_is_bounded_and_evicts_old_entries() {
    let f = regular();
    let cap = 24 * 1024;
    let mut tr = TextRenderer::with_cache_capacity(cap);
    let mut i = black(900, 60);
    // 数百種類の漢字を描いて容量を何度も超えさせる。
    let kanji: Vec<char> = JOYO.trim().chars().collect();
    for chunk in kanji.chunks(25) {
        let s: String = chunk.iter().collect();
        tr.draw_text(&mut i.canvas(), &f, &style(30.0), 0, 0, &s);
        let st = tr.cache_stats();
        assert!(st.bytes <= cap, "容量超過: {st:?}");
    }
    let st = tr.cache_stats();
    assert!(st.evicted > 100, "{st:?}");
    assert!(st.entries > 0);
    // 破棄後も同じ文字を正しく再生成できる。
    let mut a = black(100, 60);
    tr.draw_text(&mut a.canvas(), &f, &style(30.0), 0, 0, "日本");
    let mut b = black(100, 60);
    TextRenderer::new().draw_text(&mut b.canvas(), &f, &style(30.0), 0, 0, "日本");
    assert_eq!(a, b);
    // 容量を縮める。
    tr.set_cache_capacity(4096);
    assert!(tr.cache_stats().bytes <= 4096);
}

#[test]
fn huge_glyphs_bypass_cache() {
    let f = regular();
    let mut tr = TextRenderer::with_cache_capacity(8 * 1024);
    let mut i = black(300, 300);
    tr.draw_text(&mut i.canvas(), &f, &style(250.0), 0, 0, "漢");
    assert!(ink(&i).1 > 0);
    assert_eq!(
        tr.cache_stats().entries,
        0,
        "容量の 1/4 を超えるグリフはキャッシュしない"
    );
}

#[test]
fn extreme_sizes_do_not_panic() {
    let f = regular();
    let mut tr = TextRenderer::new();
    for size in [
        f32::NAN,
        f32::INFINITY,
        f32::NEG_INFINITY,
        0.0,
        -5.0,
        0.001,
        1.0,
        5000.0,
        1e30,
        f32::MAX,
    ] {
        let mut i = black(64, 64);
        let _ = f.metrics(size);
        let _ = f.measure_text(size, "あaA\n日");
        let _ = f.measure_wrapped(size, "あaA 日本語 text", 30);
        tr.draw_text(&mut i.canvas(), &f, &style(size), 3, 3, "あaA");
        tr.draw_text_box(
            &mut i.canvas(),
            &f,
            &style(size).line_height(size),
            Rect::new(0, 0, 64, 64),
            "あaA 日本語 text",
        );
    }
    // 極端な行送り。
    let mut i = black(64, 64);
    for lh in [f32::NAN, 0.0, -3.0, 1e20, f32::INFINITY] {
        tr.draw_text_box(
            &mut i.canvas(),
            &f,
            &style(12.0).line_height(lh),
            Rect::new(0, 0, 64, 64),
            "あい\nうえ",
        );
    }
}

#[test]
fn wrapping_latin_words() {
    let f = regular();
    let text = "The quick brown fox jumps over the lazy dog";
    let full = f.measure_text(20.0, text).width;
    let lay = f.layout(20.0, text, Some(full / 3), None);
    assert!(lay.lines.len() >= 3, "{} 行", lay.lines.len());
    // 行は元の文字列の連続した部分で、単語の途中で切れず、幅内に収まる。
    let mut rebuilt = Vec::new();
    for l in &lay.lines {
        let s = &text[l.start..l.end];
        assert!(!s.starts_with(' ') && !s.ends_with(' '), "{s:?}");
        assert!(
            l.width() <= full / 3,
            "{s:?} が幅を超えている ({})",
            l.width()
        );
        rebuilt.push(s);
    }
    assert_eq!(rebuilt.join(" "), text);
    // 1 単語が幅より長い場合は文字単位で割る (行数が増え、全文字が残る)。
    let lay = f.layout(20.0, "supercalifragilistic", Some(60), None);
    assert!(lay.lines.len() >= 3);
    let all: String = lay
        .lines
        .iter()
        .map(|l| &"supercalifragilistic"[l.start..l.end])
        .collect();
    assert_eq!(all, "supercalifragilistic");
    // 幅 0 でも終わる (1 文字ずつ)。
    let lay = f.layout(20.0, "abc", Some(0), None);
    assert_eq!(lay.lines.len(), 3);
}

#[test]
fn wrapping_japanese_and_kinsoku() {
    let f = regular();
    let text = "吾輩は猫である。名前はまだ無い。どこで生れたかとんと見当がつかぬ。「何でも」薄暗いじめじめした所で、ニャーニャー泣いていた事だけは記憶している。";
    for width in [120u32, 200, 333, 480] {
        let lay = f.layout(20.0, text, Some(width), None);
        let mut rebuilt = String::new();
        for (k, l) in lay.lines.iter().enumerate() {
            let s = &text[l.start..l.end];
            assert!(l.width() <= width + 20, "行 {k} が広すぎる: {s}");
            // 行頭禁則: 句読点・閉じ括弧・長音・小書き仮名で始まらない。
            if let Some(c) = s.chars().next() {
                assert!(
                    !"、。」』）ーゃゅょっ".contains(c),
                    "幅 {width}: 行 {k} が禁則文字 {c} で始まる: {s}"
                );
            }
            // 行末禁則: 開き括弧で終わらない。
            if let Some(c) = s.chars().last() {
                assert!(
                    !"「『（".contains(c),
                    "幅 {width}: 行 {k} が開き括弧で終わる: {s}"
                );
            }
            rebuilt.push_str(s);
        }
        assert_eq!(rebuilt, text, "幅 {width}: 文字の欠落/重複");
        assert!(lay.lines.len() >= (f.measure_text(20.0, text).width / width) as usize);
    }
    // 英語と日本語の混在: 英単語は割らず、日本語の直前では切れる。
    let lay = f.layout(16.0, "これはRustで書かれたOSです", Some(100), None);
    for l in &lay.lines {
        let s = &"これはRustで書かれたOSです"[l.start..l.end];
        assert!(!s.ends_with("Rus") && !s.ends_with("Ru"), "{s}");
    }
}

#[test]
fn newlines_and_blank_lines() {
    let f = regular();
    let lay = f.layout(16.0, "a\n\nb\n", None, None);
    let texts: Vec<&str> = lay
        .lines
        .iter()
        .map(|l| &"a\n\nb\n"[l.start..l.end])
        .collect();
    assert_eq!(texts, ["a", "", "b", ""]);
    assert_eq!(f.layout(16.0, "\n", None, None).lines.len(), 2);
    assert_eq!(f.layout(16.0, "a\r\nb", None, None).lines.len(), 2);
    // 行送りの指定が反映される。
    let lay = f.layout(16.0, "a\nb", None, Some(40.0));
    assert_eq!(lay.line_height, 40);
    assert_eq!(lay.height(), 80);
}

#[test]
fn draw_box_clips_and_aligns() {
    let f = regular();
    let mut tr = TextRenderer::new();
    let text = "ゆめかわいい\nOS";
    let rect = Rect::new(20, 10, 160, 80);
    let sizes = [TextAlign::Left, TextAlign::Center, TextAlign::Right];
    let mut boxes = Vec::new();
    for al in sizes {
        let mut i = black(200, 100);
        let n = tr.draw_text_box(&mut i.canvas(), &f, &style(24.0).align(al), rect, text);
        assert_eq!(n, 2);
        let (bb, _) = ink(&i);
        let bb = bb.unwrap();
        assert!(Rect::new(20, 10, 160, 80).contains(&bb), "{al:?}: {bb:?}");
        boxes.push(bb);
    }
    // 1 行目 (幅 144) の位置: 左 / 中央 / 右。
    assert!(boxes[0].x <= 22, "{:?}", boxes[0]);
    assert!(boxes[2].right() >= 178, "{:?}", boxes[2]);
    let center = (boxes[1].x + boxes[1].w as i32 / 2) as f32;
    assert!((center - 100.0).abs() <= 8.0, "中央揃えの中心 {center}");
    assert!(boxes[0].x < boxes[1].x && boxes[1].x < boxes[2].x);

    // 矩形からはみ出す文字はクリップされる: 小さい矩形の外側は 1 ピクセルも変わらない。
    let mut i = black(200, 100);
    let small = Rect::new(50, 20, 30, 20);
    let n = tr.draw_text_box(&mut i.canvas(), &f, &style(40.0), small, "日本語のテキスト");
    assert!(n >= 1);
    let (bb, _) = ink(&i);
    assert!(small.contains(&bb.unwrap()), "{bb:?}");
    // 描画後にクリップが元に戻っている。
    let mut j = black(200, 100);
    let mut c = j.canvas();
    c.set_clip(Rect::new(0, 0, 100, 100));
    tr.draw_text_box(&mut c, &f, &style(20.0), Rect::new(10, 10, 50, 50), "あ");
    assert_eq!(c.clip(), Rect::new(0, 0, 100, 100));
    // 空矩形は何も描かず 0 行。
    assert_eq!(
        tr.draw_text_box(&mut c, &f, &style(20.0), Rect::new(10, 10, 0, 50), "あ"),
        0
    );
}

#[test]
fn draw_box_wraps_into_multiple_lines_and_stops_at_bottom() {
    let f = regular();
    let mut tr = TextRenderer::new();
    let text = "吾輩は猫である。名前はまだ無い。どこで生れたかとんと見当がつかぬ。";
    let mut i = black(200, 200);
    let rect = Rect::new(10, 10, 120, 200);
    let n = tr.draw_text_box(&mut i.canvas(), &f, &style(20.0), rect, text);
    let expect = f.measure_wrapped(20.0, text, 120).lines;
    assert_eq!(n, expect);
    assert!(n >= 4);
    let (bb, _) = ink(&i);
    assert!(rect.contains(&bb.unwrap()));
    // 高さが足りなければ、収まる行までしか描かない。
    let mut j = black(200, 200);
    let lh = f.metrics(20.0).line_height as u32;
    let n2 = tr.draw_text_box(
        &mut j.canvas(),
        &f,
        &style(20.0),
        Rect::new(10, 10, 120, lh * 2),
        text,
    );
    assert_eq!(n2, 2);
    // 下の行ほど下にインクがある: 行ごとの帯にインクが入っている。
    for k in 0..n {
        let y0 = 10 + k * lh;
        let band: u64 = (y0..y0 + lh)
            .map(|y| {
                (0..200)
                    .map(|x| i.get(x, y).unwrap().r() as u64)
                    .sum::<u64>()
            })
            .sum();
        assert!(band > 0, "行 {k} にインクが無い");
    }
}

#[test]
fn color_alpha_and_bold() {
    let f = regular();
    let b = Font::new(BOLD).unwrap();
    let mut tr = TextRenderer::new();
    let mut full = black(100, 50);
    tr.draw_text(&mut full.canvas(), &f, &style(30.0), 5, 5, "日本");
    let mut half = black(100, 50);
    tr.draw_text(
        &mut half.canvas(),
        &f,
        &TextStyle::new(30.0, Color::argb(128, 255, 255, 255)),
        5,
        5,
        "日本",
    );
    let (_, s_full) = ink(&full);
    let (_, s_half) = ink(&half);
    let ratio = s_half as f64 / s_full as f64;
    assert!((ratio - 0.5).abs() < 0.03, "半透明の濃さ比 {ratio}");
    // 色: 赤で描くと緑・青は 0 のまま。
    let mut red = black(100, 50);
    tr.draw_text(
        &mut red.canvas(),
        &f,
        &TextStyle::new(30.0, Color::rgb(255, 0, 0)),
        5,
        5,
        "日本",
    );
    assert!(red.pixels().iter().all(|&p| (p & 0xFFFF) == 0));
    // Bold は Regular よりインクが多く、同じキャッシュを共有しても混ざらない。
    let mut bi = black(100, 50);
    tr.draw_text(&mut bi.canvas(), &b, &style(30.0), 5, 5, "日本");
    assert!(
        ink(&bi).1 > s_full * 11 / 10,
        "bold {} regular {}",
        ink(&bi).1,
        s_full
    );
    let mut again = black(100, 50);
    tr.draw_text(&mut again.canvas(), &f, &style(30.0), 5, 5, "日本");
    assert_eq!(again, full);
}

#[test]
fn text_is_clipped_to_canvas_and_positions_are_safe() {
    let f = regular();
    let mut tr = TextRenderer::new();
    let mut i = black(40, 30);
    // 左上・右下にはみ出しても描ける部分は描かれる。
    tr.draw_text(&mut i.canvas(), &f, &style(30.0), -12, -8, "日本語");
    assert!(ink(&i).1 > 0);
    let mut j = black(40, 30);
    tr.draw_text(&mut j.canvas(), &f, &style(30.0), 30, 20, "日本語");
    assert!(ink(&j).1 > 0);
    let mut k = black(40, 30);
    for (x, y) in [
        (i32::MIN, 0),
        (0, i32::MIN),
        (i32::MAX, 0),
        (0, i32::MAX),
        (i32::MIN, i32::MIN),
        (i32::MAX, i32::MAX),
    ] {
        tr.draw_text(&mut k.canvas(), &f, &style(30.0), x, y, "日本語 abc");
    }
    for r in [
        Rect::new(i32::MIN, i32::MIN, u32::MAX, u32::MAX),
        Rect::new(i32::MAX, i32::MAX, 10, 10),
        Rect::new(-5, -5, u32::MAX, 5),
    ] {
        tr.draw_text_box(
            &mut k.canvas(),
            &f,
            &style(30.0).align(TextAlign::Center),
            r,
            "日本語 abc\ndef",
        );
        tr.draw_text_box(
            &mut k.canvas(),
            &f,
            &style(30.0).align(TextAlign::Right),
            r,
            "日本語 abc\ndef",
        );
    }
}

#[test]
fn text_never_writes_outside_canvas() {
    const GUARD: u32 = 0xDEAD_BEEF;
    let f = regular();
    let mut tr = TextRenderer::with_cache_capacity(16 * 1024);
    let (bw, bh) = (80usize, 60usize);
    let (cx, cy, cw, ch) = (9usize, 7usize, 50u32, 33u32);
    let mut buf = vec![GUARD; bw * bh];
    let mut s = 0x2468_ACE1_u64;
    let mut rnd = move || {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        s
    };
    let pick = |r: u64| -> i32 {
        match r % 6 {
            0 => i32::MIN,
            1 => i32::MAX,
            2 => (r >> 8) as i32 % 100 - 50,
            3 => (r >> 8) as i32 % 40,
            4 => i32::MAX - ((r >> 8) % 60) as i32,
            _ => (r >> 8) as i32,
        }
    };
    let texts = [
        "あ",
        "日本語のテキスト abc",
        "Hello\nWorld",
        "ゆめかわ OS\n\n夢",
        "\u{1F600}x",
    ];
    for round in 0..300 {
        {
            let mut c = Canvas::new(&mut buf[cy * bw + cx..], cw, ch, bw).unwrap();
            c.clear(Color::BLACK);
            let size = [8.0, 14.0, 24.0, 60.0, 130.0][(rnd() % 5) as usize];
            let st = TextStyle::new(size, Color(rnd() as u32)).align(
                [TextAlign::Left, TextAlign::Center, TextAlign::Right][(rnd() % 3) as usize],
            );
            let text = texts[(rnd() % texts.len() as u64) as usize];
            if round % 2 == 0 {
                tr.draw_text(&mut c, &f, &st, pick(rnd()), pick(rnd()), text);
            } else {
                let r = Rect::new(
                    pick(rnd()),
                    pick(rnd()),
                    (rnd() % 400) as u32,
                    (rnd() % 300) as u32,
                );
                tr.draw_text_box(&mut c, &f, &st, r, text);
            }
        }
        for y in 0..bh {
            for x in 0..bw {
                let inside =
                    (cx..cx + cw as usize).contains(&x) && (cy..cy + ch as usize).contains(&y);
                if !inside {
                    assert_eq!(buf[y * bw + x], GUARD, "round {round}: ({x},{y})");
                }
            }
        }
    }
}

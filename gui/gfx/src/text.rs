//! アウトラインフォントによる UTF-8 テキスト描画。
//!
//! # 流れ
//!
//! 1. [`Font::new`] が TrueType/OpenType のバイト列 (`&[u8]`、通常は `&'static [u8]`) を
//!    `ab_glyph` で解析する。解析は遅延で、読み込み時のヒープ確保はほぼ無い。
//! 2. 配置 (`measure_text` / `layout`) は **整数のみ** (26.6 固定小数点の送り幅) で行う。
//!    送り幅は `hmtx` の値 x サイズ / unitsPerEm。カーニング・合字・シェーピングは未対応。
//! 3. 描画 ([`TextRenderer`]) は文字ごとにグリフキャッシュを引く。ミスしたときだけ
//!    `ab_glyph` でアウトラインをラスタライズ (浮動小数点を使う唯一の箇所) して、
//!    8bit カバレッジのビットマップとして保存する。キャッシュのキーは
//!    `(フォント, グリフ ID, サイズ[1/8px 単位])`。
//! 4. ビットマップを [`Canvas::draw_mask`] で色を付けて合成する (整数演算のみ)。
//!    グリフの位置は整数ピクセル (サブピクセル位置決めなし)。
//!
//! # キャッシュ
//!
//! 容量 (バイト) に上限があり、超えたら最も長く使われていないエントリから半分程度まで破棄する。
//! 上限の 1/4 を超える巨大グリフはキャッシュせずに毎回ラスタライズする。
//!
//! # 未対応
//!
//! 結合文字の位置合わせ、合字、BiDi (右から左)、縦書き、フォールバックフォントの連鎖、
//! 禁則処理の完全実装 (行頭・行末禁則の主要な文字のみ)、ぶら下げ。

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use core::fmt;
use core::sync::atomic::{AtomicU32, Ordering};

use ab_glyph::{Font as _, FontRef, GlyphId, PxScale, point};

use crate::canvas::{Canvas, MaskRef};
use crate::color::Color;
use crate::rect::Rect;

/// サイズ (px) の下限・上限。範囲外・非有限値は丸められる。
pub const MIN_FONT_SIZE: f32 = 1.0;
/// 最大フォントサイズ (px)。
pub const MAX_FONT_SIZE: f32 = 1024.0;

/// フォント読み込みのエラー。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FontError {
    /// TrueType/OpenType として解析できない。
    InvalidFont,
    /// `unitsPerEm` などの必須メトリクスが無い/不正。
    InvalidMetrics,
}

impl fmt::Display for FontError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FontError::InvalidFont => f.write_str("not a valid TrueType/OpenType font"),
            FontError::InvalidMetrics => f.write_str("font has invalid metrics"),
        }
    }
}

impl core::error::Error for FontError {}

/// 横方向の揃え。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum TextAlign {
    /// 左揃え
    #[default]
    Left,
    /// 中央揃え
    Center,
    /// 右揃え
    Right,
}

/// 描画スタイル。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextStyle {
    /// フォントサイズ (px。CSS の font-size と同じく em の大きさ)。
    pub size: f32,
    /// 文字色 (アルファも有効)。
    pub color: Color,
    /// 横方向の揃え (ボックス描画のとき、ボックス幅に対して)。
    pub align: TextAlign,
    /// 行送り (px)。`None` ならフォントの標準 (ascent + descent + lineGap)。
    pub line_height: Option<f32>,
}

impl TextStyle {
    /// 左揃え・標準の行送りで作る。
    pub const fn new(size: f32, color: Color) -> TextStyle {
        TextStyle {
            size,
            color,
            align: TextAlign::Left,
            line_height: None,
        }
    }
    /// 揃えを変える。
    pub const fn align(mut self, align: TextAlign) -> TextStyle {
        self.align = align;
        self
    }
    /// 行送り (px) を指定する。
    pub const fn line_height(mut self, px: f32) -> TextStyle {
        self.line_height = Some(px);
        self
    }
}

/// あるサイズでのフォントのメトリクス (px、整数に丸めたもの)。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FontMetrics {
    /// ベースラインから上端まで (正)
    pub ascent: i32,
    /// ベースラインから下端まで (正)
    pub descent: i32,
    /// 行間 (lineGap)
    pub line_gap: i32,
    /// 標準の行送り = ascent + descent + line_gap
    pub line_height: i32,
}

/// 測定結果。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextMetrics {
    /// 最も長い行の幅 (px、切り上げ)
    pub width: u32,
    /// 全行の高さ (行数 x 行送り)
    pub height: u32,
    /// 行数
    pub lines: u32,
}

/// 1 行ぶんの範囲。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextLine {
    /// 元の文字列でのバイト範囲の開始
    pub start: usize,
    /// バイト範囲の終了 (行末の空白と改行は含まない)
    pub end: usize,
    /// 行の幅 (26.6 固定小数点, 行末の空白を除く)
    pub width64: i64,
}

impl TextLine {
    /// 行の幅 (px、切り上げ)。
    pub fn width(&self) -> u32 {
        ((self.width64.max(0) + 63) >> 6) as u32
    }
}

/// レイアウト結果 (行分割と寸法)。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextLayout {
    /// 行 (空行も含む)
    pub lines: Vec<TextLine>,
    /// 行送り (px)
    pub line_height: i32,
    /// 最初の行の上端からベースラインまで (px)
    pub baseline: i32,
    /// 折り返し幅。折り返さない場合は最も長い行の幅
    pub box_width: u32,
}

impl TextLayout {
    /// 最も長い行の幅 (px)。
    pub fn width(&self) -> u32 {
        self.lines.iter().map(|l| l.width()).max().unwrap_or(0)
    }
    /// 全体の高さ (px)。
    pub fn height(&self) -> u32 {
        (self.lines.len() as i64 * self.line_height.max(0) as i64).min(u32::MAX as i64) as u32
    }
}

static NEXT_FONT_ID: AtomicU32 = AtomicU32::new(1);

/// 解析済みのフォント。フォントデータは借用 (`'a`)。
pub struct Font<'a> {
    id: u16,
    face: FontRef<'a>,
    upem: i64,
    ascent_u: i64,
    descent_u: i64,
    gap_u: i64,
    height_u: f32,
}

impl fmt::Debug for Font<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Font")
            .field("id", &self.id)
            .field("units_per_em", &self.upem)
            .finish_non_exhaustive()
    }
}

impl<'a> Font<'a> {
    /// TrueType/OpenType (`.ttf` / `.otf`) のバイト列から作る。
    pub fn new(data: &'a [u8]) -> Result<Font<'a>, FontError> {
        let face = FontRef::try_from_slice(data).map_err(|_| FontError::InvalidFont)?;
        let upem = face.units_per_em().ok_or(FontError::InvalidMetrics)?;
        if !(16.0..=16384.0).contains(&upem) {
            return Err(FontError::InvalidMetrics);
        }
        let ascent_u = face.ascent_unscaled() as i64;
        let descent_u = -(face.descent_unscaled() as i64);
        let gap_u = (face.line_gap_unscaled() as i64).max(0);
        let height_u = face.height_unscaled();
        if ascent_u <= 0
            || descent_u < 0
            || height_u.partial_cmp(&0.0) != Some(core::cmp::Ordering::Greater)
        {
            return Err(FontError::InvalidMetrics);
        }
        Ok(Font {
            id: NEXT_FONT_ID.fetch_add(1, Ordering::Relaxed) as u16,
            face,
            upem: upem as i64,
            ascent_u,
            descent_u,
            gap_u,
            height_u,
        })
    }

    /// この文字のグリフを持つか (豆腐になるなら false)。
    pub fn has_glyph(&self, ch: char) -> bool {
        self.face.glyph_id(ch).0 != 0
    }

    /// サイズ `size` (px) でのメトリクス。
    pub fn metrics(&self, size: f32) -> FontMetrics {
        let s = size64(size);
        let scale = |u: i64| -> i32 { ((u * s + self.upem * 32) / (self.upem * 64)) as i32 };
        let ascent = scale(self.ascent_u);
        let descent = scale(self.descent_u);
        let line_gap = scale(self.gap_u);
        FontMetrics {
            ascent,
            descent,
            line_gap,
            line_height: ascent + descent + line_gap,
        }
    }

    /// 1 文字の送り幅 (26.6 固定小数点)。
    fn advance64(&self, ch: char, s64: i64) -> i64 {
        if ch == '\t' {
            return 4 * self.advance64(' ', s64);
        }
        if is_invisible(ch) {
            return 0;
        }
        let gid = self.face.glyph_id(ch);
        if gid.0 == 0 {
            return s64 * 4 / 5; // 豆腐
        }
        let adv = self.face.h_advance_unscaled(gid) as i64;
        (adv * s64 + self.upem / 2) / self.upem
    }

    /// 改行を含む文字列の寸法 (折り返しなし)。
    pub fn measure_text(&self, size: f32, text: &str) -> TextMetrics {
        let l = self.layout(size, text, None, None);
        TextMetrics {
            width: l.width(),
            height: l.height(),
            lines: l.lines.len() as u32,
        }
    }

    /// `max_width` px で折り返したときの寸法。
    pub fn measure_wrapped(&self, size: f32, text: &str, max_width: u32) -> TextMetrics {
        let l = self.layout(size, text, Some(max_width), None);
        TextMetrics {
            width: l.width(),
            height: l.height(),
            lines: l.lines.len() as u32,
        }
    }

    /// 行分割を行う。`max_width` が `Some` なら折り返す (空白・CJK 文字間で改行。禁則あり)。
    /// `\n` は常に改行、`\r` は無視。`line_height` (px) が `None` なら標準の行送り。
    pub fn layout(
        &self,
        size: f32,
        text: &str,
        max_width: Option<u32>,
        line_height: Option<f32>,
    ) -> TextLayout {
        let s64 = size64(size);
        let m = self.metrics(size);
        let pitch = match line_height {
            Some(lh) if lh.is_finite() && lh >= 1.0 => (lh.min(65536.0) + 0.5) as i32,
            _ => m.line_height.max(1),
        };
        // 行内の縦位置: 標準の高さ (ascent + descent) を行送りの中央に置く (half-leading)。
        let baseline = (pitch - (m.ascent + m.descent)).div_euclid(2) + m.ascent;

        let mw64 = max_width.map(|w| (w as i64) << 6);
        let mut lines: Vec<TextLine> = Vec::new();
        let mut chars: Vec<(usize, char)> = Vec::new();
        let mut cum: Vec<i64> = Vec::new();

        let mut para_start = 0usize;
        loop {
            let para_end = text[para_start..]
                .find('\n')
                .map_or(text.len(), |i| para_start + i);
            let para = &text[para_start..para_end];
            chars.clear();
            cum.clear();
            cum.push(0);
            let mut acc = 0i64;
            for (i, ch) in para.char_indices() {
                if ch == '\r' {
                    continue;
                }
                acc += self.advance64(ch, s64);
                chars.push((para_start + i, ch));
                cum.push(acc);
            }
            wrap_paragraph(&chars, &cum, para_end, mw64, &mut lines);
            if para_end >= text.len() {
                break;
            }
            para_start = para_end + 1; // '\n' の次
        }
        let widest = lines.iter().map(|l| l.width()).max().unwrap_or(0);
        TextLayout {
            lines,
            line_height: pitch,
            baseline,
            box_width: max_width.unwrap_or(widest),
        }
    }
}

/// 1 段落 (改行を含まない) を行に分割して `out` に追加する。
fn wrap_paragraph(
    chars: &[(usize, char)],
    cum: &[i64],
    para_end_byte: usize,
    mw64: Option<i64>,
    out: &mut Vec<TextLine>,
) {
    let n = chars.len();
    let byte_at = |i: usize| -> usize { if i < n { chars[i].0 } else { para_end_byte } };
    let emit = |start: usize, end: usize, out: &mut Vec<TextLine>| {
        // 行末の空白を除く。
        let mut e = end;
        while e > start && is_space(chars[e - 1].1) {
            e -= 1;
        }
        out.push(TextLine {
            start: byte_at(start),
            end: byte_at(e),
            width64: cum[e] - cum[start],
        });
    };
    let Some(mw) = mw64 else {
        emit(0, n, out);
        return;
    };
    let mut line_start = 0usize;
    let mut last_break: Option<usize> = None;
    let mut i = 0usize;
    while i < n {
        let c = chars[i].1;
        if i > line_start && can_break(chars[i - 1].1, c) {
            last_break = Some(i);
        }
        if !is_space(c) && i > line_start && cum[i + 1] - cum[line_start] > mw {
            if let Some(b) = last_break.filter(|&b| b > line_start) {
                emit(line_start, b, out);
                let mut ns = b;
                while ns < n && is_space(chars[ns].1) {
                    ns += 1;
                }
                line_start = ns;
                last_break = None;
                i = line_start;
                continue;
            } else if !is_line_start_prohibited(c) {
                emit(line_start, i, out);
                line_start = i;
                last_break = None;
                continue;
            }
            // 行頭禁則の文字は、はみ出してもその行に置く。
        }
        i += 1;
    }
    emit(line_start, n, out);
}

/// `prev` と `next` の間で改行してよいか。
fn can_break(prev: char, next: char) -> bool {
    if is_space(next) {
        return false; // 空白の前では切らない (空白は行末にぶら下げて捨てる)
    }
    if is_line_start_prohibited(next) || is_line_end_prohibited(prev) {
        return false;
    }
    if is_space(prev) {
        return true;
    }
    if is_cjk(prev) || is_cjk(next) {
        return true;
    }
    // 英単語中は切らない。ハイフンの直後は切ってよい。
    prev == '-' && next.is_alphanumeric()
}

#[inline]
fn is_space(c: char) -> bool {
    c == ' ' || c == '\t'
}

/// 日本語・中国語の文字 (文字間で改行してよいもの)。
pub(crate) fn is_cjk(c: char) -> bool {
    matches!(c as u32,
        0x2E80..=0x2FDF   // 部首
        | 0x3000..=0x30FF // CJK 記号・ひらがな・カタカナ
        | 0x31F0..=0x31FF // カタカナ拡張
        | 0x3400..=0x4DBF // 拡張 A
        | 0x4E00..=0x9FFF // CJK 統合漢字
        | 0xF900..=0xFAFF // 互換漢字
        | 0xFF00..=0xFFEF // 全角形・半角カナ
        | 0x20000..=0x2FFFF)
}

/// 行頭に置いてはいけない文字 (閉じ括弧・句読点・小書き仮名・長音など)。
fn is_line_start_prohibited(c: char) -> bool {
    matches!(
        c,
        '、' | '。'
            | '，'
            | '．'
            | '・'
            | '：'
            | '；'
            | '？'
            | '！'
            | '）'
            | '］'
            | '｝'
            | '〕'
            | '〉'
            | '》'
            | '」'
            | '』'
            | '】'
            | '〙'
            | '〗'
            | 'ー'
            | '～'
            | '…'
            | '‥'
            | 'ぁ'
            | 'ぃ'
            | 'ぅ'
            | 'ぇ'
            | 'ぉ'
            | 'っ'
            | 'ゃ'
            | 'ゅ'
            | 'ょ'
            | 'ゎ'
            | 'ァ'
            | 'ィ'
            | 'ゥ'
            | 'ェ'
            | 'ォ'
            | 'ッ'
            | 'ャ'
            | 'ュ'
            | 'ョ'
            | 'ヮ'
            | ','
            | '.'
            | ')'
            | ']'
            | '}'
            | '!'
            | '?'
            | ':'
            | ';'
            | '%'
    )
}

/// 行末に置いてはいけない文字 (開き括弧)。
fn is_line_end_prohibited(c: char) -> bool {
    matches!(
        c,
        '（' | '［'
            | '｛'
            | '〔'
            | '〈'
            | '《'
            | '「'
            | '『'
            | '【'
            | '〘'
            | '〖'
            | '('
            | '['
            | '{'
    )
}

/// 表示も送りもしない文字 (制御文字、ゼロ幅文字、異体字セレクタなど)。
fn is_invisible(c: char) -> bool {
    c.is_control()
        || matches!(c as u32, 0x200B..=0x200F | 0x202A..=0x202E | 0x2060..=0x2064 | 0xFE00..=0xFE0F | 0xFEFF)
}

/// サイズを 26.6 固定小数点にする (範囲外・NaN は丸める)。
fn size64(size: f32) -> i64 {
    let s = if size.is_nan() {
        16.0
    } else {
        size.clamp(MIN_FONT_SIZE, MAX_FONT_SIZE)
    };
    (s * 64.0 + 0.5) as i64
}

// ---------------------------------------------------------------------------
// グリフキャッシュ
// ---------------------------------------------------------------------------

struct CachedGlyph {
    /// ペン位置 (ベースライン上) から見たビットマップ左上の x オフセット
    left: i32,
    /// ベースラインから見たビットマップ上端の y オフセット (上向きが負)
    top: i32,
    w: u32,
    h: u32,
    data: Box<[u8]>,
    last_used: u64,
}

impl CachedGlyph {
    fn cost(&self) -> usize {
        self.data.len() + 96
    }
}

/// キャッシュの統計。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CacheStats {
    /// 現在のエントリ数
    pub entries: usize,
    /// 現在の使用量 (バイト, 概算)
    pub bytes: usize,
    /// 容量の上限 (バイト)
    pub max_bytes: usize,
    /// ヒット数
    pub hits: u64,
    /// ミス数 (ラスタライズした回数)
    pub misses: u64,
    /// 容量超過による破棄の回数 (エントリ単位)
    pub evicted: u64,
}

/// テキスト描画器。グリフキャッシュを持つ (フォントは呼び出しごとに渡す)。
#[derive(Debug)]
pub struct TextRenderer {
    map: BTreeMap<u64, CachedGlyph>,
    bytes: usize,
    max_bytes: usize,
    tick: u64,
    hits: u64,
    misses: u64,
    evicted: u64,
}

impl fmt::Debug for CachedGlyph {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "CachedGlyph({}x{})", self.w, self.h)
    }
}

/// キャッシュ容量の既定値 (2 MiB)。
pub const DEFAULT_CACHE_BYTES: usize = 2 * 1024 * 1024;

impl Default for TextRenderer {
    fn default() -> Self {
        TextRenderer::new()
    }
}

impl TextRenderer {
    /// 既定の容量 ([`DEFAULT_CACHE_BYTES`]) で作る。
    pub fn new() -> TextRenderer {
        TextRenderer::with_cache_capacity(DEFAULT_CACHE_BYTES)
    }

    /// キャッシュ容量 (バイト) を指定して作る。0 ならキャッシュしない。
    pub fn with_cache_capacity(max_bytes: usize) -> TextRenderer {
        TextRenderer {
            map: BTreeMap::new(),
            bytes: 0,
            max_bytes,
            tick: 0,
            hits: 0,
            misses: 0,
            evicted: 0,
        }
    }

    /// キャッシュの統計。
    pub fn cache_stats(&self) -> CacheStats {
        CacheStats {
            entries: self.map.len(),
            bytes: self.bytes,
            max_bytes: self.max_bytes,
            hits: self.hits,
            misses: self.misses,
            evicted: self.evicted,
        }
    }

    /// キャッシュを空にする。
    pub fn clear_cache(&mut self) {
        self.map.clear();
        self.bytes = 0;
    }

    /// キャッシュ容量を変更する (縮めたときは超過ぶんを破棄)。
    pub fn set_cache_capacity(&mut self, max_bytes: usize) {
        self.max_bytes = max_bytes;
        self.evict_to(max_bytes / 2);
    }

    fn key(font: &Font<'_>, gid: u16, s64: i64) -> u64 {
        // サイズは 1/8px 単位に量子化 (26.6 → /8)。
        let q = ((s64 + 4) >> 3) as u64 & 0xFFFF_FFFF;
        ((font.id as u64) << 48) | ((gid as u64) << 32) | q
    }

    /// 上限を超えたときに、最終使用が古い順に `target` バイト以下になるまで破棄する。
    fn evict_to(&mut self, target: usize) {
        if self.bytes <= target {
            return;
        }
        let mut order: Vec<(u64, u64)> = self.map.iter().map(|(k, v)| (v.last_used, *k)).collect();
        order.sort_unstable();
        for (_, k) in order {
            if self.bytes <= target {
                break;
            }
            if let Some(g) = self.map.remove(&k) {
                self.bytes -= g.cost();
                self.evicted += 1;
            }
        }
    }

    /// グリフを取り出す。ミスなら生成して (可能なら) キャッシュする。
    /// 返す参照はキャッシュ内、またはキャッシュしなかった一時値 (`tmp`)。
    fn glyph<'t>(
        &'t mut self,
        font: &Font<'_>,
        ch: char,
        s64: i64,
        tmp: &'t mut Option<CachedGlyph>,
    ) -> Option<&'t CachedGlyph> {
        let gid = font.face.glyph_id(ch).0; // 0 = 豆腐
        let key = Self::key(font, gid, s64);
        self.tick += 1;
        let tick = self.tick;
        if self.map.contains_key(&key) {
            self.hits += 1;
            let g = self.map.get_mut(&key)?;
            g.last_used = tick;
            return Some(&*g);
        }
        self.misses += 1;
        let mut g = if gid == 0 {
            tofu_glyph(s64)
        } else {
            rasterize(font, GlyphId(gid), s64)?
        };
        g.last_used = tick;
        let cost = g.cost();
        if cost > self.max_bytes / 4 {
            *tmp = Some(g);
            return tmp.as_ref();
        }
        if self.bytes + cost > self.max_bytes {
            self.evict_to((self.max_bytes / 2).min(self.max_bytes.saturating_sub(cost)));
        }
        self.bytes += cost;
        self.map.insert(key, g);
        self.map.get(&key)
    }

    /// 1 行を描く。`(x, y)` は **行ボックスの左上**。改行文字は無視される (1 行のみ)。
    /// `style.line_height` で行ボックスの高さを変えられる。描いた幅 (px) を返す。
    pub fn draw_text(
        &mut self,
        canvas: &mut Canvas<'_>,
        font: &Font<'_>,
        style: &TextStyle,
        x: i32,
        y: i32,
        text: &str,
    ) -> u32 {
        let lay = font.layout(style.size, text, None, style.line_height);
        // 1 行のみ: 先頭行だけ描く (改行以降は捨てる)。
        let Some(line) = lay.lines.first() else {
            return 0;
        };
        let w = line.width();
        self.draw_line(
            canvas,
            font,
            style,
            x as i64,
            y as i64 + lay.baseline as i64,
            text,
            line,
        );
        w
    }

    /// 矩形内に折り返して描く。`rect` の外にはみ出す部分はクリップされる。
    /// `style.align` は `rect` の幅に対する揃え。描いた行数を返す。
    pub fn draw_text_box(
        &mut self,
        canvas: &mut Canvas<'_>,
        font: &Font<'_>,
        style: &TextStyle,
        rect: Rect,
        text: &str,
    ) -> u32 {
        if rect.is_empty() {
            return 0;
        }
        let lay = font.layout(style.size, text, Some(rect.w), style.line_height);
        let saved = canvas.clip();
        canvas.set_clip(saved.intersect(&rect));
        let clip = canvas.clip();
        let mut drawn = 0;
        for (i, line) in lay.lines.iter().enumerate() {
            let top = rect.y as i64 + i as i64 * lay.line_height as i64;
            if top >= rect.bottom() || top >= clip.bottom() {
                break;
            }
            if top + (lay.line_height as i64) <= clip.top() {
                continue;
            }
            let off = match style.align {
                TextAlign::Left => 0,
                TextAlign::Center => (rect.w as i64 - line.width() as i64).div_euclid(2),
                TextAlign::Right => rect.w as i64 - line.width() as i64,
            };
            self.draw_line(
                canvas,
                font,
                style,
                rect.x as i64 + off,
                top + lay.baseline as i64,
                text,
                line,
            );
            drawn += 1;
        }
        canvas.set_clip(saved);
        drawn
    }

    /// 1 行を、ペン開始位置 `x` とベースライン `baseline_y` で描く。
    #[allow(clippy::too_many_arguments)]
    fn draw_line(
        &mut self,
        canvas: &mut Canvas<'_>,
        font: &Font<'_>,
        style: &TextStyle,
        x: i64,
        baseline_y: i64,
        text: &str,
        line: &TextLine,
    ) {
        let s64 = size64(style.size);
        let clip = canvas.clip();
        let mut pen = x << 6;
        for ch in text[line.start..line.end].chars() {
            if ch == '\n' {
                break;
            }
            let adv = font.advance64(ch, s64);
            if is_invisible(ch) || is_space(ch) {
                pen += adv;
                continue;
            }
            let px = (pen + 32) >> 6;
            pen += adv;
            // 右端がクリップの外なら、以降は描かない (行は左から右へ進む)。
            if px > clip.right() {
                break;
            }
            let mut tmp = None;
            if let Some(g) = self.glyph(font, ch, s64, &mut tmp)
                && g.w > 0
                && g.h > 0
            {
                let gx = px + g.left as i64;
                let gy = baseline_y + g.top as i64;
                if let (Ok(gx), Ok(gy)) = (i32::try_from(gx), i32::try_from(gy)) {
                    let mask = MaskRef::new(g.w, g.h, &g.data, g.w as usize);
                    canvas.draw_mask(gx, gy, &mask, style.color);
                }
            }
        }
    }
}

/// フォントのアウトラインをラスタライズする。空 (スペース等) なら幅 0 のグリフ。
fn rasterize(font: &Font<'_>, gid: GlyphId, s64: i64) -> Option<CachedGlyph> {
    // ab_glyph の PxScale は (ascent - descent) の高さ [px]。em サイズから換算する。
    let em = s64 as f32 / 64.0;
    let scale = PxScale::from(em * font.height_u / font.upem as f32);
    let glyph = gid.with_scale_and_position(scale, point(0.0, 0.0));
    let empty = CachedGlyph {
        left: 0,
        top: 0,
        w: 0,
        h: 0,
        data: Box::default(),
        last_used: 0,
    };
    let Some(og) = font.face.outline_glyph(glyph) else {
        return Some(empty);
    };
    let b = og.px_bounds();
    let (w, h) = (b.max.x - b.min.x, b.max.y - b.min.y);
    if !(w >= 1.0 && h >= 1.0) || w > 8192.0 || h > 8192.0 {
        return Some(empty);
    }
    let (w, h) = (w as u32, h as u32);
    let mut data = Vec::new();
    data.try_reserve_exact(w as usize * h as usize).ok()?;
    data.resize(w as usize * h as usize, 0u8);
    og.draw(|x, y, c| {
        if x < w && y < h {
            let v = (c * 255.0 + 0.5) as u32;
            data[y as usize * w as usize + x as usize] = v.min(255) as u8;
        }
    });
    Some(CachedGlyph {
        left: b.min.x as i32,
        top: b.min.y as i32,
        w,
        h,
        data: data.into_boxed_slice(),
        last_used: 0,
    })
}

/// グリフが無い文字の代わりに描く豆腐 (□)。サイズに比例した細い枠。
fn tofu_glyph(s64: i64) -> CachedGlyph {
    let size = (s64 + 32) >> 6; // px
    let w = ((size * 3 + 2) / 5).max(3) as u32; // 0.6em
    let h = ((size * 7 + 5) / 10).max(4) as u32; // 0.7em
    let t = ((size + 8) / 16).max(1) as u32; // 線の太さ
    let mut data = alloc::vec![0u8; w as usize * h as usize];
    for y in 0..h {
        for x in 0..w {
            let edge = x < t || y < t || x >= w - t.min(w) || y >= h - t.min(h);
            if edge {
                data[(y * w + x) as usize] = 255;
            }
        }
    }
    CachedGlyph {
        left: ((size + 5) / 10) as i32, // 左のあき 0.1em
        top: -(h as i32),
        w,
        h,
        data: data.into_boxed_slice(),
        last_used: 0,
    }
}

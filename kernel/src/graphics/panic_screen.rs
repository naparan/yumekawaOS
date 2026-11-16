//! Kernel Panic 画面 (§30.2)。
//!
//! エラー種別・エラーコード・CPU 情報・命令ポインタ・レジスタ・直近のカーネルログを
//! 8x8 フォントで表示する。パニック経路から呼ばれるので、ヒープ確保をせず、
//! 固定長の文字列 ([`StackString`]) だけで組み立てる。見た目の作り込みは Phase 3/4。
//!
//! 配色 (テストの機械的チェックでも使う):
//! - 背景: [`BACKGROUND`] (深い紫)
//! - 上部の帯: [`HEADER`] (ホットピンク, 画面高さの 1/8)

use core::fmt::Write;

use super::color::{Rgb, palette};
use super::font::{self, GLYPH_SIZE};
use super::framebuffer::Framebuffer;
use crate::arch::x86_64::trap::TrapFrame;
use crate::fmtbuf::StackString;

/// 背景色。
pub const BACKGROUND: Rgb = Rgb::new(48, 24, 64);
/// 上部の帯の色。
pub const HEADER: Rgb = palette::HOT_PINK;
const TEXT: Rgb = palette::WHITE;
const LABEL: Rgb = palette::PASTEL_PINK;
const LOG_TEXT: Rgb = palette::LAVENDER;
const DIM: Rgb = Rgb::new(180, 160, 210);

/// パニック画面に表示する内容。
#[derive(Debug)]
pub struct PanicDetails<'a> {
    /// 種別 (例: "CPU exception #PF page fault" / "Rust panic")
    pub kind: &'a str,
    /// メッセージ
    pub message: &'a str,
    /// 発生場所 (Rust のパニックのとき)
    pub location: Option<&'a str>,
    /// エラーコード
    pub error_code: Option<u64>,
    /// 命令ポインタ
    pub rip: Option<u64>,
    /// CR2
    pub cr2: Option<u64>,
    /// レジスタ (例外のとき)
    pub frame: Option<&'a TrapFrame>,
    /// CPU 情報 (1 行)
    pub cpu: &'a str,
    /// その他の状態 (uptime・メモリ等, 1 行)
    pub status: &'a str,
    /// 直近のログ (UTF-8 の行の並び)
    pub log: &'a [u8],
    /// フレームポインタを辿ったリターンアドレス (新しい順)
    pub backtrace: &'a [u64],
}

struct Cursor<'f> {
    fb: &'f mut Framebuffer,
    x: i64,
    y: i64,
    scale: u64,
    cols: usize,
    bottom: i64,
}

impl Cursor<'_> {
    fn line_height(&self) -> i64 {
        (GLYPH_SIZE * self.scale + self.scale * 2) as i64
    }

    fn has_room(&self) -> bool {
        self.y + self.line_height() <= self.bottom
    }

    /// 折り返して 1 行 (以上) 描く。描けなくなったら `false`。
    fn text(&mut self, s: &str, color: Rgb) -> bool {
        let mut rest = s;
        loop {
            if !self.has_room() {
                return false;
            }
            let take = rest
                .char_indices()
                .nth(self.cols)
                .map_or(rest.len(), |(i, _)| i);
            font::draw_text(self.fb, self.x, self.y, &rest[..take], self.scale, color);
            self.y += self.line_height();
            rest = &rest[take..];
            if rest.is_empty() {
                return true;
            }
        }
    }

    fn labeled(&mut self, label: &str, value: &str) -> bool {
        if !self.has_room() {
            return false;
        }
        font::draw_text(self.fb, self.x, self.y, label, self.scale, LABEL);
        let lw = font::text_width(label, self.scale) as i64;
        let cols = self.cols.saturating_sub(label.chars().count()).max(1);
        let saved = (self.x, self.cols);
        self.x += lw;
        self.cols = cols;
        let ok = self.text(value, TEXT);
        (self.x, self.cols) = saved;
        ok
    }

    fn gap(&mut self) {
        self.y += self.line_height() / 2;
    }
}

/// パニック画面を描く。
pub fn draw(fb: &mut Framebuffer, d: &PanicDetails<'_>) {
    let width = fb.width() as i64;
    let height = fb.height() as i64;
    fb.fill_rect(0, 0, width as u64, height as u64, BACKGROUND);

    // 上部の帯とタイトル
    let header_h = (height / 8).max(24);
    fb.fill_rect(0, 0, width as u64, header_h as u64, HEADER);
    let title = "KERNEL PANIC";
    let title_scale = ((header_h as u64 * 5 / 10) / GLYPH_SIZE).clamp(1, 6);
    let title_y = (header_h - (GLYPH_SIZE * title_scale) as i64) / 2;
    let margin = (width / 40).max(8);
    font::draw_text(fb, margin, title_y, title, title_scale, TEXT);
    let sub = ":( yumekawaOS stopped to protect your data";
    let sub_x = margin + font::text_width(title, title_scale) as i64 + margin;
    let sub_scale = if sub_x + (font::text_width(sub, 2) as i64) + margin < width {
        2
    } else {
        1
    };
    if sub_x + (font::text_width(sub, sub_scale) as i64) < width {
        font::draw_text(
            fb,
            sub_x,
            title_y + ((GLYPH_SIZE * title_scale) as i64 - (GLYPH_SIZE * sub_scale) as i64) / 2,
            sub,
            sub_scale,
            BACKGROUND,
        );
    }

    let body_scale: u64 = if width >= 1024 && height >= 700 { 2 } else { 1 };
    let footer_h = (GLYPH_SIZE * body_scale * 3) as i64;
    let mut c = Cursor {
        fb,
        x: margin,
        y: header_h + margin / 2,
        scale: body_scale,
        cols: ((width - margin * 2) as u64 / (GLYPH_SIZE * body_scale)) as usize,
        bottom: height - footer_h,
    };

    c.labeled("error   : ", d.kind);
    c.labeled("message : ", d.message);
    if let Some(loc) = d.location {
        c.labeled("location: ", loc);
    }
    let mut line = StackString::<160>::new();
    if let Some(code) = d.error_code {
        let _ = write!(line, "{code:#x}");
        c.labeled("err code: ", line.as_str());
    }
    if let Some(rip) = d.rip {
        line = StackString::new();
        let _ = write!(line, "{rip:#018x}");
        c.labeled("rip     : ", line.as_str());
    }
    if let Some(cr2) = d.cr2 {
        line = StackString::new();
        let _ = write!(line, "{cr2:#018x}");
        c.labeled("cr2     : ", line.as_str());
    }
    if !d.backtrace.is_empty() {
        line = StackString::new();
        for addr in d.backtrace {
            let _ = write!(line, "{addr:#x} ");
        }
        c.labeled("trace   : ", line.as_str().trim_end());
    }
    c.labeled("cpu     : ", d.cpu);
    c.labeled("status  : ", d.status);

    // レジスタ (scale 1 で詰めて表示)
    if let Some(f) = d.frame {
        c.gap();
        let saved_scale = c.scale;
        let saved_cols = c.cols;
        c.scale = 1;
        c.cols = ((width - margin * 2) as u64 / GLYPH_SIZE) as usize;
        let rows: [[(&str, u64); 4]; 6] = [
            [
                ("rax", f.rax),
                ("rbx", f.rbx),
                ("rcx", f.rcx),
                ("rdx", f.rdx),
            ],
            [
                ("rsi", f.rsi),
                ("rdi", f.rdi),
                ("rbp", f.rbp),
                ("rsp", f.rsp),
            ],
            [("r8 ", f.r8), ("r9 ", f.r9), ("r10", f.r10), ("r11", f.r11)],
            [
                ("r12", f.r12),
                ("r13", f.r13),
                ("r14", f.r14),
                ("r15", f.r15),
            ],
            [
                ("rip", f.rip),
                ("cs ", f.cs),
                ("rfl", f.rflags),
                ("ss ", f.ss),
            ],
            [
                ("vec", f.vector),
                ("err", f.error_code),
                ("   ", 0),
                ("   ", 0),
            ],
        ];
        for row in rows {
            line = StackString::new();
            for (name, v) in row {
                if name.trim().is_empty() {
                    continue;
                }
                let _ = write!(line, "{name}={v:#018x}  ");
            }
            c.text(line.as_str(), DIM);
        }
        c.scale = saved_scale;
        c.cols = saved_cols;
    }

    // 直近のログ (入るだけ、末尾優先)
    c.gap();
    c.text("recent kernel log:", LABEL);
    c.scale = 1;
    c.cols = ((width - margin * 2) as u64 / GLYPH_SIZE) as usize;
    let line_h = c.line_height();
    let room_lines = ((c.bottom - c.y) / line_h).max(0) as usize;
    let text = core::str::from_utf8(d.log).unwrap_or("<log is not valid UTF-8>");
    // 末尾 room_lines 行 (折り返しを考慮して概算) を表示する
    let lines: usize = text.lines().count();
    let mut skip = lines.saturating_sub(room_lines);
    for l in text.lines() {
        if skip > 0 {
            skip -= 1;
            continue;
        }
        let l = l.trim_end_matches('\r');
        // 1 行に収まる分だけ (折り返さない)
        let take = l.char_indices().nth(c.cols).map_or(l.len(), |(i, _)| i);
        if !c.text(&l[..take], LOG_TEXT) {
            break;
        }
    }

    // フッタ
    let footer = "system halted. please restart the computer.";
    let fy = height - footer_h + (footer_h - (GLYPH_SIZE * body_scale) as i64) / 2;
    font::draw_text(c.fb, margin, fy, footer, body_scale, LABEL);
}

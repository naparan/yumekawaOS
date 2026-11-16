//! yk-gfx のリンク確認 + soft-float 速度プローブ。
//!
//! `x86_64-unknown-none` (SSE/x87 を使わない) でビルドした freestanding ELF を、Linux 上で
//! そのまま実行する (syscall を直接叩くだけで libc 不要)。カーネルに組み込む前に、
//! (1) 未解決シンボルなしでリンクできること、(2) 命令セット (xmm/x87 なし)、
//! (3) soft-float での実行時間 を確かめるための使い捨てコードで、`unsafe` を使っている。
//!
//! ```sh
//! cd gui/gfx/tools/softfloat-probe
//! RUSTFLAGS="-C relocation-model=static" cargo build --release --target x86_64-unknown-none
//! ./target/x86_64-unknown-none/release/softfloat-probe
//! objdump -d target/x86_64-unknown-none/release/softfloat-probe | grep -cE 'xmm|ymm'   # 0
//! ```
//!
//! (`relocation-model=static` が必要: 動的リンカ無しで実行するため。カーネルも同じ設定。)

#![no_std]
#![no_main]

extern crate alloc;

use core::alloc::{GlobalAlloc, Layout};
use core::arch::{asm, global_asm};
use core::cell::UnsafeCell;
use core::fmt::Write;
use core::sync::atomic::{AtomicUsize, Ordering};

use yk_gfx::png::{PngLimits, decode_png};
use yk_gfx::text::{Font, TextRenderer, TextStyle};
use yk_gfx::{BlendMode, Color, Filter, Image, Rect};

const HEAP_SIZE: usize = 512 << 20; // .bss なので実メモリは使った分だけ

/// 解放しないバンプアロケータ (プローブ用)。
struct Bump;
struct Heap(UnsafeCell<[u8; HEAP_SIZE]>);
// SAFETY: 単一スレッドでしか使わない。
unsafe impl Sync for Heap {}
static HEAP: Heap = Heap(UnsafeCell::new([0; HEAP_SIZE]));
static NEXT: AtomicUsize = AtomicUsize::new(0);

// SAFETY: 戻すポインタは HEAP 内の重ならない領域。
unsafe impl GlobalAlloc for Bump {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let start = (NEXT.load(Ordering::Relaxed) + l.align() - 1) & !(l.align() - 1);
        let end = start + l.size();
        if end > HEAP_SIZE {
            return core::ptr::null_mut();
        }
        NEXT.store(end, Ordering::Relaxed);
        // SAFETY: start < HEAP_SIZE。
        unsafe { (HEAP.0.get() as *mut u8).add(start) }
    }
    unsafe fn dealloc(&self, _: *mut u8, _: Layout) {}
}

#[global_allocator]
static ALLOC: Bump = Bump;

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    p(format_args!("PANIC: {info}\n"));
    sys_exit(1)
}

static FONT: &[u8] = include_bytes!("../../../../../assets/fonts/MPLUSRounded1c-Regular.ttf");
static KANJI: &str = include_str!("../../../tests/data/joyo_kanji.txt");
static PNGS: [&[u8]; 3] = [
    include_bytes!("../../../tests/data/rgba8_adam7.png"),
    include_bytes!("../../../tests/data/pal4_trns.png"),
    include_bytes!("../../../tests/data/gray4_adam7.png"),
];

// ---- Linux syscall ----

fn sys_write(buf: &[u8]) {
    // SAFETY: write(1, buf, len)。
    unsafe {
        asm!("syscall", inlateout("rax") 1usize => _, in("rdi") 1usize, in("rsi") buf.as_ptr(),
             in("rdx") buf.len(), lateout("rcx") _, lateout("r11") _, options(nostack));
    }
}

fn sys_exit(code: usize) -> ! {
    // SAFETY: exit(code)。
    unsafe { asm!("syscall", in("rax") 60usize, in("rdi") code, options(noreturn, nostack)) }
}

fn now_ns() -> u64 {
    let mut ts = [0i64; 2];
    // SAFETY: clock_gettime(CLOCK_MONOTONIC, &ts)。
    unsafe {
        asm!("syscall", inlateout("rax") 228usize => _, in("rdi") 1usize, in("rsi") ts.as_mut_ptr(),
             lateout("rcx") _, lateout("r11") _, options(nostack));
    }
    ts[0] as u64 * 1_000_000_000 + ts[1] as u64
}

struct Out {
    b: [u8; 320],
    n: usize,
}
impl Write for Out {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        for &c in s.as_bytes() {
            if self.n < self.b.len() {
                self.b[self.n] = c;
                self.n += 1;
            }
        }
        Ok(())
    }
}
fn p(args: core::fmt::Arguments<'_>) {
    let mut o = Out { b: [0; 320], n: 0 };
    let _ = o.write_fmt(args);
    sys_write(&o.b[..o.n]);
}

fn time(name: &str, iters: u32, mut f: impl FnMut()) {
    f();
    let mut best = u64::MAX;
    for _ in 0..iters {
        let t = now_ns();
        f();
        best = best.min(now_ns() - t);
    }
    p(format_args!("{name:<48} best {:>9} us\n", best / 1000));
}

// Linux はスタックを 16 バイト境界で渡すので、関数呼び出し直後の規約 (rsp % 16 == 8) に合わせる。
global_asm!(".globl _start", "_start:", "xor ebp, ebp", "and rsp, -16", "call probe_main", "ud2");

#[unsafe(no_mangle)]
extern "C" fn probe_main() -> ! {
    p(format_args!("yk-gfx on x86_64-unknown-none (soft-float), run natively\n"));
    for (i, data) in PNGS.iter().enumerate() {
        match decode_png(data, &PngLimits::default()) {
            Ok(img) => p(format_args!(
                "png #{i}: {}x{} pixel(5,3)={:08x}\n",
                img.width(),
                img.height(),
                img.get(5, 3).map_or(0, |c| c.0)
            )),
            Err(e) => p(format_args!("png #{i}: error {e}\n")),
        }
    }
    let full = Rect::new(0, 0, 1280, 800);
    let mut img = Image::filled(1280, 800, Color::WHITE).unwrap();
    time("fill_rect opaque 1280x800", 20, || img.canvas().fill_rect(full, Color::rgb(1, 2, 3)));
    time("fill_gradient_v 1280x800", 20, || {
        img.canvas().fill_gradient_v(full, Color::rgb(255, 214, 232), Color::rgb(221, 206, 255))
    });
    time("fill_rect translucent 1280x800", 10, || img.canvas().fill_rect(full, Color::argb(128, 255, 255, 255)));
    let src = Image::filled(1280, 800, Color::argb(200, 10, 20, 30)).unwrap();
    time("blit_alpha 1280x800", 10, || img.canvas().blit_alpha(&src.view(), 0, 0));
    let half = Image::filled(640, 400, Color::rgb(90, 120, 200)).unwrap();
    time("blit_scaled bilinear 640x400 -> 1280x800", 5, || {
        img.canvas().blit_scaled(&half.view(), half.bounds(), full, Filter::Bilinear, BlendMode::Copy, 255)
    });
    time("fill_round_rect 600x400", 20, || {
        img.canvas().fill_round_rect(Rect::new(100, 100, 600, 400), 24, Color::rgb(221, 206, 255))
    });
    time("fill_ellipse 600x400", 20, || img.canvas().fill_ellipse(Rect::new(100, 100, 600, 400), Color::rgb(221, 206, 255)));

    let t = now_ns();
    let font = Font::new(FONT).unwrap();
    p(format_args!("Font::new (3.4 MB font){:>31} us\n", (now_ns() - t) / 1000));
    let text = "吾輩は猫である。名前はまだ無い。どこで生れたかとんと見当がつかぬ。何でも薄暗いじめじめした所でニャーニャー泣いていた事だけは記憶している。吾輩はここで始めて人間というものを見た。The quick brown fox jumps over the lazy dog.";
    let rect = Rect::new(20, 20, 560, 760);
    for size in [14.0f32, 24.0] {
        let st = TextStyle::new(size, Color::BLACK);
        let mut tr = TextRenderer::new();
        let t = now_ns();
        tr.draw_text_box(&mut img.canvas(), &font, &st, rect, text);
        p(format_args!(
            "text {size}px 148 chars, cache COLD ({} misses){:>12} us\n",
            tr.cache_stats().misses,
            (now_ns() - t) / 1000
        ));
        time("   same text, cache WARM", 20, || {
            tr.draw_text_box(&mut img.canvas(), &font, &st, rect, text);
        });
    }
    let mut tr = TextRenderer::new();
    let t = now_ns();
    tr.draw_text_box(&mut img.canvas(), &font, &TextStyle::new(20.0, Color::BLACK), full, KANJI);
    let us = (now_ns() - t) / 1000;
    let misses = tr.cache_stats().misses;
    p(format_args!("text 20px, {misses} distinct kanji, cache COLD {us:>10} us ({} us/glyph)\n", us / misses.max(1)));
    sys_exit(0)
}

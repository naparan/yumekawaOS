//! PNG デコードのテスト (tests/data のフィクスチャ。生成: tests/gen_png_fixtures.py)。

use yk_gfx::png::{PngError, PngLimits, decode_png, png_info};
use yk_gfx::{Color, Image};

const W: u32 = 13;
const H: u32 = 9;

fn data(name: &str) -> Vec<u8> {
    std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/data")
            .join(name),
    )
    .unwrap_or_else(|e| panic!("{name}: {e}"))
}

/// gen_png_fixtures.py の `pat`。
fn pat(x: u32, y: u32) -> (u8, u8, u8, u8) {
    (
        ((x * 20 + y * 3) % 256) as u8,
        ((y * 28 + x) % 256) as u8,
        ((x * 9 + y * 17 + 40) % 256) as u8,
        ((255i32 - x as i32 * 15 - y as i32 * 5).rem_euclid(256)) as u8,
    )
}

fn check(img: &Image, f: impl Fn(u32, u32) -> Color) {
    assert_eq!((img.width(), img.height()), (W, H));
    for y in 0..H {
        for x in 0..W {
            assert_eq!(img.get(x, y).unwrap(), f(x, y), "pixel ({x},{y})");
        }
    }
}

fn dec(name: &str) -> Image {
    decode_png(&data(name), &PngLimits::default()).unwrap_or_else(|e| panic!("{name}: {e}"))
}

#[test]
fn rgb8_plain_and_interlaced() {
    for n in ["rgb8.png", "rgb8_adam7.png"] {
        check(&dec(n), |x, y| {
            let p = pat(x, y);
            Color::rgb(p.0, p.1, p.2)
        });
    }
}

#[test]
fn rgba8_plain_and_interlaced() {
    for n in ["rgba8.png", "rgba8_adam7.png"] {
        check(&dec(n), |x, y| {
            let p = pat(x, y);
            Color::argb(p.3, p.0, p.1, p.2)
        });
    }
}

#[test]
fn sixteen_bit_is_reduced_to_eight() {
    check(&dec("rgb16.png"), |x, y| {
        let p = pat(x, y);
        Color::rgb(p.0, p.1, p.2)
    });
    check(&dec("rgba16.png"), |x, y| {
        let p = pat(x, y);
        Color::argb(p.3, p.0, p.1, p.2)
    });
}

#[test]
fn grayscale_all_depths() {
    for (d, name) in [
        (1u32, "gray1.png"),
        (2, "gray2.png"),
        (4, "gray4.png"),
        (8, "gray8.png"),
        (16, "gray16.png"),
    ] {
        let m = (1u32 << d) - 1;
        check(&dec(name), |x, y| {
            let v = (x * 7 + y * 5) % (m + 1);
            let g = if d == 16 {
                (v >> 8) as u8
            } else {
                (v * 255 / m) as u8
            };
            Color::rgb(g, g, g)
        });
    }
    for (d, name) in [(1u32, "gray1_adam7.png"), (4, "gray4_adam7.png")] {
        let m = (1u32 << d) - 1;
        check(&dec(name), |x, y| {
            let g = (((x * 7 + y * 5) % (m + 1)) * 255 / m) as u8;
            Color::rgb(g, g, g)
        });
    }
}

#[test]
fn gray_alpha() {
    check(&dec("graya8.png"), |x, y| {
        let p = pat(x, y);
        Color::argb(p.3, p.0, p.0, p.0)
    });
}

#[test]
fn palette_images() {
    let pal: Vec<(u8, u8, u8)> = (0..16u32)
        .map(|i| {
            (
                ((i * 53) % 256) as u8,
                ((i * 97 + 30) % 256) as u8,
                ((i * 29 + 100) % 256) as u8,
            )
        })
        .collect();
    check(&dec("pal4.png"), |x, y| {
        let c = pal[((x + y * 3) % 16) as usize];
        Color::rgb(c.0, c.1, c.2)
    });
    check(&dec("pal4_adam7.png"), |x, y| {
        let c = pal[((x + y * 3) % 16) as usize];
        Color::rgb(c.0, c.1, c.2)
    });
    // tRNS: 先頭 5 エントリのみ指定 (残りは不透明)。
    let trns = [0u8, 64, 128, 255, 200];
    check(&dec("pal4_trns.png"), |x, y| {
        let i = ((x + y * 3) % 16) as usize;
        let c = pal[i];
        Color::argb(*trns.get(i).unwrap_or(&255), c.0, c.1, c.2)
    });
    check(&dec("pal8.png"), |x, y| {
        let i = (x * 19 + y * 31) % 256;
        Color::rgb(
            ((i * 5) % 256) as u8,
            ((i * 11) % 256) as u8,
            ((i * 23) % 256) as u8,
        )
    });
    // パレットのエントリ数が 2^depth より少ない画像。
    let pal2 = [(255u8, 0u8, 0u8), (0, 255, 0), (0, 0, 255), (255, 255, 0)];
    check(&dec("pal2_small_plte.png"), |x, y| {
        let c = pal2[((x + y) % 4) as usize];
        Color::rgb(c.0, c.1, c.2)
    });
}

#[test]
fn one_by_one() {
    let img = dec("one_by_one.png");
    assert_eq!((img.width(), img.height()), (1, 1));
    assert_eq!(img.get(0, 0).unwrap(), Color::argb(40, 10, 20, 30));
}

#[test]
fn header_info() {
    let i = png_info(&data("rgba8_adam7.png")).unwrap();
    assert_eq!(
        (i.width, i.height, i.bit_depth, i.color_type, i.interlaced),
        (W, H, 8, 6, true)
    );
    let i = png_info(&data("gray2.png")).unwrap();
    assert_eq!((i.bit_depth, i.color_type, i.interlaced), (2, 0, false));
}

#[test]
fn limits_are_enforced_before_decoding() {
    let d = data("rgb8.png");
    let lim = |w, h, p| PngLimits {
        max_width: w,
        max_height: h,
        max_pixels: p,
        verify_crc: true,
    };
    assert!(matches!(
        decode_png(&d, &lim(12, 100, 1000)),
        Err(PngError::TooLarge {
            width: 13,
            height: 9
        })
    ));
    assert!(matches!(
        decode_png(&d, &lim(100, 8, 1000)),
        Err(PngError::TooLarge { .. })
    ));
    assert!(matches!(
        decode_png(&d, &lim(100, 100, 116)),
        Err(PngError::TooLarge { .. })
    ));
    assert!(decode_png(&d, &lim(13, 9, 117)).is_ok());
}

/// IHDR だけ巨大で本体は無い (decompression bomb 風)。確保せずに拒否する。
#[test]
fn huge_declared_size_is_rejected_without_allocating() {
    let mut d = data("rgb8.png");
    // 幅 = 0x7FFFFFFF, 高さ = 0x7FFFFFFF に書き換え (CRC は無視される)。
    d[16..20].copy_from_slice(&0x7FFF_FFFFu32.to_be_bytes());
    d[20..24].copy_from_slice(&0x7FFF_FFFFu32.to_be_bytes());
    let r = decode_png(&d, &PngLimits::default());
    assert!(matches!(r, Err(PngError::TooLarge { .. })), "{r:?}");
    // 制限を極端に緩めても、Image::MAX_PIXELS で拒否される。
    let lim = PngLimits {
        max_width: u32::MAX,
        max_height: u32::MAX,
        max_pixels: u64::MAX,
        verify_crc: false,
    };
    let r = decode_png(&d, &lim);
    assert!(matches!(r, Err(PngError::TooLarge { .. })), "{r:?}");
    // 幅だけが u32 の範囲外 (> i32::MAX)。
    d[16..20].copy_from_slice(&0xFFFF_FFFFu32.to_be_bytes());
    assert!(matches!(decode_png(&d, &lim), Err(PngError::Malformed(_))));
}

#[test]
fn malformed_headers() {
    assert!(matches!(
        decode_png(&[], &PngLimits::default()),
        Err(PngError::Malformed(_))
    ));
    assert!(matches!(
        decode_png(b"not a png at all, just text...", &PngLimits::default()),
        Err(PngError::Malformed(_))
    ));
    let d = data("rgb8.png");
    for cut in [
        1usize,
        7,
        8,
        12,
        20,
        28,
        29,
        40,
        d.len() / 2,
        d.len() - 13,
        d.len() - 1,
    ] {
        let r = decode_png(&d[..cut], &PngLimits::default());
        assert!(r.is_err(), "truncated at {cut} should fail");
    }
    // 不正なカラータイプ / ビット深度 / インターレース方式。
    for (off, v) in [(24usize, 7u8), (25, 5), (28, 2), (26, 1)] {
        let mut b = d.clone();
        b[off] = v;
        assert!(
            decode_png(&b, &PngLimits::default()).is_err(),
            "IHDR byte {off}={v}"
        );
    }
    // ゼロ幅。
    let mut b = d.clone();
    b[16..20].copy_from_slice(&0u32.to_be_bytes());
    assert!(decode_png(&b, &PngLimits::default()).is_err());
}

/// 小さな IHDR に対して 40 MB に展開される IDAT (展開爆弾) は、巨大な確保をせず Err になる。
#[test]
fn decompression_bomb_is_rejected_quickly() {
    let d = data("bomb_idat.png");
    assert!(
        d.len() < 100_000,
        "フィクスチャは小さいはず ({} バイト)",
        d.len()
    );
    let t = std::time::Instant::now();
    for verify_crc in [true, false] {
        let lim = PngLimits {
            verify_crc,
            ..PngLimits::default()
        };
        assert!(matches!(decode_png(&d, &lim), Err(PngError::Malformed(_))));
    }
    assert!(
        t.elapsed().as_secs() < 5,
        "展開爆弾の拒否に時間がかかりすぎ: {:?}",
        t.elapsed()
    );
}

#[test]
fn crc_corruption_is_detected_when_enabled() {
    let mut d = data("rgba8.png");
    // 最初の IDAT のデータ中の 1 バイトを壊す。
    let idat = d.windows(4).position(|w| w == b"IDAT").unwrap();
    d[idat + 6] ^= 0x55;
    assert!(decode_png(&d, &PngLimits::default()).is_err());
}

/// 壊れた入力のファジング: どの変異でもパニックせず Ok か Err を返す。
#[test]
fn fuzz_mutations_never_panic() {
    let names = [
        "rgb8.png",
        "rgba8_adam7.png",
        "gray4_adam7.png",
        "pal4_trns.png",
        "rgb16.png",
        "gray1.png",
        "pal2_small_plte.png",
        "one_by_one.png",
    ];
    let mut s = 0x9E37_79B9u32;
    let mut next = move || {
        s ^= s << 13;
        s ^= s >> 17;
        s ^= s << 5;
        s
    };
    let lim = PngLimits {
        max_width: 64,
        max_height: 64,
        max_pixels: 4096,
        verify_crc: false,
    };
    let lim_crc = PngLimits {
        verify_crc: true,
        ..lim
    };
    // 既定は 1500 回/ファイル。長めに回すときは YK_FUZZ_ITERS=100000 のように指定する。
    let iters: u32 = std::env::var("YK_FUZZ_ITERS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1500);
    let mut ok = 0u32;
    let mut err = 0u32;
    for n in names {
        let orig = data(n);
        for iter in 0..iters {
            let mut b = orig.clone();
            match iter % 4 {
                0 => {
                    // ランダムなバイトを 1~4 個書き換える
                    for _ in 0..1 + next() % 4 {
                        let i = next() as usize % b.len();
                        b[i] = next() as u8;
                    }
                }
                1 => {
                    // 途中で切る
                    let i = next() as usize % b.len();
                    b.truncate(i);
                }
                2 => {
                    // IHDR の寸法/種別まわりを重点的に壊す
                    for _ in 0..2 {
                        let i = 16 + next() as usize % 13;
                        if i < b.len() {
                            b[i] = next() as u8;
                        }
                    }
                }
                _ => {
                    // 圧縮データ部分を壊す (CRC 無視設定なので展開器に届く)
                    if let Some(p) = b.windows(4).position(|w| w == b"IDAT") {
                        for _ in 0..3 {
                            let i = p + 4 + next() as usize % (b.len() - p - 4);
                            b[i] = next() as u8;
                        }
                    }
                }
            }
            for l in [&lim, &lim_crc] {
                match decode_png(&b, l) {
                    Ok(img) => {
                        ok += 1;
                        assert!(img.width() <= 64 && img.height() <= 64);
                    }
                    Err(_) => err += 1,
                }
            }
        }
    }
    std::println!("png fuzz: ok={ok} err={err}");
    assert!(err > 0);
}

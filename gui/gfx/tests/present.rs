//! バックバッファ → framebuffer 画素形式の変換テスト。

use yk_gfx::present::{Channel, PixelLayout};

#[test]
fn xrgb8888_drops_alpha_only() {
    let l = PixelLayout::XRGB8888;
    assert_eq!(l.encode(0xFF11_2233), 0x0011_2233);
    assert_eq!(l.encode(0x8000_0000), 0);
    let src = [0xFFAA_BBCC, 0x0001_0203, 0x7F00_FF00];
    let mut dst = [0xDEAD_BEEF; 4];
    l.convert_row(&src, &mut dst);
    assert_eq!(dst, [0x00AA_BBCC, 0x0001_0203, 0x0000_FF00, 0xDEAD_BEEF]);
}

#[test]
fn bgr_and_rgb565_layouts() {
    // RGB の並びが逆 (R=0, G=8, B=16)
    let bgr = PixelLayout {
        red: Channel { shift: 0, size: 8 },
        green: Channel { shift: 8, size: 8 },
        blue: Channel { shift: 16, size: 8 },
    };
    assert_eq!(bgr.encode(0xFF11_2233), 0x0033_2211);
    // 565 (16bit を 32bit 値の下位に置く形)
    let l565 = PixelLayout {
        red: Channel { shift: 11, size: 5 },
        green: Channel { shift: 5, size: 6 },
        blue: Channel { shift: 0, size: 5 },
    };
    assert_eq!(l565.encode(0xFFFF_FFFF), 0xFFFF);
    assert_eq!(l565.encode(0xFF00_0000), 0);
    assert_eq!(l565.encode(0xFFFF_0000), 0xF800);
    assert_eq!(l565.encode(0xFF00_FF00), 0x07E0);
    assert_eq!(l565.encode(0xFF00_00FF), 0x001F);
    // 汎用経路と XRGB の高速経路が同じ結果になる。
    let generic = PixelLayout {
        red: Channel { shift: 16, size: 8 },
        green: Channel { shift: 8, size: 8 },
        blue: Channel { shift: 0, size: 8 },
    };
    assert_eq!(generic, PixelLayout::XRGB8888);
}

#[test]
fn ten_bit_channels() {
    // 10bit 幅のチャネル: 255 -> 1023、128 -> 514 (= (128*1023+127)/255)
    let l = PixelLayout {
        red: Channel {
            shift: 20,
            size: 10,
        },
        green: Channel {
            shift: 10,
            size: 10,
        },
        blue: Channel { shift: 0, size: 10 },
    };
    assert_eq!(l.encode(0xFFFF_FFFF), 0x3FFF_FFFF);
    assert_eq!(l.encode(0xFF80_0000), 514 << 20);
}

#[test]
fn absurd_layouts_do_not_panic() {
    let weird = PixelLayout {
        red: Channel {
            shift: 255,
            size: 255,
        },
        green: Channel { shift: 31, size: 0 },
        blue: Channel { shift: 40, size: 3 },
    };
    for v in [0u32, 0xFFFF_FFFF, 0x1234_5678] {
        let _ = weird.encode(v);
    }
    let mut dst = [0u32; 2];
    weird.convert_row(&[1, 2, 3], &mut dst);
    weird.convert_row(&[], &mut dst);
}

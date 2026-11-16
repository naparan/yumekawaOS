#!/usr/bin/env python3
"""PNG テスト用フィクスチャの生成 (標準ライブラリの zlib のみ使用)。

`python3 -I gui/gfx/tests/gen_png_fixtures.py gui/gfx/tests/data` で再生成できる。
画像は 13x9 (8 の倍数でないので Adam7 の端のパスも通る)。行ごとにフィルタ種別 0..4 を
順番に使い、アンアンフィルタの全経路を通す。期待値は tests/png.rs 側で同じ式から計算する。
"""
import struct, sys, zlib, os

W, H = 13, 9


def chunk(t, d):
    c = struct.pack(">I", len(d)) + t + d
    return c + struct.pack(">I", zlib.crc32(t + d) & 0xFFFFFFFF)


def paeth(a, b, c):
    p = a + b - c
    pa, pb, pc = abs(p - a), abs(p - b), abs(p - c)
    if pa <= pb and pa <= pc:
        return a
    return b if pb <= pc else c


def filter_row(ft, row, prev, bpp):
    out = bytearray([ft])
    for i, x in enumerate(row):
        a = row[i - bpp] if i >= bpp else 0
        b = prev[i]
        c = prev[i - bpp] if i >= bpp else 0
        if ft == 0:
            v = x
        elif ft == 1:
            v = x - a
        elif ft == 2:
            v = x - b
        elif ft == 3:
            v = x - ((a + b) >> 1)
        else:
            v = x - paeth(a, b, c)
        out.append(v & 0xFF)
    return bytes(out)


def pack_row(samples, depth):
    if depth == 8:
        return bytes(samples)
    if depth == 16:
        return b"".join(struct.pack(">H", s) for s in samples)
    bits = "".join(format(s, "0%db" % depth) for s in samples)
    bits += "0" * (-len(bits) % 8)
    return bytes(int(bits[i:i + 8], 2) for i in range(0, len(bits), 8))


def encode_pass(img, nch, depth, ys, xs, rowno0=0):
    """img[y][x] = tuple of samples。ys/xs はこのパスで使う座標のリスト。"""
    if not ys or not xs:
        return b""
    bpp = max(1, (nch * depth) // 8)
    prev = None
    raw = b""
    for n, y in enumerate(ys):
        samples = [s for x in xs for s in img[y][x]]
        row = pack_row(samples, depth)
        if prev is None:
            prev = bytes(len(row))
        raw += filter_row((n + rowno0) % 5, row, prev, bpp)
        prev = row
    return raw


ADAM7 = [(0, 0, 8, 8), (4, 0, 8, 8), (0, 4, 4, 8), (2, 0, 4, 4), (0, 2, 2, 4), (1, 0, 2, 2), (0, 1, 1, 2)]


def make_png(img, ctype, depth, interlace=False, plte=None, trns=None):
    W, H = len(img[0]), len(img)
    nch = {0: 1, 2: 3, 3: 1, 4: 2, 6: 4}[ctype]
    ihdr = struct.pack(">IIBBBBB", W, H, depth, ctype, 0, 0, 1 if interlace else 0)
    if interlace:
        raw = b""
        for (x0, y0, dx, dy) in ADAM7:
            raw += encode_pass(img, nch, depth, list(range(y0, H, dy)), list(range(x0, W, dx)))
    else:
        raw = encode_pass(img, nch, depth, list(range(H)), list(range(W)))
    out = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", ihdr)
    if plte:
        out += chunk(b"PLTE", bytes(c for rgb in plte for c in rgb))
    if trns is not None:
        out += chunk(b"tRNS", trns)
    # IDAT を 2 つに分割して複数チャンクの結合も試す。
    z = zlib.compress(raw, 9)
    h = len(z) // 2
    out += chunk(b"IDAT", z[:h]) + chunk(b"IDAT", z[h:]) + chunk(b"IEND", b"")
    return out


def pat(x, y):
    return ((x * 20 + y * 3) % 256, (y * 28 + x) % 256, (x * 9 + y * 17 + 40) % 256, (255 - x * 15 - y * 5) % 256)


def main(outdir):
    os.makedirs(outdir, exist_ok=True)

    def w(name, data):
        open(os.path.join(outdir, name), "wb").write(data)

    rgb = [[pat(x, y)[:3] for x in range(W)] for y in range(H)]
    rgba = [[pat(x, y) for x in range(W)] for y in range(H)]
    w("rgb8.png", make_png(rgb, 2, 8))
    w("rgb8_adam7.png", make_png(rgb, 2, 8, interlace=True))
    w("rgba8.png", make_png(rgba, 6, 8))
    w("rgba8_adam7.png", make_png(rgba, 6, 8, interlace=True))
    rgb16 = [[tuple(v * 257 for v in pat(x, y)[:3]) for x in range(W)] for y in range(H)]
    w("rgb16.png", make_png(rgb16, 2, 16))
    rgba16 = [[tuple(v * 257 for v in pat(x, y)) for x in range(W)] for y in range(H)]
    w("rgba16.png", make_png(rgba16, 6, 16))
    for d in (1, 2, 4, 8, 16):
        m = (1 << d) - 1
        g = [[((x * 7 + y * 5) % (m + 1),) for x in range(W)] for y in range(H)]
        w("gray%d.png" % d, make_png(g, 0, d))
        if d in (1, 4):
            w("gray%d_adam7.png" % d, make_png(g, 0, d, interlace=True))
    ga = [[(pat(x, y)[0], pat(x, y)[3]) for x in range(W)] for y in range(H)]
    w("graya8.png", make_png(ga, 4, 8))
    # パレット (tRNS つき / なし)
    pal = [((i * 53) % 256, (i * 97 + 30) % 256, (i * 29 + 100) % 256) for i in range(16)]
    pi = [[((x + y * 3) % 16,) for x in range(W)] for y in range(H)]
    w("pal4.png", make_png(pi, 3, 4, plte=pal))
    w("pal4_trns.png", make_png(pi, 3, 4, plte=pal, trns=bytes([0, 64, 128, 255, 200])))
    w("pal4_adam7.png", make_png(pi, 3, 4, interlace=True, plte=pal))
    pal256 = [((i * 5) % 256, (i * 11) % 256, (i * 23) % 256) for i in range(256)]
    p8 = [[((x * 19 + y * 31) % 256,) for x in range(W)] for y in range(H)]
    w("pal8.png", make_png(p8, 3, 8, plte=pal256))
    pal2 = [(255, 0, 0), (0, 255, 0), (0, 0, 255), (255, 255, 0)]
    p2 = [[((x + y) % 4,) for x in range(W)] for y in range(H)]
    w("pal2_small_plte.png", make_png(p2, 3, 2, plte=pal2))
    # 展開爆弾: IHDR は 13x9 RGB (展開後 366 バイト) だが、IDAT の zlib は 40 MB のゼロに展開される。
    # デコーダは期待サイズを超えた時点で止めなければならない。
    z = zlib.compress(b"\0" * 40_000_000, 9)
    ihdr = struct.pack(">IIBBBBB", 13, 9, 8, 2, 0, 0, 0)
    w("bomb_idat.png", b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", ihdr) + chunk(b"IDAT", z) + chunk(b"IEND", b""))
    # 1x1
    w("one_by_one.png", make_png([[(10, 20, 30, 40)]], 6, 8))


if __name__ == "__main__":
    main(sys.argv[1] if len(sys.argv) > 1 else "data")

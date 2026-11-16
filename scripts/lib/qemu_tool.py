#!/usr/bin/env python3
"""yumekawaOS テスト用の補助ツール (Python 標準ライブラリのみ使用)。

サブコマンド:
  monitor <socket> <command>     QEMU HMP monitor (UNIX ソケット) にコマンドを送る
  ppm2png <in.ppm> <out.png>     QEMU screendump の PPM (P6) を PNG に変換する
  check-image <in.ppm>           画面が単色でないことなどを機械的に検査する
"""

import socket
import struct
import sys
import time
import zlib


def monitor(sock_path: str, command: str, timeout: float = 10.0) -> str:
    s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    s.settimeout(timeout)
    s.connect(sock_path)
    buf = b""
    deadline = time.time() + timeout
    # 最初のプロンプト "(qemu) " を待つ
    while b"(qemu)" not in buf and time.time() < deadline:
        buf += s.recv(4096)
    s.sendall(command.encode() + b"\n")
    out = b""
    # コマンドのエコーの後、次のプロンプトが来るまで読む
    while time.time() < deadline:
        try:
            chunk = s.recv(4096)
        except socket.timeout:
            break
        if not chunk:
            break
        out += chunk
        if out.count(b"(qemu)") >= 1 and out.rstrip().endswith(b"(qemu)"):
            break
    s.close()
    return out.decode(errors="replace")


def read_ppm(path: str):
    with open(path, "rb") as f:
        data = f.read()
    # ヘッダ: P6 <ws> width <ws> height <ws> maxval <single ws> raster
    fields = []
    pos = 0
    while len(fields) < 4:
        while data[pos:pos + 1].isspace():
            pos += 1
        if data[pos:pos + 1] == b"#":
            while data[pos:pos + 1] not in (b"\n", b""):
                pos += 1
            continue
        start = pos
        while not data[pos:pos + 1].isspace():
            pos += 1
        fields.append(data[start:pos])
    pos += 1
    if fields[0] != b"P6":
        raise ValueError(f"unsupported PPM magic: {fields[0]!r}")
    width, height, maxval = int(fields[1]), int(fields[2]), int(fields[3])
    if maxval != 255:
        raise ValueError(f"unsupported PPM maxval: {maxval}")
    raster = data[pos:pos + width * height * 3]
    if len(raster) != width * height * 3:
        raise ValueError("truncated PPM")
    return width, height, raster


def write_png(path: str, width: int, height: int, raster: bytes) -> None:
    def chunk(kind: bytes, payload: bytes) -> bytes:
        return (struct.pack(">I", len(payload)) + kind + payload
                + struct.pack(">I", zlib.crc32(kind + payload) & 0xFFFFFFFF))

    stride = width * 3
    rows = b"".join(b"\x00" + raster[y * stride:(y + 1) * stride] for y in range(height))
    png = (b"\x89PNG\r\n\x1a\n"
           + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0))
           + chunk(b"IDAT", zlib.compress(rows, 9))
           + chunk(b"IEND", b""))
    with open(path, "wb") as f:
        f.write(png)


def check_image(path: str) -> int:
    width, height, raster = read_ppm(path)
    total = width * height
    counts = {}
    for i in range(0, len(raster), 3):
        px = raster[i:i + 3]
        counts[px] = counts.get(px, 0) + 1
    distinct = len(counts)
    dominant_px, dominant = max(counts.items(), key=lambda kv: kv[1])
    dominant_ratio = dominant / total

    def at(x, y):
        i = (y * width + x) * 3
        return tuple(raster[i:i + 3])

    top, bottom = at(width // 2, 1), at(width // 2, height - 2)
    print(f"image: {width}x{height}, distinct colors: {distinct}, "
          f"dominant {tuple(dominant_px)} = {dominant_ratio:.1%}, top-center {top}, bottom-center {bottom}")
    failures = []
    if distinct < 16:
        failures.append(f"too few distinct colors ({distinct} < 16): screen looks blank/solid")
    if dominant_ratio > 0.5:
        failures.append(f"one color covers {dominant_ratio:.1%} of the screen (> 50%)")
    if top == bottom:
        failures.append("top and bottom rows have the same color: background gradient missing")
    for f in failures:
        print(f"FAIL: {f}")
    return 1 if failures else 0


def main(argv) -> int:
    if len(argv) < 2:
        print(__doc__)
        return 2
    cmd = argv[1]
    if cmd == "monitor" and len(argv) == 4:
        print(monitor(argv[2], argv[3]))
        return 0
    if cmd == "ppm2png" and len(argv) == 4:
        w, h, raster = read_ppm(argv[2])
        write_png(argv[3], w, h, raster)
        return 0
    if cmd == "check-image" and len(argv) == 3:
        return check_image(argv[2])
    print(__doc__)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv))

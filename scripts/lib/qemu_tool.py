#!/usr/bin/env python3
"""yumekawaOS テスト用の補助ツール (Python 標準ライブラリのみ使用)。

サブコマンド:
  monitor <socket> <command>     QEMU HMP monitor (UNIX ソケット) にコマンドを送る
  ppm2png <in.ppm> <out.png>     QEMU screendump の PPM (P6) を PNG に変換する
  check-image <in.ppm>           画面が単色でないことなどを機械的に検査する (スプラッシュ)
  check-panic-screen <in.ppm>    Kernel Panic 画面 (背景色・上部の帯・文字) を機械的に検査する
  sendkeys <socket> <key>...     QEMU monitor の sendkey でキーを順に注入する
  timer-watch <serial> <timeout> シリアルログを追い、TIMER_MARK start/end が出たホスト時刻の差と
                                 カーネルが報告した経過時間 (elapsed_ms) を比較する
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


# kernel/src/graphics/panic_screen.rs の配色と一致させる
PANIC_BACKGROUND = (48, 24, 64)
PANIC_HEADER = (255, 105, 180)


def check_panic_screen(path: str) -> int:
    width, height, raster = read_ppm(path)

    def at(x, y):
        i = (y * width + x) * 3
        return tuple(raster[i:i + 3])

    header_h = max(height // 8, 24)
    header_px = header_total = 0
    header_text = 0
    for y in range(0, header_h, 2):
        for x in range(0, width, 2):
            c = at(x, y)
            header_total += 1
            if c == PANIC_HEADER:
                header_px += 1
            elif c == (255, 255, 255):
                header_text += 1
    body_bg = body_total = body_text = 0
    for y in range(header_h, height, 2):
        for x in range(0, width, 2):
            c = at(x, y)
            body_total += 1
            if c == PANIC_BACKGROUND:
                body_bg += 1
            else:
                body_text += 1
    header_ratio = header_px / header_total
    header_text_ratio = header_text / header_total
    bg_ratio = body_bg / body_total
    text_ratio = body_text / body_total
    print(f"panic screen: {width}x{height}, header color {header_ratio:.1%} of top band "
          f"(white text {header_text_ratio:.2%}), background {bg_ratio:.1%} of body, "
          f"non-background (text) {text_ratio:.2%}")
    failures = []
    if header_ratio < 0.6:
        failures.append(f"top band is not the panic header color ({header_ratio:.1%} < 60%)")
    if header_text_ratio < 0.003:
        failures.append("no title text in the header band")
    if bg_ratio < 0.6:
        failures.append(f"body is not the panic background ({bg_ratio:.1%} < 60%)")
    if text_ratio < 0.005:
        failures.append(f"almost no text in the body ({text_ratio:.2%})")
    for f in failures:
        print(f"FAIL: {f}")
    return 1 if failures else 0


def sendkeys(sock_path: str, keys) -> int:
    for key in keys:
        monitor(sock_path, f"sendkey {key}")
        time.sleep(0.3)
    return 0


def timer_watch(serial_path: str, timeout: float, tolerance_pct: float = 15.0,
                tolerance_ms: float = 250.0) -> int:
    """TIMER_MARK start/end の出現時刻 (ホストの単調時計) を記録して比較する。"""
    deadline = time.monotonic() + timeout
    start_t = end_t = None
    kernel_ms = None
    pos = 0
    buf = b""
    while time.monotonic() < deadline and end_t is None:
        try:
            with open(serial_path, "rb") as f:
                f.seek(pos)
                chunk = f.read()
        except FileNotFoundError:
            chunk = b""
        now = time.monotonic()
        if chunk:
            pos += len(chunk)
            buf += chunk
            while b"\n" in buf:
                line, buf = buf.split(b"\n", 1)
                text = line.decode(errors="replace")
                if "TIMER_MARK start" in text and start_t is None:
                    start_t = now
                elif "TIMER_MARK end" in text and start_t is not None:
                    end_t = now
                    for tok in text.split():
                        if tok.startswith("elapsed_ms="):
                            kernel_ms = int(tok.split("=", 1)[1])
        time.sleep(0.01)
    if start_t is None or end_t is None or kernel_ms is None:
        print("FAIL: TIMER_MARK start/end were not observed")
        return 1
    host_ms = (end_t - start_t) * 1000.0
    diff = abs(host_ms - kernel_ms)
    limit = host_ms * tolerance_pct / 100.0 + tolerance_ms
    print(f"timer: kernel uptime advanced {kernel_ms} ms, host wall clock {host_ms:.0f} ms "
          f"(diff {diff:.0f} ms, limit {limit:.0f} ms = {tolerance_pct:.0f}% + {tolerance_ms:.0f} ms)")
    if diff > limit:
        print("FAIL: kernel uptime deviates too much from the host clock")
        return 1
    return 0


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
    if cmd == "check-panic-screen" and len(argv) == 3:
        return check_panic_screen(argv[2])
    if cmd == "sendkeys" and len(argv) >= 4:
        return sendkeys(argv[2], argv[3:])
    if cmd == "timer-watch" and len(argv) == 4:
        return timer_watch(argv[2], float(argv[3]))
    print(__doc__)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv))

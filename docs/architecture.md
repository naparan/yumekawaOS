# yumekawaOS アーキテクチャ (Phase 1 時点)

本書は Phase 1 (Boot) 時点で**実装済みのもの**だけを記述する。将来の構成は
[requirements.md](requirements.md) §7, §33 を参照。

## 1. ブートフロー

```text
[UEFI ファームウェア (OVMF / 実機)]
   │  CD-ROM: El Torito の EFI ブートイメージ
   │  USB/HDD: GPT の EFI System Partition (同じ FAT イメージ)
   ▼
[Limine 11.4.1  EFI/BOOT/BOOTX64.EFI]
   │  1. limine.conf を探す (ISO9660 上の /boot/limine/limine.conf)
   │  2. /boot/kernel (ELF64) の PT_LOAD を物理メモリへ読み込み、
   │     0xffffffff80000000 からマップ (PHDRS の権限どおり)
   │  3. HHDM・framebuffer (GOP) を用意し、.limine_requests の各リクエストに応答
   │  4. UEFI Boot Services を終了し、Long Mode・割り込み無効で kmain へジャンプ
   ▼
[kmain  (kernel/src/main.rs)]
   1. log::init()           COM1 16550 を初期化 (ループバック試験)。失敗しても続行
   2. boot::take()          base revision 検証 → BootInfo (safe) へ変換。1 度だけ成功
   3. cmdline 解釈           loglevel= / selftest=panic
   4. ブート情報・CPUID をログ出力
   5. Framebuffer::new()    bpp=32・RGB・pitch・マスクを検証 (不正なら FATAL で停止)
   6. splash::draw()        スプラッシュ画面を描画
   7. "YUMEKAWAOS_BOOT_OK phase=1" をログ出力 (テストの起動完了マーカー)
   8. halt_forever()        cli; hlt のループ
```

失敗時の扱い:

| 状況 | 動作 |
| --- | --- |
| COM1 が無い (実機) | ログは破棄。画面表示は継続 |
| Limine が base revision 3 を拒否 / HHDM・メモリマップ応答なし | `[FATAL]` を出して停止 |
| framebuffer が無い / 32bpp 以外 / RGB 以外 / pitch・マスク不正 | `[FATAL] framebuffer is not usable: ...` を出して停止 |
| パニック | 割り込み無効化 → `[FATAL] KERNEL PANIC at <file>:<line>:<col>: <msg>` → 停止 |

## 2. ISO の構成 (scripts/make_iso.sh)

```text
yumekawaOS.iso (ISO9660 + Rock Ridge + Joliet + HFS+)
├── boot/
│   ├── kernel                      カーネル ELF
│   └── limine/
│       ├── limine.conf
│       ├── limine-uefi-cd.bin      FAT イメージ (中に EFI/BOOT/BOOTX64.EFI)
│       └── LICENSE.limine
└── EFI/BOOT/BOOTX64.EFI
```

xorriso の `--efi-boot ... -efi-boot-part --efi-boot-image --protective-msdos-label`
により、`limine-uefi-cd.bin` が

- El Torito の EFI ブートイメージ (光学ドライブとして起動する場合)
- GPT の EFI System Partition (ISO のバイト列を USB メモリ等へそのまま書き込んだ場合)

の両方として参照される。これにより同じ ISO が CD-ROM と USB メモリの両方で UEFI 起動できる
(scripts/test-boot.sh で両方を検証している)。BIOS (Legacy) 起動は §4.2 により対象外で、
BIOS 用のブートレコードは含めていない。

## 3. メモリレイアウト

### 3.1 仮想アドレス空間 (Limine base revision 3 が構築するページテーブル)

| 範囲 | 内容 |
| --- | --- |
| `0xffffffff80000000` 〜 | カーネルイメージ (linker.ld)。下表のセグメント |
| `hhdm_offset` 〜 (QEMU では `0xffff800000000000`) | HHDM: 物理アドレス `p` を `p + hhdm_offset` でアクセス |
| 下位半分 | Phase 1 では使用しない |

カーネルセグメント (`readelf -l build/kernel` の例、release ビルド):

| セグメント | セクション | 権限 | 用途 |
| --- | --- | --- | --- |
| 1 | `.limine_requests` | RW | Limine リクエスト (開始/終了マーカーの間)。Limine が応答ポインタを書き込む |
| 2 | `.text` | R-X | コード |
| 3 | `.rodata` | R-- | 定数 (フォント等) |
| 4 | `.data` `.bss` | RW- | 可変データ (ロガー状態等) |

- `relocation-model=static` + `code-model=kernel` (ターゲット既定) で非 PIE の ELF (ET_EXEC) を生成する。
- 各セグメントはページ境界 (4KiB) に整列。
- スタック: Limine が用意する (StackSizeRequest で 256KiB を要求)。
  bootloader-reclaimable 領域にある。
- HHDM の注意 (base revision 3): Limine 11.4.1 のソース (`common/protos/limine.c`) によると、
  HHDM がマップするのは usable / bootloader-reclaimable / executable-and-modules /
  framebuffer (/ EFI reclaimable) の領域だけで、**ACPI (reclaimable / NVS)・予約領域・
  MMIO はマップされない**。Phase 2 以降で ACPI テーブルや MMIO (LAPIC, IOAPIC 等) に
  アクセスする場合は自前でマップすること。

### 3.2 物理メモリマップ

Limine のメモリマップ (`BootInfo::memory_map`) をそのまま使う。QEMU (`-m 512M`, q35, OVMF) での例:

```text
memory map: 36 regions
  [0x000000001000 - 0x0000000a0000)  636 KiB  usable
  [0x000000100000 - 0x000000800000) 7168 KiB  usable
  ...
  [0x000019ef3000 - 0x000019f02000)   60 KiB  kernel-and-modules
  ...
  [0x000080000000 - 0x0000803e8000) 4000 KiB  framebuffer
memory: usable 463 MiB, bootloader-reclaimable 42 MiB, kernel+modules 60 KiB
```

Phase 1 ではメモリを一切確保しない(ヒープ無し)。文字列整形は固定長の
`fmtbuf::StackString` で行う。

## 4. モジュール境界

```text
main.rs (kmain)
  ├── log          ── drivers::serial ── arch::x86_64::port
  ├── boot         ── (limine クレート)            ※ unsafe: ブートローダ応答
  ├── graphics
  │     ├── framebuffer   ← boot::FramebufferInfo を消費  ※ unsafe: volatile 書き込み
  │     ├── font          (font8x8 クレート)
  │     ├── color
  │     └── splash        (safe のみ)
  ├── panic        ── drivers::serial::emergency_write, arch
  ├── sync         (SpinLock)                      ※ unsafe: UnsafeCell
  └── arch::x86_64 (cpu: cli/hlt/cpuid, port: in/out) ※ unsafe: CPU 命令
```

| モジュール | 責務 | 公開 API (主なもの) |
| --- | --- | --- |
| `arch::x86_64::cpu` | CPU 制御命令 | `halt_forever() -> !`, `disable_interrupts()`, `interrupts_enabled()`, `CpuIdentity::read()` |
| `arch::x86_64::port` | Port I/O | `unsafe Port::new(port)`, `Port::read/write` |
| `boot` | ブートローダとの境界 | `take() -> Result<BootInfo, BootError>`, `BootInfo`, `MemoryMap`, `MemoryRegion`, `MemoryKind`, `FramebufferInfo` |
| `drivers::serial` | COM1 16550 | `init_com1()`, `SerialPort` (`fmt::Write`), `emergency_write()` |
| `log` | ログレベル付き出力 | `trace!`〜`fatal!` マクロ、`Level`, `set_min_level()` |
| `graphics::framebuffer` | 32bpp 線形 FB への安全な書き込み | `Framebuffer::new(FramebufferInfo)`, `fill_rect`, `fill_rect_with`, `fill_rounded_rect_with`, `fill_circle_with` |
| `graphics::font` | 8x8 ビットマップ文字 | `draw_text`, `draw_text_shaded`, `text_width` |
| `graphics::splash` | 起動画面 | `draw(&mut Framebuffer, &SplashText)` |
| `sync` | 同期 | `SpinLock<T>` |

### 4.1 unsafe の方針と所在 (§32)

`unsafe` は下表の低レイヤーに限定し、各ブロックに `// SAFETY:` コメントを付ける
(`clippy::undocumented_unsafe_blocks = deny`, `unsafe_op_in_unsafe_fn = deny`)。
上位層 (`main.rs` のロジック, `graphics::splash`, `graphics::font`, `log`) は safe Rust のみ。

| 場所 | 内容 | 理由 |
| --- | --- | --- |
| `arch/x86_64/port.rs` | `in`/`out` 命令 (inline asm)、`unsafe fn Port::new` | Port I/O。生成時に排他所有を契約とし、読み書きは safe にする |
| `arch/x86_64/cpu.rs` | `cli`, `cli; hlt`, `pushfq; pop` (inline asm) | CPU 命令 |
| `boot/protocol.rs` | `#[unsafe(link_section)]` ×12、`unsafe fn FramebufferInfo::from_bootloader` とその呼び出し | Limine リクエストの配置と、ブートローダが渡す framebuffer アドレスを「書き込み可能な一意の領域」として扱う |
| `drivers/serial.rs` | `unsafe fn SerialPort::new`、`Port::new` 呼び出し、COM1 の生成 ×2 (通常・パニック時) | UART の I/O ポートの所有権 |
| `graphics/framebuffer.rs` | ポインタ加算と `write_volatile` | framebuffer メモリへの書き込み (範囲・整列は `Framebuffer::new` で検証) |
| `sync/spinlock.rs` | `unsafe impl Sync`, `UnsafeCell` の参照化 ×2 | ロックによる排他 |
| `main.rs` | `#[unsafe(no_mangle)]` | エントリポイントのシンボル名固定 |

## 5. ログ (§38)

書式: `[LEVEL] module: message` (例: `[ INFO] kernel: bootloader: Limine 11.4.1`)。

- 既定の最小レベル: debug ビルド = TRACE、release ビルド = DEBUG
- `boot/limine.conf` の `cmdline: loglevel=info` 等で変更可能
- パニック時はロックを経由せず COM1 へ直接出力する (`drivers::serial::emergency_write`)

## 6. Phase 2 への引き継ぎ

- **ブート情報**: `boot::take()` は 1 回だけ成功する。`kmain` が受け取った `BootInfo` を
  メモリ管理の初期化関数へ渡す構成にすること(`static` に置き直さない)。
- **メモリマップ**: `BootInfo::memory_map` (`MemoryMap`, `Copy`) の `iter()` が
  `MemoryRegion { base, length, kind }` を返す。ベース昇順・重複なし。実体は
  bootloader-reclaimable 領域にあるため、**その領域をフレームアロケータへ返す前に
  必要な情報をコピーすること**。Limine のスタックやページテーブルも同領域にある。
- **HHDM**: `BootInfo::hhdm_offset`。物理→仮想変換はこれを使う(定数で埋め込まない)。
  base revision 3 では MMIO 等はマップされない点に注意 (§3.1)。
- **カーネル位置**: `BootInfo::kernel_address` (物理/仮想ベース)。リンカスクリプトのシンボル
  `__kernel_start` / `__kernel_end` も利用できる。
- **ACPI**: `BootInfo::rsdp_address` は base revision 3 では**物理アドレス**
  (QEMU 例: `0x1f77e014`)。ACPI 領域は HHDM にマップされないため、参照前にマップが必要。
- **Limine base revision**: limine クレート 0.5.0 の上限に合わせて 3 を要求している。
  Limine 11.x は 0〜6 をサポートし、revision 4 以降は HHDM が予約領域以外の全領域を
  マップする等の違いがある。上げる場合は stable で使えるバインディング
  (limine クレート 0.6.x は nightly 機能を要求) を用意してから行うこと。
- **割り込み**: Phase 1 は IDT 未設定・割り込み無効のまま。`log` の `SpinLock` は割り込みを
  無効化しないため、割り込みハンドラからログを出す場合は割り込み無効化付きのロックにするか、
  ハンドラ内では `emergency_write` 相当の経路を使うこと。
- **framebuffer**: `graphics::Framebuffer` は `FramebufferInfo` を消費して作られ、カーネル内で
  一意。Phase 3 の描画基盤はこれを所有する形で拡張する。
- **SSE**: `x86_64-unknown-none` は soft-float (SSE 無効) が既定。浮動小数点演算は
  ソフトウェア実装になるため、描画は整数演算で行っている。

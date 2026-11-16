# 外部依存 (§43.4)

外部ライブラリ・外部バイナリを追加/更新したときは、必ずこの表を更新すること。
直接依存のクレートはルートの `Cargo.toml` の `[workspace.dependencies]` で `=` 指定により完全固定し、
推移的依存も含めて `Cargo.lock` で固定している (`scripts/build.sh` は `--locked` でビルドする)。

## 1. カーネルにリンクされるもの

| 名前 | バージョン | 用途 | `no_std` 対応 | ライセンス | OS 環境との互換性 |
| --- | --- | --- | --- | --- | --- |
| [limine](https://crates.io/crates/limine) (クレート) | 0.5.0 (`=0.5.0`) | Limine boot protocol のリクエスト/応答構造体の Rust バインディング (`kernel/src/boot/protocol.rs` のみで使用) | 対応 (`#![no_std]`、アロケータ不要) | MIT OR Apache-2.0 | stable Rust で利用可能。base revision 3 まで対応 (Limine 11.4.1 は 0〜6 をサポートし、3 を受理することを起動テストで確認)。後継の 0.6.x は `#![feature(ptr_metadata)]` を使うため nightly 必須で、stable 前提の本プロジェクトでは採用しない |
| [bitflags](https://crates.io/crates/bitflags) | 2.13.2 (Cargo.lock) | limine クレートの推移的依存 (フラグ型の定義) | 対応 | MIT OR Apache-2.0 | 純粋な Rust、環境依存なし |
| [font8x8](https://crates.io/crates/font8x8) | 0.3.1 (`=0.3.1`, `default-features = false`) | Phase 1 スプラッシュの文字描画用 8x8 ビットマップフォント (`legacy::BASIC_LEGACY` の ASCII 部分のみ使用) | 対応 (`std` / `unicode` feature を無効化して使用) | MIT (グリフデータ自体はパブリックドメイン) | 定数テーブルのみ、環境依存なし。Phase 3 のフォントレンダラで置き換え予定 |
| [linked_list_allocator](https://crates.io/crates/linked_list_allocator) | 0.10.6 (`=0.10.6`, `default-features = false`) | カーネルヒープの空きリスト管理 (first-fit, 解放時に隣接ブロックを結合, `Heap::extend` によるヒープ拡張)。`kernel/src/memory/heap.rs` のみで使用し、外側をカーネルの `IrqSpinLock` で保護する | 対応 (`#![no_std]`)。既定の `use_spin` feature (spinning_top によるロック) は無効化しており、推移的依存は無い | MIT OR Apache-2.0 (Cargo.toml の表記は `Apache-2.0/MIT`) | 純粋な Rust、stable で利用可能 (rust-version 1.61)。メモリ領域は呼び出し側が与える方式でページング・アーキテクチャに依存しない。0.10.2 未満にあった脆弱性 (RUSTSEC-2022-0063) は修正済みの版 |
| Rust 標準ライブラリ `core` / `alloc` / `compiler_builtins` | Rust 1.97.0 | 言語ランタイム (`alloc` は Phase 2 のカーネルヒープで使用) | — (`x86_64-unknown-none` 用) | MIT OR Apache-2.0 (compiler_builtins は一部 Apache-2.0 WITH LLVM-exception) | 通常は rustup の公式配布物、サンドボックスでは同一コミットのソースからビルド (docs/build.md §3)。`compiler-builtins-mem` で `memcpy` 等を提供 |

## 2. ISO に同梱されるもの

| 名前 | バージョン | 用途 | `no_std` 対応 | ライセンス | OS 環境との互換性 |
| --- | --- | --- | --- | --- | --- |
| [Limine](https://github.com/limine-bootloader/limine) | 11.4.1 (タグ `v11.4.1-binary`, コミット `5be26a73d7b7b4d4477d18be94e1d16e615adf56`) | UEFI ブートローダ (`BOOTX64.EFI`, `limine-uefi-cd.bin`) | — (カーネルとはリンクしない外部バイナリ) | BSD-2-Clause (ISO 内 `boot/limine/LICENSE.limine`)。Limine のソース付属 `3RDPARTY.md` によると、バイナリには cc-runtime (Apache-2.0 WITH LLVM-exception)、PicoEFI (BSD-2-Clause, BSD-2-Clause-Patent, BSD-3-Clause, MIT 等)、Limine Boot Protocol ヘッダ (0BSD) 等が含まれる | x86_64 UEFI。Limine boot protocol でカーネルを higher-half にロード。リポジトリにはコミットせず `scripts/fetch-limine.sh` が取得・検証 (sha256 も固定) |

### Limine のバージョン選定

- 2026-11-16 時点で `git ls-remote` により確認した最新の `*-binary` タグは `v11.4.1-binary`
  (`v11.x-binary` ブランチの先頭と同一コミット)。
- ソースタグとしては `v12.x` (最新 `v12.10.0`) も存在するが、`v12` 系には `*-binary` タグ/ブランチが無い。
  v12 を使うにはソースからのビルド (追加のブートストラップ依存の取得) か GitHub Releases の
  配布物が必要になる。本サンドボックスでは github.com の Web/Releases への curl が 403 で
  拒否されるため取得を試みておらず、方針どおり binary タグの最新安定版 11.4.1 を採用した。

## 3. ビルド・テストにのみ使うツール (成果物には含まれない)

| 名前 | 確認したバージョン | 用途 | ライセンス |
| --- | --- | --- | --- |
| Rust ツールチェーン (rustc, cargo, clippy, rustfmt, rust-lld) | 1.97.0 | ビルド・リンク・静的検査 | MIT OR Apache-2.0 |
| xorriso | 1.5.6 | ISO 生成 | GPL-3.0-or-later |
| QEMU | 8.2.2 | 実行・自動テスト | GPL-2.0 |
| OVMF (EDK II) | 2024.02 | QEMU 用 UEFI ファームウェア | BSD-2-Clause-Patent |
| git | 2.43.0 | Limine / rust-lang/rust ソースの取得 | GPL-2.0 |
| Python 3 (標準ライブラリのみ) | 3.13 | QEMU monitor 操作、PPM→PNG 変換、画面検査 | PSF-2.0 |

## 4. Phase 3a (2D 描画ライブラリ `yk-gfx`) で追加したもの

`gui/gfx` (`yk-gfx`) が使う。いずれも `Cargo.toml` の `[workspace.dependencies]` で `=x.y.z` に固定し、`Cargo.lock` をコミットしている。
詳細な設計は [graphics.md](graphics.md)。

### 4.1 yk-gfx にリンクされるクレート (カーネルにリンクされる可能性があるもの)

| 名前 | バージョン | 用途 | `no_std` 対応 | ライセンス | OS 環境との互換性 |
| --- | --- | --- | --- | --- | --- |
| [ab_glyph](https://crates.io/crates/ab_glyph) | 0.2.32 (`=0.2.32`, `default-features = false`, `features = ["libm"]`) | アウトラインフォント (TrueType) のグリフを 8bit カバレッジにラスタライズ。`yk-gfx::text` のキャッシュミス時のみ使用 | 対応 (`std` feature を無効化、数学関数は `libm`)。`x86_64-unknown-none` でビルド・リンクし、SSE/x87 なしの freestanding バイナリで実行して動作確認済み (graphics.md §8.3) | Apache-2.0 | 純粋な Rust。浮動小数点はグリフのラスタライズでのみ使用 (soft-float で 1 グリフ約 45〜100µs) |
| [ab_glyph_rasterizer](https://crates.io/crates/ab_glyph_rasterizer) | 0.1.10 (ab_glyph の推移的依存) | 輪郭 (線・2 次/3 次ベジェ) の面積カバレッジ計算 | 対応 (`no_std`, `libm` feature は ab_glyph 経由で不要) | Apache-2.0 | 純粋な Rust |
| [owned_ttf_parser](https://crates.io/crates/owned_ttf_parser) | 0.25.1 (ab_glyph の推移的依存) | `ttf-parser` の所有型ラッパ (ab_glyph の `FontRef` が内部で使用) | 対応 (`default-features = false`) | Apache-2.0 | 純粋な Rust |
| [ttf-parser](https://crates.io/crates/ttf-parser) | 0.25.1 (推移的依存) | TrueType/OpenType の解析 (`cmap`, `glyf`, `hmtx`, `head`, `hhea`)。ゼロコピー・ヒープ確保なし・`unsafe` なし | 対応 (`#![no_std]`, `std` feature は無効) | MIT OR Apache-2.0 | 純粋な Rust |
| [core_maths](https://crates.io/crates/core_maths) | 0.1.1 (推移的依存) | `no_std` で `f32::sqrt` 等を提供する拡張トレイト (`libm` に委譲) | 対応 (`#![no_std]`) | MIT | 純粋な Rust |
| [libm](https://crates.io/crates/libm) | 0.2.16 (`ab_glyph` の `libm` feature 経由で推移的に使用) | 浮動小数点の数学関数 (ソフトウェア実装) | 対応 (`#![no_std]`) | MIT | 純粋な Rust。SSE/x87 を使わない |
| [zune-png](https://crates.io/crates/zune-png) | 0.5.2 (`=0.5.2`, `default-features = false`) | PNG デコーダ (全カラータイプ、1〜16bit、Adam7、tRNS)。IHDR を自前で検証してから呼び、展開サイズの上限を指定して展開爆弾を拒否する | 対応 (`no_std` + `alloc`、`std` feature を無効化) | MIT OR Apache-2.0 OR Zlib | 純粋な Rust。`unsafe` は含むが SIMD は使わない設定 (`default-features = false`) |
| [zune-core](https://crates.io/crates/zune-core) | 0.5.3 (`=0.5.3`, `default-features = false`) | zune 系共通の型 (`ZByteReader`、デコードオプション、ビット深度など) | 対応 (`no_std`) | MIT OR Apache-2.0 OR Zlib | 純粋な Rust |
| [zune-inflate](https://crates.io/crates/zune-inflate) | 0.2.54 (推移的依存) | zlib/DEFLATE の展開 (出力サイズの上限つき) | 対応 (`no_std` + `alloc`、`std` feature は無効) | MIT OR Apache-2.0 OR Zlib | 純粋な Rust |
| [simd-adler32](https://crates.io/crates/simd-adler32) | 0.3.10 (推移的依存) | zlib の Adler-32 チェックサム | 対応 (`no_std`、既定の `std` feature は無効) | MIT | 純粋な Rust (CPU 機能の検出は `std` のみで、`no_std` ではスカラ実装) |

- すべて `cargo build -p yk-gfx --target x86_64-unknown-none` (debug / release) でビルドできることを確認済み。
- **`unsafe` について**: `yk-gfx` 自体は `#![forbid(unsafe_code)]`。依存クレート (ttf-parser は `unsafe` なし、zune-png/zune-inflate/zune-core/simd-adler32/libm は内部に `unsafe` を含み得る) の
  `unsafe` は上流の責任範囲で、本リポジトリの `// SAFETY:` 規約の対象外。入力側 (PNG・フォント) はサイズ上限とファジングで防御している。
- `Cargo.lock` に載る版は上記のとおり (2026-11-16 時点の crates.io 最新の互換版)。

### 4.2 検討して採用しなかった候補

| 候補 | 不採用の理由 |
| --- | --- |
| `fontdue` | `no_std` 対応だが、フォント読み込み時に全グリフのアウトラインを展開し (3.4MB のフォントでヒープを大量に使い、読み込みが遅い)、ブートの短い時間と小さなヒープに向かない。`ab_glyph` は遅延解析で読み込み 13〜25µs・ヒープなし |
| `png` クレート (`image-rs/image-png`) | `std` 専用 (`std::io`)。カーネルにはリンクできない。**ホストのテストでの照合用 dev-dependency としてのみ採用** (§4.3) |
| `minipng` | `no_std` だがインターレース/16bit/低ビット深度のパレットなど対応範囲が狭い |
| `rusttype` | `ab_glyph` の前身 (メンテナンス終了) |
| `stb_truetype` 系 | C ライブラリの移植かバインディング。純粋な Rust のものを優先 |
| Zen Maru Gothic / Noto Sans JP (フォント) | 第一候補の M PLUS Rounded 1c を取得でき、常用漢字 2,136 字をすべて収録しているため採用せず (Zen Maru Gothic は同じ google/fonts から取得できることを確認済み) |

### 4.3 テスト・サンプル専用 (成果物には含まれない)

| 名前 | バージョン | 用途 | `no_std` 対応 | ライセンス | OS 環境との互換性 |
| --- | --- | --- | --- | --- | --- |
| [png](https://crates.io/crates/png) | 0.18.1 (`=0.18.1`, `yk-gfx` の `[dev-dependencies]`) | `examples/showcase.rs` が `build/gfx/showcase.png` を書き出す、テストが PNG フィクスチャを生成・照合する (ホストのみ) | 非対応 (`std`)。ホスト専用で `x86_64-unknown-none` のビルドには含まれない | MIT OR Apache-2.0 | 推移的依存: `bitflags` 2.13.2, `crc32fast` 1.5.5, `fdeflate` 0.3.7, `flate2` 1.1.11, `miniz_oxide` 0.9.1 / 0.8.9, `adler2` 2.0.1, `cfg-if` 1.0.5, `simd-adler32` 0.3.10 (いずれも MIT / Apache-2.0 系、`miniz_oxide` は Zlib も選択可、`adler2` は 0BSD も選択可) |

テストデータ: `gui/gfx/tests/data/joyo_kanji.txt` (常用漢字 2,136 字) と `jisx0208_kanji.txt` (JIS X 0208 漢字 6,355 字) は、
EDRDG の KANJIDIC2 (CC BY-SA 4.0) の学年・JIS コード情報から事実 (どの字が常用漢字/JIS 漢字か) だけを抽出した文字リストで、
フォントの収録検査にのみ使う (カーネルのバイナリには入らない)。PNG フィクスチャ (`tests/data/*.png`) は `tests/gen_png_fixtures.py` が生成したもので、第三者の素材を含まない。

### 4.4 フォント (カーネル/ISO に埋め込まれるデータ)

| 名前 | バージョン | 用途 | `no_std` 対応 | ライセンス | OS 環境との互換性 |
| --- | --- | --- | --- | --- | --- |
| [M PLUS Rounded 1c](https://github.com/google/fonts/tree/ce6523df69fcaec9534b128945bf47cd92bdc5b2/ofl/mplusrounded1c) (Regular / Bold) | google/fonts コミット `ce6523df69fcaec9534b128945bf47cd92bdc5b2` の `ofl/mplusrounded1c/` (sha256: Regular `fd003ca21668b7f1ab5665accd1a785587b75e18b8edff65c663fa6facefd882`, Bold `c698330e24aadd0da80cab1f321eee9c052d41b66ef84e7e5379e81fb084a890`)。`assets/fonts/` にコミット | UI の既定フォント (丸ゴシック、日本語対応) (§14)。Regular 3,389,564 B / Bold 3,542,384 B | — (データ。`&'static [u8]` として `include_bytes!` で埋め込む) | SIL Open Font License 1.1 (`assets/fonts/OFL.txt`。Copyright 2016 The Rounded M+ Project Authors。Reserved Font Name の指定なし) | TrueType (glyf)。常用漢字・JIS 第 1 水準・ひらがな・カタカナ・ASCII を完全に収録、JIS 第 2 水準は 57% (1,943/3,390)。OFL はフォント単体の販売以外の埋め込み・再配布を許可し、ライセンス全文を同梱すること (ISO には `OFL.txt` も同梱すること) が条件。OFL.txt の由来は `assets/fonts/SOURCE.txt` を参照 |

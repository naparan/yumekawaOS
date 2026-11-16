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

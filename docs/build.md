# ビルド手順

yumekawaOS は「通常の PC」と「ネットワーク制限のあるサンドボックス」のどちらでも、
**同じスクリプト・同じ `cargo build --target x86_64-unknown-none`** でビルドできる。
違いは `x86_64-unknown-none` 用の標準ライブラリ (core / alloc / compiler_builtins)
を最初に用意する方法だけである。

```sh
scripts/build.sh && scripts/make_iso.sh && scripts/test-boot.sh
```

## 1. 必要なもの

| ツール | 用途 | 動作確認したバージョン |
| --- | --- | --- |
| Rust (rustup) | カーネルのビルド | 1.97.0 (stable) |
| `x86_64-unknown-none` の core/alloc | `no_std` カーネルのリンク | 1.97.0 |
| git | Limine の取得 | 2.43.0 |
| xorriso | ISO 生成 | 1.5.6 |
| QEMU (`qemu-system-x86_64`) | 実行・テスト | 8.2.2 |
| OVMF (UEFI ファームウェア) | QEMU で UEFI 起動 | edk2 2024.02 (Ubuntu `ovmf` 2024.02-2ubuntu0.10) |
| python3 (標準ライブラリのみ) | テストでの QEMU monitor 操作・PNG 変換 | 3.13 |
| bash, coreutils | スクリプト | — |

mtools や C コンパイラは不要(Limine のバイナリリリースを使い、FAT イメージは Limine 同梱の
`limine-uefi-cd.bin` を使うため)。

スクリプトは不足しているツールを検出すると、何が足りないか・導入方法を表示して失敗する。
OVMF は Debian/Ubuntu/Arch/Fedora/QEMU 同梱の標準パスを自動検出する。見つからない場合は
`OVMF_CODE=/path/to/OVMF_CODE.fd` (必要なら `OVMF_VARS=...`) を指定する。

## 2. 通常の環境 (Linux / WSL2)

```sh
# Ubuntu / Debian / WSL2 の例
sudo apt install qemu-system-x86 ovmf xorriso git python3

# Rust (https://rustup.rs/) を導入済みとして
rustup toolchain install 1.97.0 --component clippy --component rustfmt
rustup target add x86_64-unknown-none --toolchain 1.97.0
rustup override set 1.97.0      # このリポジトリで 1.97.0 を使う (任意)

scripts/build.sh                # → build/kernel
scripts/make_iso.sh             # → build/yumekawaOS.iso, build/bootloader/
scripts/run.sh                  # QEMU で起動 (ウィンドウ + シリアルは端末)
scripts/test-boot.sh            # ヘッドレス自動テスト
```

他のバージョンの stable Rust でもビルドできる見込みだが、`scripts/build.sh` は
1.97.0 以外の場合に警告を出す。

### なぜ rust-toolchain.toml を置かないのか

`rust-toolchain.toml` に `channel = "1.97.0"` と `targets = ["x86_64-unknown-none"]` を書くと、
rustup が自動的にツールチェーンとターゲットを取得しようとする。static.rust-lang.org に到達できない
サンドボックスではこれが失敗し、ビルドそのものができなくなるため置いていない。
バージョンは `Cargo.toml` の `rust-version = "1.97"` と `scripts/build.sh` の確認で管理する。

## 3. サンドボックス環境 (`rustup target add` が使えない場合)

static.rust-lang.org に到達できない環境では `rustup target add x86_64-unknown-none` が使えない。
代わりに、**使用中の rustc と同一コミットの標準ライブラリソース** を GitHub から取得して
core / alloc / compiler_builtins を一度だけビルドし、rustup ツールチェーンの sysroot
(`$(rustc --print target-libdir --target x86_64-unknown-none)`) へ導入する。

```sh
scripts/sandbox-toolchain.sh    # 1 回だけ (数十秒)
scripts/build.sh && scripts/make_iso.sh && scripts/test-boot.sh
```

`scripts/sandbox-toolchain.sh` の処理:

1. `rustc -vV` から release (例: `1.97.0`) と commit-hash を取得。
2. `https://github.com/rust-lang/rust.git` のタグ `1.97.0` を sparse / shallow clone し
   (`library/` のみ)、タグのコミットが rustc の commit-hash
   (`2d8144b7880597b6e6d3dfd63a9a9efae3f533d3`) と一致することを確認。
3. rust-src が無ければビルド中だけ `<sysroot>/lib/rustlib/src/rust` にシンボリックリンクを張り、
   `RUSTC_BOOTSTRAP=1 cargo build -Zbuild-std=core,alloc,compiler_builtins
   -Zbuild-std-features=compiler-builtins-mem --release` を実行。
   プロファイルは rust-lang/rust の `library/Cargo.toml` の `[profile.dist]`
   (opt-level 3, codegen-units 1, debug 1, `-Cembed-bitcode=yes`,
   `-Cforce-frame-pointers=non-leaf`) に合わせている。
4. 生成された `libcore-*.rlib` / `liballoc-*.rlib` / `libcompiler_builtins-*.rlib` を
   sysroot へコピーし、スタンプファイル `.yumekawaos-sandbox-sysroot` を書く。終了時にリンクを削除。

作業ディレクトリは既定で `<repo>/../toolchain` (リポジトリ外)。`YUMEKAWA_TOOLCHAIN_WORK` で変更可能。
既に導入済み (または rustup で導入済み) なら何もしない。作り直すときは `--force`。

### 通常環境との差

| | 通常環境 | サンドボックス |
| --- | --- | --- |
| core/alloc の入手 | `rustup target add` (公式ビルド済み) | `scripts/sandbox-toolchain.sh` (同一コミットのソースからローカルビルド) |
| `RUSTC_BOOTSTRAP` | 使わない | sysroot のビルド時のみ使う (カーネルのビルドでは使わない) |
| カーネルのビルドコマンド | `scripts/build.sh` | 同じ |
| 生成物 | 同等 | 同等 (標準ライブラリのビルドオプションが公式と完全一致する保証はない) |

## 4. 成果物 (§42)

```text
build/
├── kernel               カーネル ELF (release: 最適化 + 行番号情報)
├── bootloader/          ISO に入れた Limine 一式
│   ├── BOOTX64.EFI
│   ├── limine-uefi-cd.bin
│   ├── limine.conf
│   ├── LICENSE.limine
│   └── VERSION          タグ・コミット・取得元
├── yumekawaOS.iso       UEFI ハイブリッド ISO (単独で配布可能)
├── iso_root/            ISO のルート (中間生成物)
├── screenshots/         test-boot.sh のスクリーンショット (PNG)
├── test/                test-boot.sh のシリアルログ・QEMU ログ等
└── run/                 run.sh 用の OVMF 変数ストア・USB イメージ
```

`scripts/build.sh --debug` で debug ビルド (ログは TRACE まで出力) になる。

## 5. 実行とテスト

### scripts/run.sh

| オプション | 内容 |
| --- | --- |
| (なし) | ISO を CD-ROM として起動。QEMU ウィンドウ + シリアルを端末へ |
| `--usb` | ISO のコピーを raw ディスクとして xHCI + usb-storage に接続して起動 |
| `--headless` | 画面なし |
| `--gdb` | `-s -S` (GDB で `target remote :1234`、シンボルは `build/kernel`) |
| `-- <args>` | 追加の QEMU 引数 |

KVM が使える環境では自動で `-accel kvm` を使う (`YUMEKAWA_NO_KVM=1` で無効化)。

### scripts/test-boot.sh

| ケース | 内容 | 合格条件 |
| --- | --- | --- |
| `cdrom` | ISO を IDE CD-ROM として UEFI 起動 | シリアルに `YUMEKAWAOS_BOOT_OK` / スクリーンショット検査に合格 |
| `usb` | ISO のバイト列をそのまま raw ディスクにして `qemu-xhci` + `usb-storage` で UEFI 起動 (USB メモリへ dd した状態の代理) | 同上 |
| `panic` | `cmdline: selftest=panic` に書き換えた検証用 ISO で起動 | `KERNEL PANIC at kernel/src/main.rs:<行>:<列>: selftest: ...` と `system halted.` が出て、起動完了マーカーが出ない |

スクリーンショット検査: QEMU monitor の `screendump` (PPM) を PNG に変換して
`build/screenshots/boot-<case>.png` に保存し、「色数 16 以上」「最頻色が画面の 50% 以下」
「最上行と最下行の色が異なる (グラデーション背景)」を確認する。

タイムアウト (`BOOT_TIMEOUT`、既定 300 秒) までにマーカーが出なければ失敗。QEMU が途中で
終了した場合も失敗。失敗時はシリアルログ末尾と QEMU のログを表示し、終了コード 1 を返す。
`TEST_CASES="cdrom usb"` のように実行するケースを選べる。

## 6. 再現性

| 要素 | 固定方法 |
| --- | --- |
| Rust | 1.97.0 で確認 (`rust-version`、`scripts/build.sh` が他バージョンで警告) |
| Rust クレート | `Cargo.lock` をコミットし、`cargo build --locked` でビルド。直接依存は `=` で完全固定 |
| Limine | タグ `v11.4.1-binary` / コミット `5be26a73d7b7b4d4477d18be94e1d16e615adf56` / `BOOTX64.EFI` と `limine-uefi-cd.bin` の sha256 を `scripts/lib/common.sh` で固定し、`scripts/fetch-limine.sh` が検証 |
| ISO | ファイルのタイムスタンプと xorriso の日付を `SOURCE_DATE_EPOCH` (未指定時は最終コミット時刻) で固定 |
| サンドボックスの標準ライブラリ | rustc の commit-hash とソースのコミット一致を検証 |

同じチェックアウト・同じパス・同じツールチェーンでは `build/kernel` と
`build/yumekawaOS.iso` はバイト単位で同一になる(確認済み)。ただし `build/kernel` の
デバッグ情報にはソースの絶対パス (リポジトリ・`~/.cargo/registry`) が含まれるため、
別のパスでビルドするとカーネル (と ISO) のバイト列は一致しない。

## 7. USB メモリへの書き込み

```sh
lsblk                          # 書き込み先を確認 (例: /dev/sdX)。間違えるとデータが消える
sudo dd if=build/yumekawaOS.iso of=/dev/sdX bs=4M conv=fsync status=progress
```

Windows では Rufus を使い、「DD イメージモードで書き込む」を選ぶ (ISO イメージモードは不可)。
起動時は UEFI ブートメニューで USB を選択し、Secure Boot を無効にする。

## 8. トラブルシューティング

| 症状 | 対処 |
| --- | --- |
| `x86_64-unknown-none 向けの core ライブラリがありません` | `rustup target add x86_64-unknown-none`、使えない環境では `scripts/sandbox-toolchain.sh` |
| `OVMF (UEFI ファームウェア) が見つかりません` | `sudo apt install ovmf` 等、または `OVMF_CODE` を指定 |
| `取得した Limine が固定値と一致しません` | タグの付け替えや改ざんの可能性。`deps/` を削除して再取得し、それでも失敗するなら固定値の見直しが必要 |
| テストが timeout | `build/test/serial-<case>.log` を確認。TCG で遅い環境では `BOOT_TIMEOUT` を延ばす |

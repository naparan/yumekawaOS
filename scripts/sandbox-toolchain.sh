#!/usr/bin/env bash
# =============================================================================
# yumekawaOS — サンドボックス専用ツールチェーンセットアップ
#
# 目的:
#   `rustup target add x86_64-unknown-none` が使えない環境(static.rust-lang.org
#   に到達できないサンドボックス等)で、x86_64-unknown-none 向けの
#   core / alloc / compiler_builtins を「いま使っている rustc と同一コミットの
#   library ソース」からビルドし、rustup ツールチェーンの sysroot
#   (lib/rustlib/x86_64-unknown-none/lib) へ導入する。
#
#   導入後は通常環境と全く同じ `cargo build --target x86_64-unknown-none`
#   (= scripts/build.sh)がそのまま通る。カーネル本体のビルドに
#   RUSTC_BOOTSTRAP や nightly 機能は一切使わない。
#
# 通常の PC では本スクリプトは不要。代わりに:
#   rustup target add x86_64-unknown-none
#
# 使い方:
#   scripts/sandbox-toolchain.sh            # 未導入なら導入
#   scripts/sandbox-toolchain.sh --force    # 既存の sysroot を作り直す
#
# 環境変数:
#   YUMEKAWA_TOOLCHAIN_WORK  作業ディレクトリ(既定: <repo>/../toolchain)
#                            library ソースとビルド中間物を置く。リポジトリ外。
#
# 取得元: https://github.com/rust-lang/rust.git の rustc と同じバージョンのタグ
#         (sparse checkout で library/ のみ。タグのコミットが
#          `rustc -vV` の commit-hash と一致することを検証する)
# =============================================================================
set -euo pipefail

TARGET="x86_64-unknown-none"
RUST_GIT_URL="https://github.com/rust-lang/rust.git"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
WORK="${YUMEKAWA_TOOLCHAIN_WORK:-$(cd "${REPO_ROOT}/.." && pwd)/toolchain}"

FORCE=0
for arg in "$@"; do
    case "$arg" in
        --force) FORCE=1 ;;
        -h|--help) sed -n '2,30p' "$0"; exit 0 ;;
        *) echo "error: unknown argument: $arg" >&2; exit 2 ;;
    esac
done

log() { printf '[sandbox-toolchain] %s\n' "$*"; }
die() { printf '[sandbox-toolchain] error: %s\n' "$*" >&2; exit 1; }

for tool in rustc cargo git; do
    command -v "$tool" >/dev/null 2>&1 || die "必要なコマンドが見つかりません: $tool"
done

# ---- 1. 使用中の rustc を特定 ---------------------------------------------------
RUSTC_VV="$(rustc -vV)"
RUST_RELEASE="$(printf '%s\n' "$RUSTC_VV" | sed -n 's/^release: //p')"
RUST_COMMIT="$(printf '%s\n' "$RUSTC_VV" | sed -n 's/^commit-hash: //p')"
SYSROOT="$(rustc --print sysroot)"
TARGET_LIBDIR="$(rustc --print target-libdir --target "$TARGET")"

case "$RUST_RELEASE" in
    *-nightly|*-beta*|*-dev) die "stable の rustc を想定しています (検出: $RUST_RELEASE)" ;;
    "") die "rustc のバージョンを取得できません" ;;
esac
[ -n "$RUST_COMMIT" ] || die "rustc の commit-hash を取得できません"

log "rustc ${RUST_RELEASE} (${RUST_COMMIT})"
log "sysroot: ${SYSROOT}"
log "target libdir: ${TARGET_LIBDIR}"

STAMP="${TARGET_LIBDIR}/.yumekawaos-sandbox-sysroot"
if [ "$FORCE" -eq 0 ] && ls "${TARGET_LIBDIR}"/libcore-*.rlib >/dev/null 2>&1; then
    if [ -f "$STAMP" ] && grep -q "rustc-commit=${RUST_COMMIT}" "$STAMP"; then
        log "既にこの rustc 用の ${TARGET} sysroot が導入済みです (再構築は --force)"
    else
        log "既に ${TARGET} の標準ライブラリが存在します (rustup 由来の可能性)。何もしません"
    fi
    exit 0
fi

# ---- 2. library ソースの取得(rustc と同一コミット) ------------------------------
SRC="${WORK}/rust-${RUST_RELEASE}"
mkdir -p "$WORK"
if [ ! -d "${SRC}/.git" ]; then
    log "rust-lang/rust タグ ${RUST_RELEASE} の library/ を取得します (sparse, depth=1)"
    git -c advice.detachedHead=false clone --quiet --depth 1 --branch "$RUST_RELEASE" \
        --filter=blob:none --sparse "$RUST_GIT_URL" "$SRC"
    git -C "$SRC" sparse-checkout set library
fi
SRC_COMMIT="$(git -C "$SRC" rev-parse HEAD)"
[ "$SRC_COMMIT" = "$RUST_COMMIT" ] || \
    die "library ソースのコミット (${SRC_COMMIT}) が rustc (${RUST_COMMIT}) と一致しません"
log "library ソース: ${SRC} @ ${SRC_COMMIT}"

# ---- 3. -Zbuild-std で core/alloc/compiler_builtins をビルド ---------------------
# cargo は <sysroot>/lib/rustlib/src/rust/library を参照するため、
# rust-src が無い場合のみビルド中だけ一時的にシンボリックリンクを張る。
RUST_SRC_LINK="${SYSROOT}/lib/rustlib/src/rust"
CREATED_LINK=0
cleanup() {
    if [ "$CREATED_LINK" -eq 1 ]; then
        rm -f "$RUST_SRC_LINK"
        rmdir "${SYSROOT}/lib/rustlib/src" 2>/dev/null || true
    fi
}
trap cleanup EXIT
if [ -e "$RUST_SRC_LINK" ]; then
    if [ ! -d "${RUST_SRC_LINK}/library/core" ]; then
        die "${RUST_SRC_LINK} が存在しますが library/core がありません"
    fi
    log "既存の rust-src を使用します: ${RUST_SRC_LINK}"
else
    mkdir -p "${SYSROOT}/lib/rustlib/src"
    ln -s "$SRC" "$RUST_SRC_LINK"
    CREATED_LINK=1
fi

BUILDER="${WORK}/sysroot-builder"
rm -rf "$BUILDER"
mkdir -p "${BUILDER}/src"
cat > "${BUILDER}/Cargo.toml" <<'TOML'
[package]
name = "yumekawaos-sysroot-builder"
version = "0.0.0"
edition = "2024"
publish = false

[lib]
path = "src/lib.rs"

# rust-lang/rust library/Cargo.toml の [profile.dist] に合わせる
[profile.release]
opt-level = 3
codegen-units = 1
debug = 1
debug-assertions = false
overflow-checks = false
panic = "abort"
TOML
cat > "${BUILDER}/src/lib.rs" <<'RS'
#![no_std]
extern crate alloc;
RS

log "core / alloc / compiler_builtins を ${TARGET} 向けにビルドします"
# RUSTC_BOOTSTRAP はこの sysroot ビルドのためだけに使う(カーネルのビルドでは使わない)。
# -Cembed-bitcode=yes / frame pointers は公式配布物(profile.dist)と同じ設定。
# compiler-builtins-mem: 公式の *-none ターゲット配布物と同様に memcpy 等を提供する。
(
    cd "$BUILDER"
    RUSTC_BOOTSTRAP=1 \
    RUSTFLAGS="-Cembed-bitcode=yes -Zunstable-options -Cforce-frame-pointers=non-leaf" \
    cargo build --release --quiet \
        --target "$TARGET" \
        -Zbuild-std=core,alloc,compiler_builtins \
        -Zbuild-std-features=compiler-builtins-mem
)

DEPS="${BUILDER}/target/${TARGET}/release/deps"
mkdir -p "$TARGET_LIBDIR"
rm -f "${TARGET_LIBDIR}"/*.rlib "$STAMP"
INSTALLED=()
for rlib in "$DEPS"/*.rlib; do
    name="$(basename "$rlib")"
    case "$name" in
        libyumekawaos_sysroot_builder-*) continue ;;
    esac
    cp "$rlib" "$TARGET_LIBDIR/"
    INSTALLED+=("$name")
done
ls "${TARGET_LIBDIR}"/libcore-*.rlib >/dev/null 2>&1 || die "libcore の導入に失敗しました"
ls "${TARGET_LIBDIR}"/liballoc-*.rlib >/dev/null 2>&1 || die "liballoc の導入に失敗しました"
ls "${TARGET_LIBDIR}"/libcompiler_builtins-*.rlib >/dev/null 2>&1 || die "libcompiler_builtins の導入に失敗しました"

{
    echo "# yumekawaOS scripts/sandbox-toolchain.sh により生成"
    echo "rustc-release=${RUST_RELEASE}"
    echo "rustc-commit=${RUST_COMMIT}"
    echo "source=${RUST_GIT_URL}#${RUST_RELEASE}"
    echo "features=compiler-builtins-mem"
    for f in "${INSTALLED[@]}"; do echo "rlib=${f}"; done
} > "$STAMP"

log "導入完了: ${TARGET_LIBDIR}"
for f in "${INSTALLED[@]}"; do log "  ${f}"; done

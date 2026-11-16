#!/usr/bin/env bash
# カーネルをビルドして build/kernel に置く。
#
#   scripts/build.sh            # release ビルド (既定)
#   scripts/build.sh --debug    # debug ビルド
#
# 通常環境・サンドボックス環境のどちらでも同じコマンド
# (`cargo build --target x86_64-unknown-none`) でビルドする。
set -euo pipefail
# shellcheck source=lib/common.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib/common.sh"

PROFILE="release"
for arg in "$@"; do
    case "$arg" in
        --debug) PROFILE="debug" ;;
        --release) PROFILE="release" ;;
        -h|--help) sed -n '2,9p' "$0"; exit 0 ;;
        *) die "不明な引数: $arg" ;;
    esac
done

require_tool cargo "Rust をインストールしてください: https://rustup.rs/"
require_tool rustc "Rust をインストールしてください: https://rustup.rs/"

rust_version="$(rustc --version | awk '{print $2}')"
if [ "$rust_version" != "$TESTED_RUST_VERSION" ]; then
    warn "rustc ${rust_version} を使用しています (動作確認済み: ${TESTED_RUST_VERSION})"
fi

# x86_64-unknown-none の標準ライブラリ (core/alloc) が sysroot にあるか確認
target_libdir="$(rustc --print target-libdir --target "${KERNEL_TARGET}")"
if ! ls "${target_libdir}"/libcore-*.rlib >/dev/null 2>&1; then
    die "${KERNEL_TARGET} 向けの core ライブラリがありません (${target_libdir})。
  通常の環境: rustup target add ${KERNEL_TARGET}
  rustup で取得できない環境 (サンドボックス等): scripts/sandbox-toolchain.sh
  詳細は docs/build.md を参照してください"
fi

cargo_args=(build --locked --target "${KERNEL_TARGET}" -p yumekawa-kernel)
if [ "$PROFILE" = "release" ]; then
    cargo_args+=(--release)
fi

log "cargo ${cargo_args[*]}"
(cd "${REPO_ROOT}" && cargo "${cargo_args[@]}")

mkdir -p "${BUILD_DIR}"
cp "${REPO_ROOT}/target/${KERNEL_TARGET}/${PROFILE}/kernel" "${BUILD_DIR}/kernel"
log "kernel: ${BUILD_DIR}/kernel (${PROFILE})"

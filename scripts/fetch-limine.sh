#!/usr/bin/env bash
# Limine のバイナリリリースを固定タグ/コミットで取得し deps/ (gitignore 対象) に置く。
#
#   タグ   : v11.4.1-binary
#   コミット: 5be26a73d7b7b4d4477d18be94e1d16e615adf56
#
# タグが付け替えられていた場合や、ファイルの sha256 が一致しない場合は失敗する。
set -euo pipefail
# shellcheck source=lib/common.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib/common.sh"

require_tool git "git をインストールしてください (例: sudo apt install git)"
require_tool sha256sum "coreutils をインストールしてください"

verify() {
    local dir="$1"
    local head
    head="$(git -C "$dir" rev-parse HEAD 2>/dev/null || true)"
    [ "$head" = "$LIMINE_COMMIT" ] || return 1
    echo "${LIMINE_SHA256_BOOTX64}  ${dir}/BOOTX64.EFI" | sha256sum --check --quiet - >/dev/null 2>&1 || return 1
    echo "${LIMINE_SHA256_UEFI_CD}  ${dir}/limine-uefi-cd.bin" | sha256sum --check --quiet - >/dev/null 2>&1 || return 1
    return 0
}

if [ -d "${LIMINE_DIR}" ] && verify "${LIMINE_DIR}"; then
    log "Limine ${LIMINE_TAG} (${LIMINE_COMMIT}) は取得済みです: ${LIMINE_DIR}"
    exit 0
fi

mkdir -p "${DEPS_DIR}"
tmp="${LIMINE_DIR}.tmp"
rm -rf "$tmp"
log "Limine ${LIMINE_TAG} を取得します: ${LIMINE_REPO}"
git -c advice.detachedHead=false clone --quiet --depth 1 --branch "${LIMINE_TAG}" "${LIMINE_REPO}" "$tmp"

if ! verify "$tmp"; then
    actual="$(git -C "$tmp" rev-parse HEAD)"
    rm -rf "$tmp"
    die "取得した Limine が固定値と一致しません (期待: ${LIMINE_COMMIT}, 実際: ${actual}, または sha256 不一致)"
fi

rm -rf "${LIMINE_DIR}"
mv "$tmp" "${LIMINE_DIR}"
log "取得完了: ${LIMINE_DIR} @ ${LIMINE_COMMIT}"

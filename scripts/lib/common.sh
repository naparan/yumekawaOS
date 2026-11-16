# shellcheck shell=bash
# yumekawaOS scripts 共通処理 (source して使う)
# ここで定義する変数は source 側のスクリプトで使う
# shellcheck disable=SC2034

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
BUILD_DIR="${REPO_ROOT}/build"
DEPS_DIR="${REPO_ROOT}/deps"
KERNEL_TARGET="x86_64-unknown-none"
ISO_PATH="${BUILD_DIR}/yumekawaOS.iso"

# 動作確認済みの Rust バージョン (docs/build.md 参照)
TESTED_RUST_VERSION="1.97.0"

# Limine: バイナリリリースのタグとコミットを固定する (scripts/fetch-limine.sh)
LIMINE_VERSION="11.4.1"
LIMINE_TAG="v${LIMINE_VERSION}-binary"
LIMINE_COMMIT="5be26a73d7b7b4d4477d18be94e1d16e615adf56"
LIMINE_REPO="https://github.com/limine-bootloader/limine.git"
LIMINE_DIR="${DEPS_DIR}/limine-${LIMINE_VERSION}"
# 取得物の完全性確認用 (sha256)
LIMINE_SHA256_BOOTX64="333f7a69379b1f47e019be215885cdc078b9edd6e9255cbe2f706d6c5ec0ef06"
LIMINE_SHA256_UEFI_CD="0c7a1daf60d2140d124bf020ea99fff95788963afc55ed9acdb9bbfe603a7b20"

_script_name="$(basename "${0}")"

log() { printf '[%s] %s\n' "${_script_name}" "$*"; }
warn() { printf '[%s] warning: %s\n' "${_script_name}" "$*" >&2; }
die() { printf '[%s] error: %s\n' "${_script_name}" "$*" >&2; exit 1; }

# require_tool <command> <説明(導入方法)>
require_tool() {
    command -v "$1" >/dev/null 2>&1 || die "必要なツールが見つかりません: '$1' — $2"
}

# OVMF (UEFI ファームウェア) のコード/変数ファイルを探す。
# 環境変数 OVMF_CODE / OVMF_VARS で明示指定も可能。
# 成功すると OVMF_CODE と OVMF_VARS (無ければ空) を設定する。
find_ovmf() {
    if [ -n "${OVMF_CODE:-}" ]; then
        [ -f "${OVMF_CODE}" ] || die "OVMF_CODE で指定されたファイルがありません: ${OVMF_CODE}"
        OVMF_VARS="${OVMF_VARS:-}"
        return 0
    fi
    # Secure Boot 版 (.secboot / .ms) は除外する (Limine は署名されていないため)
    local pairs=(
        "/usr/share/OVMF/OVMF_CODE_4M.fd:/usr/share/OVMF/OVMF_VARS_4M.fd"          # Debian/Ubuntu
        "/usr/share/OVMF/OVMF_CODE.fd:/usr/share/OVMF/OVMF_VARS.fd"                # 旧 Debian/Ubuntu
        "/usr/share/edk2/x64/OVMF_CODE.4m.fd:/usr/share/edk2/x64/OVMF_VARS.4m.fd"  # Arch
        "/usr/share/edk2/ovmf/OVMF_CODE.fd:/usr/share/edk2/ovmf/OVMF_VARS.fd"      # Fedora
        "/usr/share/edk2-ovmf/x64/OVMF_CODE.fd:/usr/share/edk2-ovmf/x64/OVMF_VARS.fd"
        "/usr/share/qemu/edk2-x86_64-code.fd:/usr/share/qemu/edk2-i386-vars.fd"    # QEMU 同梱
        "/opt/homebrew/share/qemu/edk2-x86_64-code.fd:/opt/homebrew/share/qemu/edk2-i386-vars.fd"
        "/usr/local/share/qemu/edk2-x86_64-code.fd:/usr/local/share/qemu/edk2-i386-vars.fd"
    )
    local pair code vars
    for pair in "${pairs[@]}"; do
        code="${pair%%:*}"
        vars="${pair#*:}"
        if [ -f "$code" ]; then
            OVMF_CODE="$code"
            if [ -f "$vars" ]; then OVMF_VARS="$vars"; else OVMF_VARS=""; fi
            return 0
        fi
    done
    die "OVMF (UEFI ファームウェア) が見つかりません。
  Debian/Ubuntu/WSL2: sudo apt install ovmf
  Fedora: sudo dnf install edk2-ovmf / Arch: sudo pacman -S edk2-ovmf
  または OVMF_CODE=/path/to/OVMF_CODE.fd (必要なら OVMF_VARS も) を指定してください"
}

# QEMU に渡す pflash 引数を組み立てる (変数ストアは run ごとに build/ 配下へコピー)。
# 使い方: ovmf_args <vars のコピー先>
ovmf_args() {
    local vars_copy="$1"
    QEMU_FW_ARGS=(-drive "if=pflash,format=raw,unit=0,readonly=on,file=${OVMF_CODE}")
    if [ -n "${OVMF_VARS}" ]; then
        cp "${OVMF_VARS}" "${vars_copy}"
        QEMU_FW_ARGS+=(-drive "if=pflash,format=raw,unit=1,file=${vars_copy}")
    fi
}

# KVM が使えれば使う (このサンドボックスのように無ければ TCG)
qemu_accel_args() {
    if [ -z "${YUMEKAWA_NO_KVM:-}" ] && [ -r /dev/kvm ] && [ -w /dev/kvm ]; then
        QEMU_ACCEL_ARGS=(-accel kvm -cpu host)
    else
        QEMU_ACCEL_ARGS=(-accel tcg -cpu qemu64)
    fi
}

# ISO ルートディレクトリから UEFI 起動可能なハイブリッド ISO を作る。
#   make_hybrid_iso <iso_root> <出力 ISO>
# - El Torito EFI ブートイメージ: boot/limine/limine-uefi-cd.bin
# - -efi-boot-part --efi-boot-image: 同じ FAT イメージを GPT の EFI System Partition
#   として公開 (USB メモリへ dd した場合に UEFI がそこから起動する)
# - --protective-msdos-label: GPT 用の保護 MBR
# - -hfsplus -apm-block-size 2048: Limine 推奨構成 (Mac の UEFI 向け)
make_hybrid_iso() {
    local root="$1" out="$2"
    rm -f "$out"
    xorriso -as mkisofs \
        -R -r -J \
        -V YUMEKAWAOS \
        -hfsplus -apm-block-size 2048 \
        --efi-boot boot/limine/limine-uefi-cd.bin \
        -efi-boot-part --efi-boot-image \
        --protective-msdos-label \
        "$root" -o "$out" 2>&1 | sed 's/^/  xorriso: /'
    [ "${PIPESTATUS[0]}" -eq 0 ] || die "xorriso が失敗しました"
    [ -s "$out" ] || die "ISO が生成されませんでした: $out"
}

#!/usr/bin/env bash
# QEMU + OVMF (UEFI) で build/yumekawaOS.iso を起動する。シリアル (COM1) は標準入出力へ。
#
#   scripts/run.sh                 # ISO を CD-ROM として起動 (ウィンドウ表示)
#   scripts/run.sh --usb           # ISO を USB メモリ (xHCI + usb-storage) として起動
#   scripts/run.sh --headless      # 画面なし (シリアルのみ)
#   scripts/run.sh --gdb           # GDB 待ち受け (tcp::1234, 起動時停止)
#   scripts/run.sh -- <QEMU 引数>  # 追加の QEMU 引数
#
# 環境変数: OVMF_CODE / OVMF_VARS (OVMF の場所を明示), QEMU_MEM (既定 512M),
#           YUMEKAWA_NO_KVM=1 (KVM を使わない)
set -euo pipefail
# shellcheck source=lib/common.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib/common.sh"

MODE="cdrom"
HEADLESS=0
GDB=0
EXTRA=()
while [ $# -gt 0 ]; do
    case "$1" in
        --usb) MODE="usb" ;;
        --cdrom) MODE="cdrom" ;;
        --headless) HEADLESS=1 ;;
        --gdb) GDB=1 ;;
        --) shift; EXTRA=("$@"); break ;;
        -h|--help) sed -n '2,13p' "$0"; exit 0 ;;
        *) die "不明な引数: $1" ;;
    esac
    shift
done

require_tool qemu-system-x86_64 "QEMU をインストールしてください (例: sudo apt install qemu-system-x86)"
[ -f "${ISO_PATH}" ] || die "${ISO_PATH} がありません。scripts/build.sh && scripts/make_iso.sh を実行してください"
find_ovmf
mkdir -p "${BUILD_DIR}/run"
ovmf_args "${BUILD_DIR}/run/ovmf-vars-${MODE}.fd"
qemu_accel_args

MEDIA_ARGS=()
case "$MODE" in
    cdrom)
        MEDIA_ARGS=(-cdrom "${ISO_PATH}")
        ;;
    usb)
        # dd で USB メモリへ書き込んだ状態を再現するため、ISO のバイト列をそのまま raw ディスクにする
        USB_IMG="${BUILD_DIR}/run/usb-stick.img"
        cp "${ISO_PATH}" "${USB_IMG}"
        MEDIA_ARGS=(
            -device qemu-xhci,id=xhci
            -drive "if=none,id=usbstick,format=raw,file=${USB_IMG}"
            -device usb-storage,bus=xhci.0,drive=usbstick,bootindex=0
        )
        ;;
esac

DISPLAY_ARGS=()
if [ "$HEADLESS" -eq 1 ]; then
    DISPLAY_ARGS=(-display none)
fi
GDB_ARGS=()
if [ "$GDB" -eq 1 ]; then
    GDB_ARGS=(-s -S)
    log "GDB: target remote :1234 (symbol-file build/kernel)"
fi

log "OVMF: ${OVMF_CODE}"
log "mode: ${MODE}, accel: ${QEMU_ACCEL_ARGS[*]}"
exec qemu-system-x86_64 \
    -machine q35 \
    "${QEMU_ACCEL_ARGS[@]}" \
    -m "${QEMU_MEM:-512M}" \
    "${QEMU_FW_ARGS[@]}" \
    "${MEDIA_ARGS[@]}" \
    -serial stdio \
    -no-reboot \
    "${DISPLAY_ARGS[@]}" \
    "${GDB_ARGS[@]}" \
    "${EXTRA[@]}"

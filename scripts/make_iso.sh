#!/usr/bin/env bash
# build/kernel と Limine から UEFI 起動可能な ISO (build/yumekawaOS.iso) を作る。
#
# 生成される ISO は「ハイブリッド」構成:
#   - El Torito の EFI ブートイメージ (CD/DVD として UEFI 起動)
#   - 同じ FAT イメージを指す GPT パーティション (+ 保護 MBR)
#     → ISO をそのまま USB メモリへ書き込む (dd / Rufus の DD モード等) と、
#       USB ディスクの ESP として UEFI から起動できる。
#
# 副産物:
#   build/bootloader/   ISO に含めた Limine のファイル一式 (§42)
#   build/iso_root/     ISO のルートディレクトリ (中間生成物)
set -euo pipefail
# shellcheck source=lib/common.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib/common.sh"

require_tool xorriso "xorriso をインストールしてください (例: sudo apt install xorriso)"

KERNEL="${BUILD_DIR}/kernel"
[ -f "${KERNEL}" ] || die "${KERNEL} がありません。先に scripts/build.sh を実行してください"

"${REPO_ROOT}/scripts/fetch-limine.sh"

ISO_ROOT="${BUILD_DIR}/iso_root"
BOOTLOADER_OUT="${BUILD_DIR}/bootloader"
rm -rf "${ISO_ROOT}" "${BOOTLOADER_OUT}"
mkdir -p "${ISO_ROOT}/boot/limine" "${ISO_ROOT}/EFI/BOOT" "${BOOTLOADER_OUT}"

# カーネルと設定
cp "${KERNEL}" "${ISO_ROOT}/boot/kernel"
cp "${REPO_ROOT}/boot/limine.conf" "${ISO_ROOT}/boot/limine/limine.conf"
# El Torito / GPT から参照される FAT イメージ (中に EFI/BOOT/BOOTX64.EFI を含む)
cp "${LIMINE_DIR}/limine-uefi-cd.bin" "${ISO_ROOT}/boot/limine/limine-uefi-cd.bin"
# ISO9660 側にも EFI 実行ファイルを置く (Limine の推奨構成)
cp "${LIMINE_DIR}/BOOTX64.EFI" "${ISO_ROOT}/EFI/BOOT/BOOTX64.EFI"
# ライセンス表示
cp "${LIMINE_DIR}/LICENSE" "${ISO_ROOT}/boot/limine/LICENSE.limine"

# build/bootloader: 使用したブートローダ一式
cp "${LIMINE_DIR}/BOOTX64.EFI" "${LIMINE_DIR}/limine-uefi-cd.bin" "${BOOTLOADER_OUT}/"
cp "${LIMINE_DIR}/LICENSE" "${BOOTLOADER_OUT}/LICENSE.limine"
cp "${REPO_ROOT}/boot/limine.conf" "${BOOTLOADER_OUT}/limine.conf"
cat > "${BOOTLOADER_OUT}/VERSION" <<VERSION
Limine ${LIMINE_VERSION}
tag: ${LIMINE_TAG}
commit: ${LIMINE_COMMIT}
source: ${LIMINE_REPO}
VERSION

# 再現性: タイムスタンプを固定する (SOURCE_DATE_EPOCH 未指定時は最終コミット時刻)
if [ -z "${SOURCE_DATE_EPOCH:-}" ]; then
    SOURCE_DATE_EPOCH="$(git -C "${REPO_ROOT}" log -1 --format=%ct 2>/dev/null || echo 0)"
fi
export SOURCE_DATE_EPOCH
find "${ISO_ROOT}" -exec touch -h -d "@${SOURCE_DATE_EPOCH}" {} +

log "xorriso で ${ISO_PATH} を生成します (SOURCE_DATE_EPOCH=${SOURCE_DATE_EPOCH})"
make_hybrid_iso "${ISO_ROOT}" "${ISO_PATH}"

log "ISO: ${ISO_PATH} ($(du -h "${ISO_PATH}" | cut -f1))"
log "USB メモリへの書き込み例: sudo dd if=build/yumekawaOS.iso of=/dev/sdX bs=4M conv=fsync status=progress"

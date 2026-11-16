#!/usr/bin/env bash
# ヘッドレス起動テスト。失敗時は非 0 で終了する。
#
#   a. ISO を CD-ROM として UEFI 起動 → シリアルに起動完了マーカーが出ること
#   b. 同じ ISO のバイト列を raw ディスクとして USB ストレージ (xHCI + usb-storage) から
#      UEFI 起動 → 同じマーカーが出ること (USB メモリへ dd した場合の代理検証)
#   c. 各起動で QEMU monitor の screendump を取り、PNG にして build/screenshots/ へ保存。
#      画面が単色でないこと等を機械的に検査する
#   d. (panic) カーネルコマンドライン `selftest=panic` を設定した検証用 ISO を
#      build/test/ に作って起動し、パニックハンドラが場所とメッセージをシリアルへ
#      出力して停止すること (起動完了マーカーが出ないこと) を確認する
#
# ハング (タイムアウト) やカーネルパニックはテスト失敗として扱う。
#
# 環境変数:
#   BOOT_TIMEOUT   1 ケースあたりの起動待ち秒数 (既定 300。TCG では起動に時間がかかる)
#   TEST_CASES     実行するケース (既定 "cdrom usb panic")
#   OVMF_CODE / OVMF_VARS, YUMEKAWA_NO_KVM — scripts/run.sh と同じ
set -euo pipefail
# shellcheck source=lib/common.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib/common.sh"

BOOT_MARKER="YUMEKAWAOS_BOOT_OK"
BOOT_TIMEOUT="${BOOT_TIMEOUT:-300}"
TEST_CASES="${TEST_CASES:-cdrom usb panic}"
PANIC_MARKER="KERNEL PANIC at"
HALT_MARKER="system halted."
TOOL="${REPO_ROOT}/scripts/lib/qemu_tool.py"
TEST_DIR="${BUILD_DIR}/test"
SHOT_DIR="${BUILD_DIR}/screenshots"

require_tool qemu-system-x86_64 "QEMU をインストールしてください (例: sudo apt install qemu-system-x86)"
require_tool python3 "python3 をインストールしてください (monitor 操作と PNG 変換に使用)"
[ -f "${ISO_PATH}" ] || die "${ISO_PATH} がありません。scripts/build.sh && scripts/make_iso.sh を実行してください"
find_ovmf
qemu_accel_args

mkdir -p "${TEST_DIR}" "${SHOT_DIR}"

QEMU_PID=""
cleanup() {
    if [ -n "${QEMU_PID}" ] && kill -0 "${QEMU_PID}" 2>/dev/null; then
        kill "${QEMU_PID}" 2>/dev/null || true
        wait "${QEMU_PID}" 2>/dev/null || true
    fi
}
trap cleanup EXIT

# make_selftest_iso <selftest 名>: cmdline に selftest=<名> を設定した検証用 ISO を作り、パスを出力する
make_selftest_iso() {
    local which="$1"
    local root="${TEST_DIR}/iso_root-selftest-${which}"
    local out="${TEST_DIR}/yumekawaOS-selftest-${which}.iso"
    require_tool xorriso "xorriso をインストールしてください (例: sudo apt install xorriso)"
    [ -d "${BUILD_DIR}/iso_root" ] || die "${BUILD_DIR}/iso_root がありません。scripts/make_iso.sh を実行してください"
    rm -rf "$root"
    cp -a "${BUILD_DIR}/iso_root" "$root"
    sed -i "s/^\([[:space:]]*\)cmdline:.*/\1cmdline: selftest=${which}/" "$root/boot/limine/limine.conf"
    grep -q "cmdline: selftest=${which}" "$root/boot/limine/limine.conf" \
        || die "検証用 limine.conf の書き換えに失敗しました"
    make_hybrid_iso "$root" "$out" >&2
    printf '%s\n' "$out"
}

# run_case <name>
run_case() {
    local name="$1"
    local serial="${TEST_DIR}/serial-${name}.log"
    local qemu_log="${TEST_DIR}/qemu-${name}.log"
    local mon="${TEST_DIR}/monitor-${name}.sock"
    local ppm="${TEST_DIR}/screen-${name}.ppm"
    local png="${SHOT_DIR}/boot-${name}.png"
    rm -f "$serial" "$qemu_log" "$mon" "$ppm" "$png"

    ovmf_args "${TEST_DIR}/ovmf-vars-${name}.fd"

    local media=()
    case "$name" in
        cdrom)
            media=(-drive "if=none,id=cd,media=cdrom,format=raw,readonly=on,file=${ISO_PATH}"
                   -device ide-cd,drive=cd,bootindex=0)
            ;;
        panic)
            local panic_iso
            panic_iso="$(make_selftest_iso panic)"
            media=(-drive "if=none,id=cd,media=cdrom,format=raw,readonly=on,file=${panic_iso}"
                   -device ide-cd,drive=cd,bootindex=0)
            ;;
        usb)
            # ISO をそのまま raw ディスクとしてコピー (= dd でUSBメモリに書き込んだ状態)
            local img="${TEST_DIR}/usb-stick.img"
            cp "${ISO_PATH}" "$img"
            media=(-device qemu-xhci,id=xhci
                   -drive "if=none,id=usbstick,format=raw,file=${img}"
                   -device usb-storage,bus=xhci.0,drive=usbstick,removable=on,bootindex=0)
            ;;
        *) die "unknown test case: $name" ;;
    esac

    log "[$name] QEMU を起動します (timeout ${BOOT_TIMEOUT}s, ${QEMU_ACCEL_ARGS[*]})"
    qemu-system-x86_64 \
        -machine q35 \
        "${QEMU_ACCEL_ARGS[@]}" \
        -m 512M \
        "${QEMU_FW_ARGS[@]}" \
        "${media[@]}" \
        -nodefaults -vga std \
        -display none \
        -serial "file:${serial}" \
        -monitor "unix:${mon},server,nowait" \
        -no-reboot \
        >"$qemu_log" 2>&1 &
    QEMU_PID=$!

    local start elapsed=0 result=""
    start=$(date +%s)
    while :; do
        if [ -f "$serial" ] && grep -aq "${BOOT_MARKER}" "$serial"; then
            result="booted"; break
        fi
        if [ -f "$serial" ] && grep -aq "${HALT_MARKER}" "$serial"; then
            result="fatal"; break
        fi
        if ! kill -0 "${QEMU_PID}" 2>/dev/null; then
            result="exited"; break
        fi
        elapsed=$(( $(date +%s) - start ))
        if [ "$elapsed" -ge "${BOOT_TIMEOUT}" ]; then
            result="timeout"; break
        fi
        sleep 1
    done
    elapsed=$(( $(date +%s) - start ))

    local status=0
    if [ "$name" = "panic" ]; then
        # パニック検証: 場所付きのパニック出力と停止メッセージが出て、起動完了には至らないこと
        if [ "$result" = "fatal" ] \
            && grep -aq "${PANIC_MARKER} kernel/src/main.rs:[0-9]*:[0-9]*: selftest: intentional panic" "$serial"; then
            log "[$name] パニック出力を検出 (${elapsed}s): $(grep -a "${PANIC_MARKER}" "$serial" | tr -d '\r' | head -1)"
            sleep 2
            if grep -aq "${BOOT_MARKER}" "$serial"; then
                log "[$name] FAIL: パニック後に起動完了マーカーが出力されました"
                status=1
            fi
        else
            log "[$name] FAIL: 期待したパニック出力がありません (result=${result}, ${elapsed}s)"
            status=1
        fi
    elif [ "$result" = "booted" ]; then
        log "[$name] 起動完了マーカーを検出 (${elapsed}s): $(grep -a "${BOOT_MARKER}" "$serial" | tr -d '\r' | head -1)"
        # 画面描画とマーカー出力は同期しているが、念のため少し待ってから取得
        sleep 1
        python3 "$TOOL" monitor "$mon" "screendump ${ppm}" >/dev/null
        local i
        for i in 1 2 3 4 5 6 7 8 9 10; do
            [ -s "$ppm" ] && break
            sleep 1
        done
        if [ -s "$ppm" ]; then
            python3 "$TOOL" ppm2png "$ppm" "$png"
            log "[$name] screenshot: ${png}"
            # pipefail により python3 の失敗がパイプライン全体の失敗になる
            if ! python3 "$TOOL" check-image "$ppm" | sed "s/^/[${_script_name}] [$name] /"; then
                status=1
            fi
        else
            log "[$name] FAIL: screendump が取得できませんでした"
            status=1
        fi
    else
        log "[$name] FAIL: ${result} (${elapsed}s)"
        status=1
    fi

    if [ "$status" -ne 0 ]; then
        log "[$name] --- シリアルログ末尾 (${serial}) ---"
        tail -n 30 "$serial" 2>/dev/null | tr -d '\r' | sed 's/^/    /' || true
        log "[$name] --- QEMU ログ (${qemu_log}) ---"
        sed 's/^/    /' "$qemu_log" || true
    fi

    if kill -0 "${QEMU_PID}" 2>/dev/null; then
        python3 "$TOOL" monitor "$mon" "quit" >/dev/null 2>&1 || true
        sleep 1
    fi
    cleanup
    QEMU_PID=""
    return "$status"
}

failed=()
passed=()
for case_name in ${TEST_CASES}; do
    if run_case "$case_name"; then
        passed+=("$case_name")
    else
        failed+=("$case_name")
    fi
done

log "結果: passed=[${passed[*]:-}] failed=[${failed[*]:-}]"
if [ "${#failed[@]}" -ne 0 ]; then
    log "FAILED"
    exit 1
fi
log "ALL PASSED"

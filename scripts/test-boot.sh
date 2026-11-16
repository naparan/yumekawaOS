#!/usr/bin/env bash
# ヘッドレス起動テスト。どれかが失敗・ハング (タイムアウト) したら非 0 で終了する。
#
# ケース (TEST_CASES で選択。既定はすべて):
#   cdrom          ISO を CD-ROM として UEFI 起動 → 起動完了マーカー + スクリーンショット検査
#   usb            ISO のバイト列を USB ストレージ (xHCI + usb-storage) から UEFI 起動 → 同上
#   panic          cmdline `selftest=panic` → パニック出力 (場所・メッセージ) と Kernel Panic 画面
#   memory         cmdline `selftest=memory` → フレームアロケータ / マッパ / ヒープ (項目ごとに PASS/FAIL)
#                  (-m 3G (4GiB 以上に 1GiB 境界の RAM ができる)・1GiB ページ・SMEP・SMAP 対応 CPU で実行)
#   exceptions     cmdline `selftest=exceptions` → #BP 復帰 / #PF (CR2) / #GP / W^X / NX / SMEP / SMAP
#   timer          cmdline `selftest=timer` → tick の進行・uptime と PM timer・ホストの実時間の比較
#   keyboard       通常起動後に QEMU monitor の sendkey でキーを注入 → スキャンコード (set 1) の受信ログ
#   stackoverflow  cmdline `selftest=stackoverflow` → ガードページ → #DF (IST) → Kernel Panic 画面
#   idle           通常起動後 IDLE_SECONDS 秒アイドル → 例外・パニック・エラーが無く、ハートビートが進むこと
#
# スクリーンショットは build/screenshots/、シリアルログ等は build/test/ に保存する。
#
# 環境変数:
#   BOOT_TIMEOUT   1 ケースあたりの起動待ち秒数 (既定 300。TCG では起動に時間がかかる)
#   TEST_CASES     実行するケース (既定: 上の全ケース)
#   IDLE_SECONDS   idle ケースのアイドル時間 (既定 30)
#   OVMF_CODE / OVMF_VARS, YUMEKAWA_NO_KVM — scripts/run.sh と同じ
set -euo pipefail
# shellcheck source=lib/common.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib/common.sh"

BOOT_MARKER="YUMEKAWAOS_BOOT_OK"
BOOT_TIMEOUT="${BOOT_TIMEOUT:-300}"
TEST_CASES="${TEST_CASES:-cdrom usb panic memory exceptions timer keyboard stackoverflow idle}"
IDLE_SECONDS="${IDLE_SECONDS:-30}"
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
WATCH_PID=""
cleanup() {
    if [ -n "${QEMU_PID}" ] && kill -0 "${QEMU_PID}" 2>/dev/null; then
        kill "${QEMU_PID}" 2>/dev/null || true
        wait "${QEMU_PID}" 2>/dev/null || true
    fi
    if [ -n "${WATCH_PID}" ] && kill -0 "${WATCH_PID}" 2>/dev/null; then
        kill "${WATCH_PID}" 2>/dev/null || true
        wait "${WATCH_PID}" 2>/dev/null || true
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

# serial_has <file> <固定文字列>
serial_has() { grep -aqF -- "$2" "$1"; }

# take_screenshot <monitor socket> <ppm> <png>: 成功すれば 0
take_screenshot() {
    local mon="$1" ppm="$2" png="$3" _
    python3 "$TOOL" monitor "$mon" "screendump ${ppm}" >/dev/null
    for _ in 1 2 3 4 5 6 7 8 9 10; do
        [ -s "$ppm" ] && break
        sleep 1
    done
    [ -s "$ppm" ] || return 1
    python3 "$TOOL" ppm2png "$ppm" "$png"
}

# check_selftest_suite <serial> <suite> <case 名> [no-skip]: SELFTEST 行を検査して結果を表示する
check_selftest_suite() {
    local serial="$1" suite="$2" name="$3" no_skip="${4:-}"
    local done_line fails skips
    done_line="$(grep -a "SELFTEST_DONE suite=${suite} " "$serial" | tr -d '\r' | head -1 || true)"
    grep -a "SELFTEST ${suite}\." "$serial" | tr -d '\r' | sed "s/^.*SELFTEST /[${_script_name}] [${name}]   /" || true
    if [ -z "$done_line" ]; then
        log "[$name] FAIL: SELFTEST_DONE suite=${suite} が出力されていません"
        return 1
    fi
    log "[$name] ${done_line#*] selftest: }"
    fails="$(grep -ac "SELFTEST ${suite}\..*: FAIL" "$serial" || true)"
    if [ "$fails" -ne 0 ] || ! grep -aq "SELFTEST_DONE suite=${suite} passed=[0-9]* failed=0 " "$serial"; then
        log "[$name] FAIL: ${fails} 個の項目が失敗しました"
        return 1
    fi
    if [ -n "$no_skip" ]; then
        skips="$(grep -ac "SELFTEST ${suite}\..*: SKIP" "$serial" || true)"
        if [ "$skips" -ne 0 ]; then
            log "[$name] FAIL: このケースは全機能を持つ CPU で実行しているため SKIP は許容しません (${skips} 件)"
            return 1
        fi
    fi
    return 0
}

# check_no_errors <serial> <case 名>: 例外・パニック・ERROR ログが無いこと
check_no_errors() {
    local serial="$1" name="$2" bad
    bad="$(grep -aE '^\[(ERROR|FATAL)\]|KERNEL PANIC|unhandled interrupt|recovered via exception table' "$serial" | tr -d '\r' | head -5 || true)"
    if [ -n "$bad" ]; then
        log "[$name] FAIL: 想定外のエラー/例外のログがあります:"
        printf '%s\n' "$bad" | sed 's/^/    /'
        return 1
    fi
    return 0
}

# run_case <name>
run_case() {
    local name="$1"
    local serial="${TEST_DIR}/serial-${name}.log"
    local qemu_log="${TEST_DIR}/qemu-${name}.log"
    local mon="${TEST_DIR}/monitor-${name}.sock"
    local ppm="${TEST_DIR}/screen-${name}.ppm"
    local png="${SHOT_DIR}/boot-${name}.png"
    local watch_log="${TEST_DIR}/timer-watch-${name}.log"
    rm -f "$serial" "$qemu_log" "$mon" "$ppm" "$png" "$watch_log"

    ovmf_args "${TEST_DIR}/ovmf-vars-${name}.fd"

    # ケースごとの設定: 起動メディア / 期待する結果 (booted = 起動完了, fatal = 停止) / CPU / メモリ
    local media=() expect="booted" cpu_args=("${QEMU_ACCEL_ARGS[@]}") memory="512M" iso="${ISO_PATH}"
    case "$name" in
        cdrom|keyboard|idle) ;;
        panic|stackoverflow)
            iso="$(make_selftest_iso "$name")"
            expect="fatal"
            ;;
        memory|exceptions|timer)
            iso="$(make_selftest_iso "$name")"
            ;;
        usb) ;;
        *) die "unknown test case: $name" ;;
    esac
    case "$name" in
        memory|exceptions)
            # SMEP/SMAP/1GiB ページの経路も検証する (KVM の -cpu host ならホスト CPU の機能に従う)
            if [ "${cpu_args[*]}" = "-accel tcg -cpu qemu64" ]; then
                cpu_args=(-accel tcg -cpu qemu64,+smep,+smap,+pdpe1gb)
            fi
            if [ "$name" = "memory" ]; then
                memory="3G"
            fi
            ;;
    esac
    if [ "$name" = "usb" ]; then
        # ISO をそのまま raw ディスクとしてコピー (= dd でUSBメモリに書き込んだ状態)
        local img="${TEST_DIR}/usb-stick.img"
        cp "${ISO_PATH}" "$img"
        media=(-device qemu-xhci,id=xhci
               -drive "if=none,id=usbstick,format=raw,file=${img}"
               -device usb-storage,bus=xhci.0,drive=usbstick,removable=on,bootindex=0)
    else
        media=(-drive "if=none,id=cd,media=cdrom,format=raw,readonly=on,file=${iso}"
               -device ide-cd,drive=cd,bootindex=0)
    fi

    log "[$name] QEMU を起動します (timeout ${BOOT_TIMEOUT}s, ${cpu_args[*]}, -m ${memory})"
    local case_start
    case_start=$(date +%s)
    qemu-system-x86_64 \
        -machine q35 \
        "${cpu_args[@]}" \
        -m "$memory" \
        "${QEMU_FW_ARGS[@]}" \
        "${media[@]}" \
        -nodefaults -vga std \
        -display none \
        -serial "file:${serial}" \
        -monitor "unix:${mon},server,nowait" \
        -no-reboot \
        >"$qemu_log" 2>&1 &
    QEMU_PID=$!

    if [ "$name" = "timer" ]; then
        # TIMER_MARK の出現時刻をホスト側で記録する (QEMU 起動直後から追跡)
        python3 "$TOOL" timer-watch "$serial" "${BOOT_TIMEOUT}" >"$watch_log" 2>&1 &
        WATCH_PID=$!
    fi

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
    if [ "$result" != "$expect" ]; then
        log "[$name] FAIL: 期待した結果は ${expect} ですが ${result} でした (${elapsed}s)"
        status=1
    elif [ "$expect" = "booted" ]; then
        log "[$name] 起動完了マーカーを検出 (${elapsed}s): $(grep -a "${BOOT_MARKER}" "$serial" | tr -d '\r' | head -1)"
    else
        log "[$name] 停止を検出 (${elapsed}s)"
        # パニック画面の描画完了を待つ
        sleep 2
    fi

    if [ "$status" -eq 0 ]; then
        case "$name" in
            cdrom|usb)
                sleep 1
                if take_screenshot "$mon" "$ppm" "$png"; then
                    log "[$name] screenshot: ${png}"
                    # pipefail により python3 の失敗がパイプライン全体の失敗になる
                    if ! python3 "$TOOL" check-image "$ppm" | sed "s/^/[${_script_name}] [$name] /"; then
                        status=1
                    fi
                else
                    log "[$name] FAIL: screendump が取得できませんでした"
                    status=1
                fi
                check_no_errors "$serial" "$name" || status=1
                ;;
            panic|stackoverflow)
                if [ "$name" = "panic" ]; then
                    if grep -aq "${PANIC_MARKER} kernel/src/main.rs:[0-9]*:[0-9]*: selftest: intentional panic" "$serial"; then
                        log "[$name] パニック出力: $(grep -a "${PANIC_MARKER}" "$serial" | tr -d '\r' | head -1)"
                    else
                        log "[$name] FAIL: 期待したパニック出力 (場所とメッセージ) がありません"
                        status=1
                    fi
                else
                    if serial_has "$serial" "CPU exception #DF double fault (vector 8)" \
                        && serial_has "$serial" "kernel stack overflow detected: stack 'kernel-main'"; then
                        log "[$name] 検出: $(grep -a "KERNEL PANIC:" "$serial" | tr -d '\r' | head -1 | cut -c1-200)"
                    else
                        log "[$name] FAIL: #DF によるスタックオーバーフローの検出ログがありません"
                        status=1
                    fi
                fi
                if ! serial_has "$serial" "kernel panic screen drawn"; then
                    log "[$name] FAIL: Kernel Panic 画面が描画されていません"
                    status=1
                fi
                if grep -aq "${BOOT_MARKER}" "$serial"; then
                    log "[$name] FAIL: パニック後に起動完了マーカーが出力されました"
                    status=1
                fi
                if take_screenshot "$mon" "$ppm" "$png"; then
                    log "[$name] screenshot: ${png}"
                    if ! python3 "$TOOL" check-panic-screen "$ppm" | sed "s/^/[${_script_name}] [$name] /"; then
                        status=1
                    fi
                else
                    log "[$name] FAIL: screendump が取得できませんでした"
                    status=1
                fi
                ;;
            memory)
                check_selftest_suite "$serial" memory "$name" no-skip || status=1
                if ! grep -aq "pages 4K=[0-9]* 2M=[0-9]* 1G=[1-9]" "$serial"; then
                    log "[$name] FAIL: 1GiB ページが使われていません (pdpe1gb, -m 3G)"
                    status=1
                fi
                ;;
            exceptions)
                check_selftest_suite "$serial" exceptions "$name" no-skip || status=1
                ;;
            timer)
                check_selftest_suite "$serial" timer "$name" no-skip || status=1
                wait "${WATCH_PID}" || true
                if [ -s "$watch_log" ] && ! grep -q '^FAIL' "$watch_log"; then
                    sed "s/^/[${_script_name}] [$name] /" "$watch_log"
                else
                    sed "s/^/[${_script_name}] [$name] /" "$watch_log" 2>/dev/null || true
                    log "[$name] FAIL: ホストの実時間との比較に失敗しました"
                    status=1
                fi
                WATCH_PID=""
                log "[$name] calibration: $(grep -a "timer: calibrated" "$serial" | tr -d '\r' | head -1 | sed 's/^.*timer: //')"
                ;;
            keyboard)
                # a, b, Enter, 右矢印 (拡張キー: E0 プレフィックス)
                python3 "$TOOL" sendkeys "$mon" a b ret right
                local expected=(0x1e 0x9e 0x30 0xb0 0x1c 0x9c 0xe0 0x4d 0xe0 0xcd) got _
                for _ in 1 2 3 4 5 6 7 8 9 10; do
                    got="$(grep -a "keyboard: scancode" "$serial" | sed 's/.*scancode \(0x[0-9a-f]*\).*/\1/' | tr '\n' ' ' | sed 's/ $//')"
                    [ "$got" = "${expected[*]}" ] && break
                    sleep 1
                done
                log "[$name] 受信したスキャンコード: [${got}]"
                if [ "$got" != "${expected[*]}" ]; then
                    log "[$name] FAIL: 期待値 [${expected[*]}] と一致しません"
                    status=1
                else
                    grep -a "keyboard: scancode" "$serial" | tr -d '\r' | sed "s/^/[${_script_name}] [$name]   /"
                fi
                check_no_errors "$serial" "$name" || status=1
                ;;
            idle)
                log "[$name] ${IDLE_SECONDS} 秒アイドルさせます"
                sleep "${IDLE_SECONDS}"
                if ! kill -0 "${QEMU_PID}" 2>/dev/null; then
                    log "[$name] FAIL: QEMU が終了しました"
                    status=1
                fi
                check_no_errors "$serial" "$name" || status=1
                if serial_has "$serial" "$HALT_MARKER"; then
                    log "[$name] FAIL: カーネルが停止しました"
                    status=1
                fi
                local beats
                beats="$(grep -ac "heartbeat: uptime=" "$serial" || true)"
                grep -a "heartbeat: uptime=" "$serial" | tr -d '\r' | sed "s/^/[${_script_name}] [$name]   /"
                if [ "$beats" -lt $(( IDLE_SECONDS / 10 - 1 )) ]; then
                    log "[$name] FAIL: ハートビートが少なすぎます (${beats} 回)"
                    status=1
                fi
                if take_screenshot "$mon" "$ppm" "$png"; then
                    python3 "$TOOL" check-image "$ppm" | sed "s/^/[${_script_name}] [$name] /" || status=1
                fi
                ;;
        esac
    fi

    if [ "$status" -ne 0 ]; then
        log "[$name] --- シリアルログ末尾 (${serial}) ---"
        tail -n 40 "$serial" 2>/dev/null | tr -d '\r' | sed 's/^/    /' || true
        log "[$name] --- QEMU ログ (${qemu_log}) ---"
        sed 's/^/    /' "$qemu_log" || true
    fi

    if kill -0 "${QEMU_PID}" 2>/dev/null; then
        python3 "$TOOL" monitor "$mon" "quit" >/dev/null 2>&1 || true
        sleep 1
    fi
    cleanup
    QEMU_PID=""
    WATCH_PID=""
    log "[$name] $( [ "$status" -eq 0 ] && echo PASS || echo FAIL ) ($(( $(date +%s) - case_start ))s)"
    return "$status"
}

failed=()
passed=()
all_start=$(date +%s)
for case_name in ${TEST_CASES}; do
    if run_case "$case_name"; then
        passed+=("$case_name")
    else
        failed+=("$case_name")
    fi
done

log "結果: passed=[${passed[*]:-}] failed=[${failed[*]:-}] (合計 $(( $(date +%s) - all_start ))s)"
if [ "${#failed[@]}" -ne 0 ]; then
    log "FAILED"
    exit 1
fi
log "ALL PASSED"

#!/usr/bin/env bash
# Smoke-сценарий на телефоне: запуск по intent-параметру и проверка строки результата.
#
#   scripts/device-smoke.sh [--variant debug|release] [--no-build] [--timeout SEC] <сценарий>
#
# Сценарии: hello (обе сборки), panic (только debug). Ожидания — в scripts/smoke_wait.py.
# Коды выхода: 0 — сценарий прошёл, 1 — сценарий упал, 2 — устройство не найдено,
# 3 — нет строки результата за таймаут, 4 — не собралось/не встало/не запустилось, 64 — неверный вызов.
set -euo pipefail

# shellcheck source=lib/common.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib/common.sh"

usage() {
    sed -n '2,9p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//' >&2
}

variant="debug"
should_build=true
timeout_seconds=30
scenario=""

while [[ $# -gt 0 ]]; do
    case "$1" in
        --variant) variant="${2:-}"; shift 2 ;;
        --no-build) should_build=false; shift ;;
        --timeout) timeout_seconds="${2:-}"; shift 2 ;;
        -h|--help) usage; exit "$EXIT_OK" ;;
        -*) log_error "неизвестный аргумент: $1"; usage; exit "$EXIT_USAGE" ;;
        *)
            if [[ -n "$scenario" ]]; then
                log_error "сценарий уже задан ($scenario), лишний аргумент: $1"
                exit "$EXIT_USAGE"
            fi
            scenario="$1"; shift ;;
    esac
done

if [[ -z "$scenario" ]]; then
    log_error "не задан сценарий"
    usage
    exit "$EXIT_USAGE"
fi
if ! package="$(package_for_variant "$variant")"; then
    log_error "вариант должен быть debug или release, а не '$variant'"
    exit "$EXIT_USAGE"
fi
if [[ ! "$timeout_seconds" =~ ^[0-9]+$ ]]; then
    log_error "--timeout ждёт целое число секунд, а не '$timeout_seconds'"
    exit "$EXIT_USAGE"
fi

serial="$(find_device_serial)" || exit "$EXIT_NO_DEVICE"

if [[ "$should_build" == true ]]; then
    build_apk "$variant" || exit "$EXIT_BUILD_OR_INSTALL_FAILED"
fi
install_apk "$serial" "$variant" || exit "$EXIT_BUILD_OR_INSTALL_FAILED"

since_epoch="$(device_epoch_seconds "$serial")"
start_activity "$serial" "$package" -e scenario "$scenario" || exit "$EXIT_BUILD_OR_INSTALL_FAILED"

log_step "ожидание FASTICAMERA_RESULT сценария $scenario (до $timeout_seconds с)"
exec python3 "$FASTICAMERA_ROOT/scripts/smoke_wait.py" \
    --serial "$serial" \
    --since-epoch "$since_epoch" \
    --scenario "$scenario" \
    --timeout "$timeout_seconds"

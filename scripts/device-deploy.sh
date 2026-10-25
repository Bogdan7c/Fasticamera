#!/usr/bin/env bash
# Сборка, установка и запуск Fasticamera на телефоне, затем живой logcat приложения.
#
#   scripts/device-deploy.sh [--variant debug|release] [--no-build] [--log-seconds N]
#
# --log-seconds N — показать N секунд логов и выйти (по умолчанию logcat идёт до Ctrl-C).
# Коды выхода: 0 — запущено, 2 — нет устройства, 4 — не собралось/не встало/не запустилось, 64 — неверный вызов.
set -euo pipefail

# shellcheck source=lib/common.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib/common.sh"

usage() {
    sed -n '2,7p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//' >&2
}

variant="debug"
should_build=true
log_seconds=""

while [[ $# -gt 0 ]]; do
    case "$1" in
        --variant) variant="${2:-}"; shift 2 ;;
        --no-build) should_build=false; shift ;;
        --log-seconds) log_seconds="${2:-}"; shift 2 ;;
        -h|--help) usage; exit "$EXIT_OK" ;;
        *) log_error "неизвестный аргумент: $1"; usage; exit "$EXIT_USAGE" ;;
    esac
done

if ! package="$(package_for_variant "$variant")"; then
    log_error "вариант должен быть debug или release, а не '$variant'"
    exit "$EXIT_USAGE"
fi
if [[ -n "$log_seconds" && ! "$log_seconds" =~ ^[0-9]+$ ]]; then
    log_error "--log-seconds ждёт целое число секунд, а не '$log_seconds'"
    exit "$EXIT_USAGE"
fi

serial="$(find_device_serial)" || exit "$EXIT_NO_DEVICE"

if [[ "$should_build" == true ]]; then
    build_apk "$variant" || exit "$EXIT_BUILD_OR_INSTALL_FAILED"
fi
install_apk "$serial" "$variant" || exit "$EXIT_BUILD_OR_INSTALL_FAILED"

since_epoch="$(device_epoch_seconds "$serial")"
start_activity "$serial" "$package" || exit "$EXIT_BUILD_OR_INSTALL_FAILED"

# Только наши записи и падения рантайма; чужой шум (CamX и др.) отрезан фильтром.
logcat_command=(adb -s "$serial" logcat -v time -T "${since_epoch}.000" -s Fasticamera:V AndroidRuntime:E)
if [[ -n "$log_seconds" ]]; then
    log_step "logcat приложения, $log_seconds с"
    timeout "$log_seconds" "${logcat_command[@]}" || true
else
    log_step "logcat приложения (Ctrl-C — выход)"
    exec "${logcat_command[@]}"
fi

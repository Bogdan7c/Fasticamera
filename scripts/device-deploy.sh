#!/usr/bin/env bash
# Сборка, установка и запуск Fasticamera на телефоне, затем живой logcat приложения.
#
#   scripts/device-deploy.sh [--variant debug|release] [--no-build] [--log-seconds N]
#                            [--white-resume-rule take_from_source|keep_last]
#
# --log-seconds N — показать N секунд логов и выйти (по умолчанию logcat идёт до Ctrl-C).
# --white-resume-rule R — выбрать правило возобновления белого (intent-параметр white_resume_rule): приложение
#   шлёт его движку как выбор человека, и он запоминается в файле белой на следующие запуски. Без параметра действует
#   запомненное, а без памяти — take_from_source. Допустимость значения проверяет приложение (при ошибке не стартует).
# Коды выхода: 0 — запущено, 2 — нет устройства, 4 — не собралось/не встало/не запустилось, 64 — неверный вызов.
set -euo pipefail

# shellcheck source=lib/common.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib/common.sh"

usage() {
    sed -n '2,11p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//' >&2
}

variant="debug"
should_build=true
log_seconds=""
white_resume_rule=""

while [[ $# -gt 0 ]]; do
    case "$1" in
        --variant) variant="${2:-}"; shift 2 ;;
        --no-build) should_build=false; shift ;;
        --log-seconds) log_seconds="${2:-}"; shift 2 ;;
        --white-resume-rule) white_resume_rule="${2:-}"; shift 2 ;;
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

if [[ -n "$white_resume_rule" && ! "$white_resume_rule" =~ ^[a-z_]+$ ]]; then
    log_error "--white-resume-rule ждёт take_from_source или keep_last, а не '$white_resume_rule'"
    exit "$EXIT_USAGE"
fi

serial="$(find_device_serial)" || exit "$EXIT_NO_DEVICE"

if [[ "$should_build" == true ]]; then
    build_apk "$variant" || exit "$EXIT_BUILD_OR_INSTALL_FAILED"
fi
install_apk "$serial" "$variant" || exit "$EXIT_BUILD_OR_INSTALL_FAILED"

since_epoch="$(device_epoch_timestamp "$serial")"
launch_extras=()
if [[ -n "$white_resume_rule" ]]; then
    launch_extras+=(-e white_resume_rule "$white_resume_rule")
fi
start_activity "$serial" "$package" "${launch_extras[@]}" || exit "$EXIT_BUILD_OR_INSTALL_FAILED"

# Только наши записи и падения рантайма; чужой шум (CamX и др.) отрезан фильтром.
logcat_command=(adb -s "$serial" logcat -v time -T "$since_epoch" -s Fasticamera:V AndroidRuntime:E)
if [[ -n "$log_seconds" ]]; then
    log_step "logcat приложения, $log_seconds с"
    timeout "$log_seconds" "${logcat_command[@]}" || true
else
    log_step "logcat приложения (Ctrl-C — выход)"
    exec "${logcat_command[@]}"
fi

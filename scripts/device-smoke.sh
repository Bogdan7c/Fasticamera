#!/usr/bin/env bash
# Smoke-сценарий на телефоне: запуск по intent-параметру и проверка строки результата.
#
#   scripts/device-smoke.sh [--variant debug|release] [--no-build] [--timeout SEC] [--artifacts-dir DIR]
#                           [--stages LIST] [--ab-window-ms MS] [--neutral-card] <сценарий>
#
# Сценарии (--timeout в скобках): hello, panic (debug), lifecycle, permission (60), motion, platform,
# render (120), render-lifecycle (400), ahb-import и gpu-fence (60); камера: camera-catalog (30, сверка с
# dumpsys), camera-stream (120), camera-evict (debug, 90; сам запускает системную камеру); поток 05:
# raw-stream (120), raw-stream-endurance (420), raw-stream-cpu-probe (debug, 60).
# Видоискатель 07: viewfinder (120; ISP за минуту: к/с, GPU, AWB, задержка, скриншоты; --neutral-card —
# ещё и «серая карта в центре вышла серой», телефон наведён на карту); viewfinder-latency (240; ABBA
# двух профилей baseline / tuned: 6 окон × 20 с = 120 с + запуск); viewfinder-stage-budget (~6 мин на строку, по умолчанию 9 строк = 3600;
# --stages levels,tone,whole_viewfinder — подмножество; --ab-window-ms 15000 — короткие окна для мс GPU;
# только на батарее, без кабеля); viewfinder-endurance
# (720; 10 минут, троттлинг); video-headroom-bench (720; стенд V4 16:9@60, 120 Гц — скрипт касается экрана).
# Шаги на телефоне — scripts/smoke_driver.py, ожидания — smoke_wait.py, smoke_checks*.py.
# raw-stream* и viewfinder снимают два скриншота на стадии viewfinder_live в каталог артефактов ВНЕ
# репозитория: --artifacts-dir DIR либо свежий `mktemp -d` (путь печатается).
# Validation-слой есть только в debug-сборке: в release отчёт пишет validation.mode=disabled.
# Подробности отчёта — строки FASTICAMERA_DETAIL в выводе.
# Коды выхода: 0 — сценарий прошёл, 1 — сценарий упал, 2 — устройство не найдено,
# 3 — нет строки результата за таймаут, 4 — не собралось/не встало/не запустилось, 64 — неверный вызов.
#
# Разрешение камеры у пакета должно быть выдано (release: adb shell pm grant dev.fasticamera
# android.permission.CAMERA); экран разблокирован.
#
set -euo pipefail

# shellcheck source=lib/common.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib/common.sh"

usage() {
    sed -n '2,27p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//' >&2
}

variant="debug"
should_build=true
timeout_seconds=30
artifacts_dir=""
stages=""
ab_window_ms=""
neutral_card=false
scenario=""

while [[ $# -gt 0 ]]; do
    case "$1" in
        --variant) variant="${2:-}"; shift 2 ;;
        --no-build) should_build=false; shift ;;
        --timeout) timeout_seconds="${2:-}"; shift 2 ;;
        --artifacts-dir) artifacts_dir="${2:-}"; shift 2 ;;
        --stages) stages="${2:-}"; shift 2 ;;
        --ab-window-ms) ab_window_ms="${2:-}"; shift 2 ;;
        --neutral-card) neutral_card=true; shift ;;
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
# Строки таблицы — только имена через запятую (их проверит Rust при initialize); ничего, что сломает adb shell.
if [[ -n "$stages" && ! "$stages" =~ ^[a-z_]+(,[a-z_]+)*$ ]]; then
    log_error "--stages ждёт имена строк через запятую (levels,tone,whole_viewfinder), а не '$stages'"
    exit "$EXIT_USAGE"
fi
if [[ -n "$stages" && "$scenario" != "viewfinder-stage-budget" ]]; then
    log_error "--stages имеет смысл только для viewfinder-stage-budget"
    exit "$EXIT_USAGE"
fi
# Длина окна A/B — только целое число мс; границы (длиннее прогрева) проверит Rust при initialize.
if [[ -n "$ab_window_ms" && ! "$ab_window_ms" =~ ^[1-9][0-9]*$ ]]; then
    log_error "--ab-window-ms ждёт целое число миллисекунд, а не '$ab_window_ms'"
    exit "$EXIT_USAGE"
fi
if [[ -n "$ab_window_ms" && "$scenario" != "viewfinder-stage-budget" ]]; then
    log_error "--ab-window-ms имеет смысл только для viewfinder-stage-budget"
    exit "$EXIT_USAGE"
fi
if [[ "$neutral_card" == true && "$scenario" != "viewfinder" ]]; then
    log_error "--neutral-card имеет смысл только для viewfinder"
    exit "$EXIT_USAGE"
fi
if [[ ! "$timeout_seconds" =~ ^[0-9]+$ ]]; then
    log_error "--timeout ждёт целое число секунд, а не '$timeout_seconds'"
    exit "$EXIT_USAGE"
fi

# Сценариям со скриншотами нужен каталог артефактов: без явного — свежий временный (вне репозитория).
if [[ -z "$artifacts_dir" ]] && scenario_takes_screenshots "$scenario"; then
    artifacts_dir="$(mktemp -d -t fasticamera-smoke-XXXXXX)" || {
        log_error "не удалось создать временный каталог артефактов"
        exit "$EXIT_USAGE"
    }
fi
if [[ -n "$artifacts_dir" ]]; then
    mkdir -p "$artifacts_dir" || {
        log_error "не удалось создать каталог артефактов '$artifacts_dir'"
        exit "$EXIT_USAGE"
    }
    log_step "каталог артефактов (скриншоты): $artifacts_dir"
fi

serial="$(find_device_serial)" || exit "$EXIT_NO_DEVICE"

if [[ "$should_build" == true ]]; then
    build_apk "$variant" || exit "$EXIT_BUILD_OR_INSTALL_FAILED"
fi
install_apk "$serial" "$variant" || exit "$EXIT_BUILD_OR_INSTALL_FAILED"

# Разрешение камеры возвращаем в любом исходе (в том числе по Ctrl+C). Ловушка ставится ДО сброса:
# частично выполненный сброс тоже надо откатить. Код выхода скрипта ловушка не меняет.
restore_permission_on_exit() {
    restore_camera_permission "$serial" "$package" ||
        log_error "не удалось вернуть разрешение камеры $package: выдайте вручную (adb -s $serial shell pm grant $package $CAMERA_PERMISSION)"
}
if scenario_resets_camera_permission "$scenario"; then
    trap restore_permission_on_exit EXIT
    trap 'exit 130' INT
    trap 'exit 143' TERM
    reset_camera_permission "$serial" "$package" || {
        log_error "не удалось сбросить разрешение камеры $package"
        exit "$EXIT_BUILD_OR_INSTALL_FAILED"
    }
fi

since_epoch="$(device_epoch_timestamp "$serial")"
start_activity "$serial" "$package" -e scenario "$scenario" ${stages:+-e stages "$stages"} \
    ${ab_window_ms:+-e ab_window_ms "$ab_window_ms"} ||
    exit "$EXIT_BUILD_OR_INSTALL_FAILED"

# Дополнительные флаги проверки результата (пустой массив — без флагов).
wait_options=()
if [[ "$neutral_card" == true ]]; then
    wait_options+=(--neutral-card)
fi
log_step "ожидание FASTICAMERA_RESULT сценария $scenario (до $timeout_seconds с)"
# Без exec: после проверки надо отработать ловушку восстановления разрешения.
if python3 "$FASTICAMERA_ROOT/scripts/smoke_wait.py" \
    --serial "$serial" \
    --since-epoch "$since_epoch" \
    --package "$package" \
    --scenario "$scenario" \
    --timeout "$timeout_seconds" \
    ${artifacts_dir:+--artifacts-dir "$artifacts_dir"} \
    "${wait_options[@]}"; then
    smoke_exit_code=0
else
    smoke_exit_code=$?
fi
exit "$smoke_exit_code"

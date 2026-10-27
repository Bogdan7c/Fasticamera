#!/usr/bin/env bash
# Smoke-сценарий на телефоне: запуск по intent-параметру и проверка строки результата.
#
#   scripts/device-smoke.sh [--variant debug|release] [--no-build] [--timeout SEC] [--artifacts-dir DIR] <сценарий>
#
# Сценарии: hello, lifecycle, permission, motion, platform, render, render-lifecycle, ahb-import,
# gpu-fence, camera-catalog, camera-stream, raw-stream, raw-stream-endurance (обе сборки),
# panic, camera-evict и raw-stream-cpu-probe (только debug).
# lifecycle сворачивает и возвращает приложение, permission сбрасывает и затем возвращает
# разрешение камеры нашего пакета; шаги на телефоне — scripts/smoke_driver.py,
# ожидания — scripts/smoke_wait.py и scripts/smoke_checks.py.
# render наблюдает 10 с показа (нужен --timeout 60); render-lifecycle сворачивает приложение
# 20 раз кнопкой HOME и 3 раза гасит экран (экран без блокировки; нужен --timeout 300);
# ahb-import и gpu-fence гоняют GPU-проверки на отдельном контексте (--timeout 60).
# camera-catalog читает каталог камер и сверяет его с `dumpsys media.camera` (--timeout 30);
# camera-stream проходит шаги открыть-поток-замер и подтверждает 60 к/с через NDK (--timeout 120);
# camera-evict сам запускает системную камеру, чтобы она отняла камеру у нас (--timeout 90).
# raw-stream (сессия 05) гонит минуту всю цепочку «камера -> лизинг кадров -> рендер -> экран» и судит
# частоту по меткам сенсора, ноль CPU-копий, утечки, показ и метаданные кадров (--timeout 120);
# raw-stream-endurance то же на 5 минут (--timeout 420); raw-stream-cpu-probe (debug) ~5 с доказывает, что
# счётчик CPU-копий живой (--timeout 60). raw-stream* делают два скриншота экрана на стадии viewfinder_live
# и сохраняют их (сырой вывод screencap и PNG) в каталог артефактов ВНЕ репозитория: --artifacts-dir DIR
# либо свежий `mktemp -d` (путь печатается). Скриншоты в репозиторий не кладём.
# Validation-слой есть только в debug-сборке: в release отчёт пишет validation.mode=disabled.
# Коды выхода: 0 — сценарий прошёл, 1 — сценарий упал, 2 — устройство не найдено,
# 3 — нет строки результата за таймаут, 4 — не собралось/не встало/не запустилось, 64 — неверный вызов.
set -euo pipefail

# shellcheck source=lib/common.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib/common.sh"

usage() {
    sed -n '2,26p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//' >&2
}

variant="debug"
should_build=true
timeout_seconds=30
artifacts_dir=""
scenario=""

while [[ $# -gt 0 ]]; do
    case "$1" in
        --variant) variant="${2:-}"; shift 2 ;;
        --no-build) should_build=false; shift ;;
        --timeout) timeout_seconds="${2:-}"; shift 2 ;;
        --artifacts-dir) artifacts_dir="${2:-}"; shift 2 ;;
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
start_activity "$serial" "$package" -e scenario "$scenario" || exit "$EXIT_BUILD_OR_INSTALL_FAILED"

log_step "ожидание FASTICAMERA_RESULT сценария $scenario (до $timeout_seconds с)"
# Без exec: после проверки надо отработать ловушку восстановления разрешения.
if python3 "$FASTICAMERA_ROOT/scripts/smoke_wait.py" \
    --serial "$serial" \
    --since-epoch "$since_epoch" \
    --package "$package" \
    --scenario "$scenario" \
    --timeout "$timeout_seconds" \
    ${artifacts_dir:+--artifacts-dir "$artifacts_dir"}; then
    smoke_exit_code=0
else
    smoke_exit_code=$?
fi
exit "$smoke_exit_code"

# shellcheck shell=bash
# Общие настройки и функции скриптов Fasticamera. Подключается через `source`, сам не запускается.
#
# Все пути и параметры можно переопределить переменными окружения — значения ниже лишь
# дефолты рабочей машины владельца (тулчейн из отчёта сессии 00).

# Корень репозитория — на уровень выше scripts/.
FASTICAMERA_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
export FASTICAMERA_ROOT

export ANDROID_HOME="${ANDROID_HOME:-$HOME/Android/Sdk}"
export JAVA_HOME="${JAVA_HOME:-/usr/lib/jvm/java-21-openjdk}"

# Модель телефона проекта (OnePlus 11). Скрипты выбирают устройство только этой модели.
FASTICAMERA_DEVICE_MODEL="${FASTICAMERA_DEVICE_MODEL:-CPH2447}"

# Коды выхода скриптов работы с телефоном. Те же числа — в scripts/smoke_wait.py.
readonly EXIT_OK=0
readonly EXIT_SCENARIO_FAILED=1
readonly EXIT_NO_DEVICE=2
readonly EXIT_NO_RESULT=3
readonly EXIT_BUILD_OR_INSTALL_FAILED=4
readonly EXIT_USAGE=64

log_step() {
    printf '==> %s\n' "$*" >&2
}

log_error() {
    printf 'ошибка: %s\n' "$*" >&2
}

# Имя пакета для варианта сборки (debug и release стоят рядом, решение владельца сессии 01).
package_for_variant() {
    case "$1" in
        debug) printf 'dev.fasticamera.debug\n' ;;
        release) printf 'dev.fasticamera\n' ;;
        *) return 1 ;;
    esac
}

# Путь к APK варианта после `gradlew assemble<Variant>`.
apk_path_for_variant() {
    # FASTICAMERA_APK_PATH позволяет подставить готовый APK (используется тестами скриптов).
    if [[ -n "${FASTICAMERA_APK_PATH:-}" ]]; then
        printf '%s\n' "$FASTICAMERA_APK_PATH"
        return 0
    fi
    printf '%s/android/app/build/outputs/apk/%s/app-%s.apk\n' "$FASTICAMERA_ROOT" "$1" "$1"
}

# Активность приложения (namespace фиксирован, не зависит от суффикса пакета).
readonly MAIN_ACTIVITY="dev.fasticamera.MainActivity"

# Находит единственный телефон нужной модели и печатает его serial.
# ANDROID_SERIAL, если задан, имеет приоритет. Код 1 — устройство не определено (причина в stderr).
find_device_serial() {
    if [[ -n "${ANDROID_SERIAL:-}" ]]; then
        if [[ "$(adb -s "$ANDROID_SERIAL" get-state 2>/dev/null)" != "device" ]]; then
            log_error "устройство ANDROID_SERIAL=$ANDROID_SERIAL не подключено"
            return 1
        fi
        printf '%s\n' "$ANDROID_SERIAL"
        return 0
    fi
    if ! command -v adb >/dev/null 2>&1; then
        log_error "adb не найден в PATH"
        return 1
    fi
    local matching_serials
    # Строки `adb devices -l`: "<serial> device ... model:CPH2447 ...". Офлайн и unauthorized не берём.
    matching_serials="$(adb devices -l | awk -v model="model:$FASTICAMERA_DEVICE_MODEL" \
        '$2 == "device" { for (i = 3; i <= NF; i++) if ($i == model) print $1 }')"
    local serial_count
    serial_count="$(printf '%s' "$matching_serials" | grep -c . || true)"
    if [[ "$serial_count" -eq 0 ]]; then
        log_error "устройство не найдено: нет подключённого телефона модели $FASTICAMERA_DEVICE_MODEL (adb devices -l)"
        return 1
    fi
    if [[ "$serial_count" -gt 1 ]]; then
        log_error "найдено $serial_count телефонов модели $FASTICAMERA_DEVICE_MODEL; выберите один через ANDROID_SERIAL"
        return 1
    fi
    printf '%s\n' "$matching_serials"
}

# Собирает APK варианта через Gradle wrapper.
build_apk() {
    local variant="$1"
    local gradle_task="assemble${variant^}"
    # В debug-APK кладётся validation layer (решение 34): без него Gradle не соберёт debug.
    if [[ "$variant" == "debug" ]]; then
        "$FASTICAMERA_ROOT/scripts/fetch-validation-layer.sh" || return $?
    fi
    log_step "сборка $gradle_task"
    (cd "$FASTICAMERA_ROOT/android" && ./gradlew --console=plain --quiet "$gradle_task")
}

# Ставит APK варианта на устройство (с заменой, данные приложения сохраняются).
install_apk() {
    local serial="$1" variant="$2"
    local apk_path
    apk_path="$(apk_path_for_variant "$variant")"
    if [[ ! -f "$apk_path" ]]; then
        log_error "нет APK $apk_path — соберите без --no-build"
        return 1
    fi
    log_step "установка $(basename "$apk_path") на $serial"
    adb -s "$serial" install -r "$apk_path" >/dev/null
}

# Время устройства «секунды.миллисекунды» эпохи: точка отсчёта для `logcat -T`, чтобы не читать
# старые записи и не очищать буфер logcat (очистка — изменение состояния системы).
# Миллисекунды обязательны: с точностью до секунды прогон, начатый в ту же секунду, в которую
# предыдущий напечатал FASTICAMERA_RESULT, читал чужую строку результата (дефект сессии 03).
# toybox `date` отдаёт %N — 9 цифр наносекунд; последние 6 отбрасываем.
device_epoch_timestamp() {
    local nanos_timestamp
    nanos_timestamp="$(adb -s "$1" shell date +%s.%N | tr -d '\r')"
    printf '%s\n' "${nanos_timestamp%??????}"
}

# Перезапускает Activity (-S: остановить старый процесс, чтобы конфиг процесса был свежим).
# Остальные аргументы — intent-параметры для `am start` (например, -e scenario hello).
# Интент — как у иконки лаунчера (MAIN + LAUNCHER): тогда возврат из фона лаунчер-интентом
# поднимает ту же задачу, а не создаёт новую Activity поверх (факт сессии 02, сценарий lifecycle).
start_activity() {
    local serial="$1" package="$2"
    shift 2
    log_step "запуск $package/$MAIN_ACTIVITY $*"
    local start_output
    start_output="$(adb -s "$serial" shell am start -S -W \
        -a android.intent.action.MAIN -c android.intent.category.LAUNCHER \
        -n "$package/$MAIN_ACTIVITY" "$@" 2>&1)"
    if grep -q -E '^(Error|Exception)' <<<"$start_output"; then
        log_error "am start: $start_output"
        return 1
    fi
}

# Разрешение, которое сбрасывает сценарий permission, и его флаги «решено пользователем».
readonly CAMERA_PERMISSION="android.permission.CAMERA"
readonly CAMERA_PERMISSION_USER_FLAGS=(user-set user-fixed)

# Сценарии, которым перед запуском нужен сброшенный доступ к камере (данные, не if по имени).
readonly SCENARIOS_RESETTING_CAMERA_PERMISSION=(permission)

# Успех (0), если сценарию нужен сброс разрешения камеры.
scenario_resets_camera_permission() {
    local candidate
    for candidate in "${SCENARIOS_RESETTING_CAMERA_PERMISSION[@]}"; do
        [[ "$candidate" == "$1" ]] && return 0
    done
    return 1
}

# Сценарии, которые делают скриншоты и потому нуждаются в каталоге артефактов (данные, не if по имени).
readonly SCENARIOS_TAKING_SCREENSHOTS=(raw-stream raw-stream-endurance viewfinder 3a)

# Успех (0), если сценарий делает скриншоты.
scenario_takes_screenshots() {
    local candidate
    for candidate in "${SCENARIOS_TAKING_SCREENSHOTS[@]}"; do
        [[ "$candidate" == "$1" ]] && return 0
    done
    return 1
}

# Сбрасывает разрешение камеры ТОЛЬКО у пакета приложения: отзыв и снятие флагов
# «пользователь решил», чтобы система снова показала диалог.
reset_camera_permission() {
    local serial="$1" package="$2"
    log_step "сброс разрешения камеры у $package"
    adb -s "$serial" shell pm revoke "$package" "$CAMERA_PERMISSION" >/dev/null &&
        adb -s "$serial" shell pm clear-permission-flags "$package" "$CAMERA_PERMISSION" \
            "${CAMERA_PERMISSION_USER_FLAGS[@]}" >/dev/null
}

# Возвращает разрешение камеры пакету приложения (после сценария, в любом исходе).
restore_camera_permission() {
    local serial="$1" package="$2"
    log_step "возврат разрешения камеры $package"
    adb -s "$serial" shell pm grant "$package" "$CAMERA_PERMISSION" >/dev/null &&
        adb -s "$serial" shell pm clear-permission-flags "$package" "$CAMERA_PERMISSION" \
            "${CAMERA_PERMISSION_USER_FLAGS[@]}" >/dev/null
}

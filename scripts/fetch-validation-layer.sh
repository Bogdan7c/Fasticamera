#!/usr/bin/env bash
# Скачивает и распаковывает validation layer Vulkan для debug APK (решение владельца 34).
#
#   scripts/fetch-validation-layer.sh          — подготовить слой (идемпотентно)
#   scripts/fetch-validation-layer.sh --print-dir — только напечатать каталог jniLibs со слоем
#
# Версия, URL и SHA-256 закреплены в android/validation-layer.toml. Архив лежит в кэше вне репозитория:
#   ${XDG_CACHE_HOME:-$HOME/.cache}/fasticamera/vulkan-validation-layers/<версия>/
# Если архив уже в кэше и SHA-256 совпал — сеть не нужна. Несовпадение SHA-256 — архив удаляется,
# скрипт завершается ошибкой (никогда не используем непроверенный бинарник).
#
# Переменные окружения (для тестов и нестандартных машин):
#   FASTICAMERA_VALIDATION_LAYER_METADATA — другой файл метаданных вместо android/validation-layer.toml;
#   XDG_CACHE_HOME — корень кэша.
#
# Коды выхода: 0 — слой готов; 5 — не скачался; 6 — SHA-256 не совпал; 7 — нет нужной утилиты
# или повреждены метаданные/архив; 64 — неверный вызов.
set -euo pipefail

# shellcheck source=lib/common.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib/common.sh"

readonly EXIT_DOWNLOAD_FAILED=5
readonly EXIT_CHECKSUM_MISMATCH=6
readonly EXIT_TOOLING_OR_METADATA=7

readonly METADATA_FILE="${FASTICAMERA_VALIDATION_LAYER_METADATA:-$FASTICAMERA_ROOT/android/validation-layer.toml}"
readonly CACHE_ROOT="${XDG_CACHE_HOME:-$HOME/.cache}/fasticamera/vulkan-validation-layers"

# Значение ключа из файла метаданных (`ключ = "значение"`). Пусто/нет ключа — ошибка метаданных.
metadata_value() {
    local key="$1" value
    value="$(sed -n -E "s/^${key}[[:space:]]*=[[:space:]]*\"([^\"]*)\".*$/\1/p" "$METADATA_FILE" | head -n 1)"
    if [[ -z "$value" ]]; then
        log_error "в $METADATA_FILE нет ключа $key"
        exit "$EXIT_TOOLING_OR_METADATA"
    fi
    printf '%s\n' "$value"
}

require_tool() {
    if ! command -v "$1" >/dev/null 2>&1; then
        log_error "нужна утилита $1, её нет в PATH"
        exit "$EXIT_TOOLING_OR_METADATA"
    fi
}

# SHA-256 файла (только хэш).
sha256_of() {
    sha256sum "$1" | awk '{ print $1 }'
}

# Проверяет архив; при несовпадении удаляет весь кэш версии и завершает скрипт кодом 6.
verify_or_purge() {
    local archive="$1" expected="$2" version_dir="$3" actual
    actual="$(sha256_of "$archive")"
    if [[ "$actual" != "$expected" ]]; then
        log_error "SHA-256 архива не совпал: ожидался $expected, получен $actual; кэш $version_dir очищен"
        rm -rf -- "$version_dir"
        exit "$EXIT_CHECKSUM_MISMATCH"
    fi
}

# Скачивает архив во временный файл рядом (атомарный mv: оборванная загрузка не станет «кэшем»).
download_archive() {
    local url="$1" destination="$2" partial="$2.partial"
    log_step "скачивание $url"
    if ! curl --fail --location --silent --show-error --output "$partial" "$url"; then
        rm -f -- "$partial"
        log_error "не удалось скачать $url"
        exit "$EXIT_DOWNLOAD_FAILED"
    fi
    mv -- "$partial" "$destination"
}

main() {
    if [[ $# -gt 1 || ( $# -eq 1 && "$1" != "--print-dir" ) ]]; then
        log_error "использование: fetch-validation-layer.sh [--print-dir]"
        return "$EXIT_USAGE"
    fi
    require_tool curl
    require_tool unzip
    require_tool sha256sum

    local version url expected_sha archive_member
    version="$(metadata_value version)"
    url="$(metadata_value url)"
    expected_sha="$(metadata_value sha256)"
    archive_member="$(metadata_value archive_member)"

    local version_dir="$CACHE_ROOT/$version"
    local archive="$version_dir/android-binaries-$version.zip"
    # Структура каталога jniLibs, которую ждёт AGP: <abi>/lib*.so.
    local jni_libs_dir="$version_dir/jniLibs"
    local layer_library="$jni_libs_dir/$archive_member"

    if [[ "${1:-}" == "--print-dir" ]]; then
        printf '%s\n' "$jni_libs_dir"
        return 0
    fi

    mkdir -p -- "$version_dir"
    if [[ -f "$archive" ]]; then
        log_step "архив найден в кэше, проверяю SHA-256"
    else
        download_archive "$url" "$archive"
    fi
    verify_or_purge "$archive" "$expected_sha" "$version_dir"

    if [[ ! -f "$layer_library" ]]; then
        log_step "распаковка $archive_member"
        mkdir -p -- "$(dirname "$layer_library")"
        # -j: без каталогов архива, потом кладём в нужное место; шаблон */<member> пропускает корневой каталог.
        local extract_dir
        extract_dir="$(mktemp -d -- "$version_dir/extract.XXXXXX")"
        if ! unzip -q -j -o "$archive" "*/$archive_member" -d "$extract_dir" || [[ ! -f "$extract_dir/$(basename "$archive_member")" ]]; then
            rm -rf -- "$extract_dir"
            log_error "в архиве нет $archive_member"
            exit "$EXIT_TOOLING_OR_METADATA"
        fi
        mv -- "$extract_dir/$(basename "$archive_member")" "$layer_library"
        rm -rf -- "$extract_dir"
    fi
    log_step "validation layer готов: $layer_library"
}

main "$@"

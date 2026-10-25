#!/usr/bin/env bash
# Проверки репозитория Fasticamera (без телефона).
#
#   scripts/ci-checks.sh all          — все стадии по порядку, итоговая таблица
#   scripts/ci-checks.sh <стадия>...  — только перечисленные стадии
#
# Стадии: fmt, clippy-host, clippy-android, test, scripts-test, boundaries, module-size, apk.
# Все стадии выполняются даже после провала одной — так за один прогон видно всё.
# Код выхода: 0 — все выбранные стадии прошли, 1 — хотя бы одна упала, 64 — неверный вызов.
set -uo pipefail

# shellcheck source=lib/common.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib/common.sh"

readonly ALL_STAGES=(fmt clippy-host clippy-android test scripts-test boundaries module-size apk)

# Предупреждения clippy (в т.ч. unwrap_used/expect_used/panic из workspace.lints) — ошибки.
readonly CLIPPY_ARGUMENTS=(--workspace --all-targets --all-features --locked -- -D warnings)

stage_fmt() {
    cargo fmt --all --check
}

stage_clippy_host() {
    cargo clippy "${CLIPPY_ARGUMENTS[@]}"
}

stage_clippy_android() {
    cargo clippy --target aarch64-linux-android "${CLIPPY_ARGUMENTS[@]}"
}

# Тесты дважды: без фич (как release) и со всеми фичами (как debug, с отладочными сценариями).
stage_test() {
    cargo test --workspace --locked && cargo test --workspace --all-features --locked
}

stage_scripts_test() {
    bash -n "$FASTICAMERA_ROOT"/scripts/*.sh "$FASTICAMERA_ROOT"/scripts/lib/*.sh &&
        python3 -m unittest discover -s "$FASTICAMERA_ROOT/scripts/tests" -t "$FASTICAMERA_ROOT/scripts/tests"
}

stage_boundaries() {
    python3 "$FASTICAMERA_ROOT/scripts/check-crate-boundaries.py"
}

stage_module_size() {
    python3 "$FASTICAMERA_ROOT/scripts/module-size-check.py"
}

stage_apk() {
    (cd "$FASTICAMERA_ROOT/android" && ./gradlew --console=plain --quiet assembleDebug assembleRelease)
}

run_stage() {
    local stage="$1"
    local function_name="stage_${stage//-/_}"
    log_step "стадия $stage"
    (cd "$FASTICAMERA_ROOT" && "$function_name")
}

main() {
    if [[ $# -eq 0 ]]; then
        sed -n '2,9p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//' >&2
        return "$EXIT_USAGE"
    fi
    local selected_stages=()
    if [[ "$1" == "all" && $# -eq 1 ]]; then
        selected_stages=("${ALL_STAGES[@]}")
    else
        local stage
        for stage in "$@"; do
            if [[ ! " ${ALL_STAGES[*]} " == *" $stage "* ]]; then
                log_error "неизвестная стадия '$stage'; есть: all ${ALL_STAGES[*]}"
                return "$EXIT_USAGE"
            fi
        done
        selected_stages=("$@")
    fi

    local failed_stages=()
    local summary_lines=()
    local stage
    for stage in "${selected_stages[@]}"; do
        if run_stage "$stage"; then
            summary_lines+=("  ok    $stage")
        else
            summary_lines+=("  FAIL  $stage")
            failed_stages+=("$stage")
        fi
    done

    printf '\nИтог ci-checks:\n' >&2
    printf '%s\n' "${summary_lines[@]}" >&2
    if [[ ${#failed_stages[@]} -gt 0 ]]; then
        log_error "упали стадии: ${failed_stages[*]}"
        return 1
    fi
    return 0
}

main "$@"

# Конвенции кода

- Комментарии/доки на русском: ключевая логика, инварианты, неочевидные решения; тексты UI английские.
- Линты: `[workspace.lints]` — `unsafe_op_in_unsafe_fn = deny`, clippy `unwrap_used/expect_used/panic = warn` (в CI `-D warnings`); в тестах разрешены через `clippy.toml`. Намеренное нарушение — `#[expect(clippy::..., reason = "...")]`, не `allow`. rustfmt `max_width = 120`.
- Каждый crate: `[lints] workspace = true`, поля пакета через `*.workspace = true`, зависимости через `workspace = true`.
- Новый crate обязан быть классифицирован в `scripts/crate-boundaries.toml` (иначе guardrail = ошибка); правила зависимостей — данные там, не код.
- `unsafe`/FFI — только в отдельных модулях с `// SAFETY:` (пример: `crates/fasticamera-app/src/logging/logcat_sink.rs`). Платформенный код — `#[cfg(target_os = "android")]`, нейтральная логика рядом и тестируется на ПК.
- Граница JNI: каждый экспорт через `jni_boundary::run_guarded` (паника → `PanicReport`), ответы — JSON со статусом в поле `status` (serde `tag = "status"`, snake_case); сбой JNI → `ThrowRuntimeExAndDefault`. Kotlin только маршрутизирует.
- Отчёты/итоги — enum с вариантами (absent/failed/panicked и т.д. не сливаются в bool).
- Конфиг нативной части: JSON от Kotlin (`RuntimeConfig`, `deny_unknown_fields`), дефолты в `android/gradle.properties` → BuildConfig; фиксирован на процесс. Фильтр логов обязан иметь уровень по умолчанию.
- Логи: только `tracing` с `target: "fasticamera::<модуль>"`; тег logcat `Fasticamera` — контракт со скриптами. Мимо фильтра (`logging::emit_unfiltered`) — только паника и строка `FASTICAMERA_RESULT`.
- Smoke-сценарий: логика в Rust (`crates/fasticamera-app/src/scenario/`), ожидания — `CHECKS` в `scripts/smoke_wait.py`; отладочные сценарии — за фичей `debug-scenarios`.
- Python-скрипты: stdlib, dataclass + чистые функции, отдельный `main`, коды выхода константами, конфиг в TOML рядом; тесты unittest end-to-end через subprocess и временные каталоги (фикстуры копируются, в репо не пишем).
- Bash: общие функции и коды выхода — `scripts/lib/common.sh`; `set -euo pipefail`; проверка `bash -n` в CI (shellcheck не установлен).

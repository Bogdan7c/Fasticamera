# Конвенции кода

- Комментарии/доки на русском: ключевая логика, инварианты, неочевидные решения; тексты UI английские.
- Линты: `[workspace.lints]` — `unsafe_op_in_unsafe_fn = deny`, clippy `unwrap_used/expect_used/panic = warn` (в CI `-D warnings`); в тестах разрешены через `clippy.toml`. Намеренное нарушение — `#[expect(clippy::..., reason = "...")]`, не `allow`. rustfmt `max_width = 120`.
- Каждый crate: `[lints] workspace = true`, поля пакета через `*.workspace = true`, зависимости через `workspace = true`.
- Новый crate обязан быть классифицирован в `scripts/crate-boundaries.toml` (иначе guardrail = ошибка); правила зависимостей — данные там, не код.
- `unsafe`/FFI — только в отдельных модулях с `// SAFETY:` (пример: `crates/fasticamera-app/src/logging/logcat_sink.rs`). Платформенный код — `#[cfg(target_os = "android")]`, нейтральная логика рядом и тестируется на ПК.
- Граница JNI: все нативные методы регистрируются в `JNI_OnLoad` (`fasticamera-app/src/jni_exports.rs`) через `RegisterNatives`, сигнатуры — макросом `jni::native_method!` (без экспортов `Java_*`). `jni`/`ndk-sys` разрешены только `android-shell` и `fasticamera-app` (guardrail `jni-ndk-only-in-shell`); нейтральные типы шва — в `shell-contract`.
- `NativeBridge` (initialize/runScenario): логика под `jni_boundary::run_guarded` (паника → `PanicReport`), ответы — JSON со статусом в `status` (serde `tag = "status"`, snake_case); сбой JNI → `ThrowRuntimeExAndDefault`. Колбэки оболочки (`ShellBridge`): политика `LogAndCountCallbackFailure` — исключение в Kotlin не бросается, висящее Java-исключение снимается, ошибка → лог + счётчик `native_callback_failures`. Kotlin только маршрутизирует и вызывает API платформы.
- Главный поток Android никогда не ждёт (только `try_send` в очередь), кроме `surfaceDestroyed` (ожидание квитанции ≤ `surface_release_timeout_ms`) и осознанного опроса вибромотора в `onCreate` (2–3 мс). Вызовы платформы (binder) — вне замков; команды Rust → Kotlin идут через поток `fasticamera-platform`.
- Отчёты/итоги — enum с вариантами (absent/failed/panicked и т.д. не сливаются в bool).
- Конфиг нативной части: JSON от Kotlin (`RuntimeConfig`, `deny_unknown_fields`), дефолты в `android/gradle.properties` → BuildConfig; фиксирован на процесс. Фильтр логов обязан иметь уровень по умолчанию.
- Логи: только `tracing` с `target: "fasticamera::<модуль>"`; тег logcat `Fasticamera` — контракт со скриптами. Мимо фильтра (`logging::emit_unfiltered`) — только паника и строка `FASTICAMERA_RESULT`.
- Smoke-сценарий: логика в Rust (`crates/fasticamera-app/src/scenario/`). Мгновенные (hello, panic) отвечают в `runScenario`; наблюдающие (`ObservingScenario`: lifecycle, permission, motion, platform) живут в маршрутизаторе, пишут стадии `FASTICAMERA_STAGE {json}` и итог позже. Ожидания — `CHECKS` в `scripts/smoke_wait.py` + `scripts/smoke_checks.py`; действия на стадии — таблица `SCENARIO_STEPS` в `scripts/smoke_driver.py`. Отладочные сценарии — за фичей `debug-scenarios`. Функциональные тесты сценариев — через настоящую очередь и маршрутизатор на fake-оболочке (`event_router_tests.rs`).
- Python-скрипты: stdlib, dataclass + чистые функции, отдельный `main`, коды выхода константами, конфиг в TOML рядом; тесты unittest end-to-end через subprocess и временные каталоги (фикстуры копируются, в репо не пишем).
- Bash: общие функции и коды выхода — `scripts/lib/common.sh`; `set -euo pipefail`; проверка `bash -n` в CI (shellcheck не установлен).

## GPU (с сессии 03)
- Каждый объект Vulkan — RAII `Owned*` из `gpu-vulkan::live_objects` (счётчик по видам); сырой `destroy_*` у вызывающих запрещён. Порядок уничтожения — порядком полей/владением; при таймауте GPU объекты намеренно утекают (лучше утечка, чем use-after-free), это видно в `live_objects()` и `detach_failures`.
- Пайплайн нельзя уничтожать раньше цели, чьи кадры на него ссылаются (`VUID-vkDestroyPipeline-00765`): `DisplayChain` держит цель и паттерн вместе.
- Ни одного бесконечного ожидания: таймауты насыщаются в `i64::MAX` нс (`frame_ring::timeout_nanos`), не `u64::MAX`; `vkDeviceWaitIdle` — только запасной путь без maintenance1.
- `extern "C"`/`#[link(` в `gpu-vulkan` — только `src/android/hardware_buffer_ffi.rs` (guardrail `[[source_rules]]` в `scripts/crate-boundaries.toml`).
- Validation: `ValidationMode::FULL` (с sync-валидацией через `VK_EXT_layer_settings`) в тестах и debug; любая ошибка = провал. Известные отступления — только списком VUID с причиной (`gpu.known_validation_deviations`, по умолчанию 02387 и 02384, решение 35), точное совпадение `pMessageIdName`.
- GPU-тесты на ПК — на реальном RADV; нет GPU → тест падает с понятным текстом, `FASTICAMERA_SKIP_GPU_TESTS=1` → явный пропуск; `FASTICAMERA_EXPECT_RADV=1` — проверить имя драйвера.
- Шейдеры: GLSL в `crates/gpu-vulkan/shaders/`, `glslc` NDK (`--target-env=vulkan1.3 -Werror -O`, затем `spirv-val`), путь — `FASTICAMERA_GLSLC` → `ANDROID_NDK_HOME` → `~/Android/Sdk/ndk/<fasticamera.ndkVersion>`; compute с `LocalSizeId` требует `maintenance4` (включена).
- GpuContext не пересоздаётся без нужды; instance, работавший с AHardwareBuffer, не уничтожается до конца процесса (бэклог п. 2).

## Рендер (с сессии 03)
- Маршрутизатор не держит surface: события → `RenderInbox`; квитанция освобождения — только поток рендера ПОСЛЕ detach. Бюджет: `max(acquire, teardown) + teardown ≤ render.release_receipt_budget_ms (450) < shell.surface_release_timeout_ms (500)` — проверяется конфигом.
- Сценарии, которые долго работают с GPU, — на рабочем потоке (`scenario/background_job.rs`) с дедлайном `scenarios.gpu_job_timeout_ms`; маршрутизатор только опрашивает.

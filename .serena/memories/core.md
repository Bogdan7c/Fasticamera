# Fasticamera — core (точка входа)

Короткая карта текущего состояния (граф ссылок, не журнал).

## Проект
- Личная камера для OnePlus 11 (CPH2447, salami), Rust + Vulkan (ash), в магазин не публикуется. Приватный репозиторий GitHub `Bogdan7c/Fasticamera`, ветка `main`. Пакет `dev.fasticamera` (debug: `dev.fasticamera.debug`).
- Цель: сырой поток с сенсора без копий в Vulkan, вся обработка своя, снаружи — клон камеры iOS 26 (Liquid Glass). Конечная цель владельца — паритет с iPhone по видео (решение 16), затем фото «как iPhone» (17); костяк обязан выполнять задел V1–V10.
- Действующий план — `user/foundation/` (README: решения 1–30, архитектура, владельцы состояния, инварианты, V1–V10, порядок сессий 00–17, общие правила, бэклог). Отчёты — `user/foundation/results/NN.md`.
- Сделано: 00 (GO: 60 к/с RAW без root), 01 (скелет, CI, smoke), 02 (шов Android ↔ Rust: `shell-contract` + `android-shell`, маршрутизатор событий, smoke lifecycle/permission/motion/platform). Следующая по маршруту — 03 (Vulkan core: устройство, swapchain 120 Гц на surface из шва, импорт AHB).
- Факты о телефоне: `mem:device/oneplus11-camera-facts`.

## Карта кода (сейчас)
- `crates/camera-contract` — пустой каркас словаря камеры (наполняется в 04).
- `crates/shell-contract` — нейтральный словарь шва: `ShellEvent` + ограниченная очередь (`shell_event_channel`, касания выбрасываются со счётчиком и `TouchGap`, критичные не вытесняются), `NativeSurface` + квитанция `SurfaceReleaseRequest`/`Waiter`, `TouchSample`, `DeviceOrientation`, `CameraPermission`, `ThermalReading` + `ThermalPolicy` (единственный владелец тепловых решений, V8), `HapticPattern`, трейты `ShellCommands`/`HapticsService`/`MotionSource`/`ThermalSource`/`ShellDiagnostics`, `MotionRing` (гироскоп, V3/V9), часы `MonotonicNanos` (касания) ≠ `BoottimeNanos` (датчики/камера).
- `crates/android-shell` — единственный (кроме app) с `jni`/`ndk-sys`. Нейтрально: `touch_decode`, `haptic_plan`, `permission_answer`, `orientation_classifier`, `surface_registry`, `accounting`, `config` (`ShellConfig`). `src/android/`: `natives` (RegisterNatives `dev.fasticamera.ShellBridge`), safety-модули `native_window`/`motion_event`/`sensor_thread`/`thermal_ffi`, `platform_calls` (поток команд → `ShellPlatform.kt`), `thermal_monitor`, `runtime` (`install`), `services` (`AndroidShellServices`).
- `crates/fasticamera-app` — composition root: `jni_exports` (`JNI_OnLoad` → RegisterNatives `NativeBridge` + оболочки), `shell_wiring` (установка оболочки, поток `fasticamera-router`), `event_router` (единственный получатель событий; в 02 — временный владелец surface, сразу подтверждает освобождение; в 03 место займёт рендер), `scenario/` (мгновенные hello/panic + наблюдающие lifecycle/permission/motion/platform), `runtime_config` (секции `shell`/`thermal_policy`/`scenarios` необязательны), `logging/`, `panic_report`, `result_line` (`FASTICAMERA_RESULT`, `FASTICAMERA_STAGE`).
- `android/` — `MainActivity` (полный экран под вырезом, `SurfaceView`, портрет), `ShellBridge` (колбэки → Rust), `ShellPlatform` (тонкие вызовы API: вибромотор, диалог разрешения, галерея, keep screen on), `NativeBridge`, `NativeConfig`.
- `scripts/` — CI, guardrail, ratchet, deploy/smoke (`smoke_wait.py`, `smoke_checks.py`, `smoke_driver.py`, `smoke_ui.py`), тесты в `scripts/tests/`.
- `tools/device-probe` — зонд сессии 00, отдельный проект.

## Где что
- Версии, профили сборки, варианты APK: `mem:tech_stack`.
- Команды (CI, тесты, деплой, smoke, коды выхода): `mem:suggested_commands`.
- Правила кода, границы JNI, логирование, конфиг, стиль скриптов: `mem:conventions`.
- Чеклист завершения задачи: `mem:task_completion`.

## Ключевые решения (детали — README плана)
- Без root; своё имя пакета (20). Камера — Camera2 NDK (22). RAW10 по умолчанию (21). Фото-видоискатель основной 4:3@60 через ключи CamX за `VendorCapabilities` (23).
- UI гибрид: экран камеры — свой Rust/Vulkan UI; настройки — Compose. Видоискатель только из RAW через наш ISP; AF/AE вендорские, AWB наши.
- Эталон «как iPhone» — парные съёмки с iPhone 17 Pro Max (25).
- Сессия 02: словарь шва — отдельный crate (26); `MotionEvent` целиком в Rust (27); хаптика — свой план через Vibrator, системные эффекты одобрены (28); ориентация — свой классификатор (29); для smoke permission владелец включил опцию разработчика (30).

## Ловушки окружения
- Буфер logcat `main` 256 КиБ: записи вытесняются за секунды. Читать потоком с `-T <epoch>` (в файл scratchpad), не `-d` постфактум; падения процесса — `logcat -b crash -d`. `logcat -c`/`-G` не трогать.
- Телефон — личный и в использовании: запускать только свои APK, скриншоты не снимать без нужды. `uiautomator dump` — только в stdout, наружу выводить лишь `resource-id`/`package`.
- Запускать Activity интентом лаунчера (`-a MAIN -c LAUNCHER`), иначе возврат из фона создаёт новую Activity поверх.
- `adb shell pm revoke/grant` на OxygenOS работают только с опцией разработчика «отключить мониторинг разрешений» (включена владельцем).
- `scripts/device-smoke.sh --no-build` ставит последний собранный APK — после правок сначала собрать.

## Процесс
- Правила `AGENTS.md`; Context7 перед кодом (для `jni-rs` его нет — источник истины исходники в `~/.cargo/registry`); широкий поиск — read-only субагентом Sonnet. Serena: в `.serena/project.yml` включён `rust` (с сессии 02; работает после перезапуска Serena).
- Коммиты только по команде владельца, без веток; push — по поручению. В `user/` только `.md`; сырые данные — `~/fasticamera-captures/`. Межплановый бэклог — `user/general_problems/general_backlog.md`.

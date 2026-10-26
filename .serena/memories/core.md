# Fasticamera — core (точка входа)

Короткая карта текущего состояния (граф ссылок, не журнал).

## Проект
- Личная камера для OnePlus 11 (CPH2447, salami), Rust + Vulkan (ash), в магазин не публикуется. Приватный репозиторий GitHub `Bogdan7c/Fasticamera`, ветка `main`. Пакет `dev.fasticamera` (debug: `dev.fasticamera.debug`).
- Цель: сырой поток с сенсора без копий в Vulkan, вся обработка своя, снаружи — клон камеры iOS 26 (Liquid Glass). Конечная цель владельца — паритет с iPhone по видео (решение 16), затем фото «как iPhone» (17); костяк обязан выполнять задел V1–V10.
- Действующий план — `user/foundation/` (README: решения 1–36, архитектура, владельцы состояния, инварианты, V1–V10, порядок сессий 00–17, общие правила, бэклог). Отчёты — `user/foundation/results/NN.md`. Межплановый бэклог — `user/general_problems/general_backlog.md` (п. 1: OxygenOS 60 Гц до первого касания; п. 2: уничтожение instance после импорта AHB убивает поток под validation).
- Сделано: 00 (GO), 01 (скелет, CI), 02 (шов Android ↔ Rust), 03 (Vulkan-ядро, поток рендера, импорт AHB, sync_fd, тестовая картинка на телефоне). Следующая по маршруту — 04 (Camera2 NDK).
- Факты о телефоне: `mem:device/oneplus11-camera-facts`.

## Карта кода (сейчас)
- `crates/camera-contract` — пустой каркас словаря камеры (наполняется в 04).
- `crates/shell-contract` — нейтральный словарь шва: `ShellEvent` + ограниченная очередь, `NativeSurface` + квитанция `SurfaceReleaseRequest`/`Waiter`, касания, ориентация, разрешение, тепло + `ThermalPolicy` (V8), хаптика, команды (`set_preferred_frame_rate`, `keep_screen_on`), `MotionSource`/`MotionRing`, часы `MonotonicNanos` ≠ `BoottimeNanos`. С 03: `vsync` (`VsyncTick`, `VsyncSlot` «самый свежий», `VsyncSubscription`, трейт `VsyncSource`) и `sync_fence` (`GpuWaitFence`/`GpuReleaseFence` — владение sync_fd; нейтрально, чтобы `camera-android` в 05 не зависел от `gpu-vulkan`).
- `crates/android-shell` — единственный (кроме app) с `jni`/`ndk-sys`. С 03: поток AChoreographer (`android/choreographer_thread.rs`, `postVsyncCallback` API 33, предпочитаемая линия кадра; нейтральное `vsync_timeline.rs`), `AndroidShellServices: VsyncSource` (поток на подписку).
- `crates/gpu-vulkan` (роль gpu) — Vulkan-ядро на ash 0.38: `context` (`GpuContext` Clone/Arc; профили `AndroidDisplay`/`Headless`/`HeadlessPresentation`; API 1.3, обязательны dynamicRendering, synchronization2, timelineSemaphore, maintenance4), `validation` (messenger → tracing, счётчики, `KnownValidationDeviations` — решение 35), `live_objects` (RAII `Owned*` + `LiveObjectSnapshot` по видам), `swapchain` (`DisplayTarget` владеет `NativeSurface`: preTransform=currentTransform, FIFO, пересоздание с `oldSwapchain`, present-fence-ы maintenance1, `VK_GOOGLE_display_timing`), `frame_ring`, `test_pattern` (целочисленно точный эталон), `offscreen` (readback), `sync` (импорт/экспорт SYNC_FD, timeline, `QueueSubmission`, диагностика `fence_order_check`), `hardware_buffer_cache` (нейтральный кэш импорта + GPU-контрольные суммы), `android/` (единственный `extern "C"`: `hardware_buffer_ffi.rs`; `ImportedHardwareBuffer`: RAW16 — sampled `R16_UINT` + `texelFetch`, RAW10 — `VkBuffer`; диагностика `ahb_roundtrip`). Шейдеры `shaders/*.{vert,frag,comp}` → `build.rs` (glslc NDK) → `shaders::blobs::<NAME>_<STAGE>`. Тестовые швы отказов — фича `fault-injection`.
- `crates/render-loop` (роль gpu, решение 31) — поток `fasticamera-render`: `mailbox` (`RenderInbox` для маршрутизатора; не переполняется), `surface_state` (квитанция только ПОСЛЕ detach), `backend` (`DisplayBackend`; реальный — `vulkan_backend::VulkanDisplayBackend`), `pacing` (интервалы только между соседними present_id, дыры отдельно, гистерезис периода, покрытие), `thread` (`RenderLoop::spawn`, `RenderStatus`, `VsyncFrameClock`, удержание экрана на переходах, паника итерации → `Faulted`), `config` (`RenderLoopConfig`). `vulkan_backend/present_time.rs`: desiredPresentTime = ожидаемый vsync − 0,5 периода.
- `crates/fasticamera-app` — composition root: маршрутизатор отдаёт surface в `RenderRoute` (`InboxRoute` или запасной `HoldAndConfirmRoute`, если GPU/vsync не поднялись), `DisplayState` хранит только факты; `render_wiring`, `gpu_config` (секция `gpu`), `gpu_services` (`RenderStatusSource`, `GpuDiagnostics`), `vulkan_diagnostics` (свой долгоживущий контекст, см. бэклог п. 2); сценарии `scenario/`: hello, panic, lifecycle, permission, motion, platform, render, render-lifecycle, ahb-import, gpu-fence (`background_job` — рабочий поток с дедлайном).
- `android/` — тонкая Activity (`SurfaceView`, портрет), `ShellBridge`, `ShellPlatform`; validation layer — только в debug APK (`android/validation-layer.toml`).
- `scripts/` — CI, guardrail (граф crate-ов + `[[source_rules]]` по исходникам), ratchet, deploy/smoke (`smoke_wait.py`, `smoke_checks.py`, `smoke_driver.py`, `smoke_ui.py`), `fetch-validation-layer.sh`, тесты в `scripts/tests/`.
- `tools/device-probe` — зонд сессии 00, отдельный проект.

## Где что
- Версии, профили сборки, варианты APK: `mem:tech_stack`.
- Команды (CI, тесты, деплой, smoke, коды выхода): `mem:suggested_commands`.
- Правила кода, границы JNI/GPU, логирование, конфиг, стиль скриптов: `mem:conventions`.
- Чеклист завершения задачи: `mem:task_completion`.

## Ключевые решения (детали — README плана)
- Без root; своё имя пакета (20). Камера — Camera2 NDK (22). RAW10 по умолчанию (21). Фото-видоискатель основной 4:3@60 через ключи CamX (23).
- UI гибрид; видоискатель только из RAW через наш ISP; AF/AE вендорские, AWB наши. Эталон «как iPhone» — iPhone 17 Pro Max (25).
- 02: словарь шва — отдельный crate (26), `MotionEvent` целиком в Rust (27), хаптика (28), ориентация (29), опция разработчика для permission (30).
- 03: `render-loop` (31), свои объявления NDK для AHB в `gpu-vulkan` (32), GLSL + glslc из NDK (33), validation layer Khronos только в debug (34), два известных отступления VUID для импорта RAW (35), частота показа по умолчанию 60 Гц (36).

## Ловушки окружения
- Буфер logcat `main` 256 КиБ: читать потоком с `-T <секунды.миллисекунды>` (целые секунды ловили строку прошлого прогона), падения — `logcat -b crash -d`. `logcat -c`/`-G` не трогать.
- Телефон — личный: запускать только свои APK, скриншоты без нужды не снимать.
- Запуск Activity — интентом лаунчера (`-a MAIN -c LAUNCHER`).
- `adb shell pm revoke/grant` — только с опцией разработчика (решение 30).
- `scripts/device-smoke.sh --no-build` ставит последний собранный APK.
- Экран: `KEYCODE_SLEEP` игнорируется; `KEYCODE_POWER` переключает (жать только при `mWakefulness=Awake`). После циклов экрана телефон на блокировке: системный диалог разрешения не виден, `permission` требует разблокировки владельцем.
- Фоновый процесс Oplus замораживает (`do_freezer_trap` в wchan) — не путать с зависанием, если приложение ушло с переднего плана.
- Стеки без root: `simpleperf record --app <пакет> -g` (debuggable); `debuggerd` требует root.

## Процесс
- Правила `AGENTS.md`; Context7 перед кодом (ash — только master, источник истины для 0.38 — исходники в реестре; `jni-rs` нет). Широкий поиск — read-only субагентом Sonnet; реализация — фоновыми пакетами Sonnet с непересекающимися зонами + независимое ревью. Serena: `rust` в `.serena/project.yml`.
- Коммиты только по команде владельца, без веток; push — по поручению. В `user/` только `.md`; сырые данные — `~/fasticamera-captures/`.

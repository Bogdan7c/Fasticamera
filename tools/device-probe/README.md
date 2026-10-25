# Зонд разведки телефона (`tools/device-probe`)

Инструмент сессии 00 плана `user/foundation/`. Это не продукт. Он переиспользуется после обновлений прошивки, чтобы перепроверить факты о камере: 60 к/с RAW, импорт в Vulkan, вендорскую обработку RAW. Итоги разведки — в `user/foundation/results/00.md`.

## Состав

- `app/` — Kotlin-приложение. Ведёт камеру через Java Camera2: только там можно задать вендорский `sessionType` и ключи по имени. Три варианта сборки отличаются лишь `applicationId`:
  - `control` = `dev.fasticamera.probe`;
  - `oneplus` = `com.oneplus.camera`;
  - `oppo` = `com.oppo.camera` — прошивка не даёт его поставить, см. отчёт.
- `native/` — Rust-библиотека `probe_native` (`jni`, `ndk-sys`, `ash`):
  - каталог камер через NDK;
  - `AImageReader` с GPU-usage → импорт `AHardwareBuffer` в Vulkan → compute-сумма против CPU;
  - swapchain Vulkan на входной surface HEVC-кодера.
- `pc/probe.py` — ПК-сторона: установка с проверкой `pm`, запуск сценария, сбор результатов, сводная таблица.
- `pc/experiments/*.json` — матрицы экспериментов. Каждая строка — одна попытка с причиной (`why`).

## Тулчейн (проверено 25.10.2026)

- JDK 21 (`/usr/lib/jvm/java-21-openjdk`).
- Android SDK в `~/Android/Sdk`:
  - platform 36;
  - build-tools 36.1.0;
  - NDK 30.0.16248370.
- Gradle 9.8.2 (wrapper), AGP 9.4.1 со встроенным Kotlin.
- Rust 1.97.1, таргет `aarch64-linux-android`, `cargo-ndk` 4.1.2.
- Версия NDK и API для Rust — в `gradle.properties` (`probe.ndkVersion`, `probe.rustPlatformApi`).

## Сборка одной командой

```sh
cd tools/device-probe
ANDROID_HOME=$HOME/Android/Sdk JAVA_HOME=/usr/lib/jvm/java-21-openjdk ./gradlew assembleDebug test
```

Собирает Rust-библиотеку (задача `cargoNdkBuild`) и три APK, затем гоняет JVM-тесты. Rust-тесты на ПК:

```sh
cd tools/device-probe/native && cargo test
```

## Запуск

```sh
python3 pc/probe.py install oneplus                 # сборка + adb install (сначала проверка pm path)
python3 pc/probe.py run oneplus catalog              # C1: что видит пакет
python3 pc/probe.py run oneplus stream camera=2 size=4096x3072 format=0x20 ae_fps=60 frames=600 content_every=50
python3 pc/probe.py matrix oneplus pc/experiments/c2-fps-bypass.json
python3 pc/probe.py run oneplus vkimport camera=2 size=4096x3072 format=0x20 ae_fps=60 usages=0x100
python3 pc/probe.py run oneplus codecsurface frames=300
python3 pc/probe.py run oneplus platform
python3 pc/probe.py run oneplus capture camera=2 scene=lamp && python3 pc/probe.py pull-captures oneplus
python3 pc/probe.py table ~/fasticamera-captures/probe-runs/<дата>
```

Параметры сценария `stream` — в `StreamExperimentConfig.fromArgs`:
- `camera`, `physical`, `size`, `format`;
- `frames`, `timeout_ms`;
- `template`, `session_type` (в т.ч. `0x8021`);
- `ae_fps`, `exposure_ns`/`iso`/`frame_duration_ns`;
- `req_keys`/`sess_keys` вида `имя:тип=значение;…` с типами `byte|int|long|float|int[]` (типы — из `dumpsys media.camera`, раздел Vendor tags);
- `result_keys`, `companion_yuv`;
- `content_every` — анализ содержимого каждого N-го кадра.

## Куда идут результаты

- Основной канал — файл `files/runs/<run>.jsonl` в данных приложения. ПК забирает его через `run-as` (debug-сборка).
  - Logcat (`FasticameraProbe`) — только для наблюдения: его кольцевой буфер на телефоне 256 КиБ, и CamX вытесняет записи за секунды.
- Прогоны сохраняются в `~/fasticamera-captures/probe-runs/<дата>/`, эталонные снимки C7 — в `~/fasticamera-captures/c7-reference/`. Это вне репозитория. В `user/` попадают только выводы.

## Подводные камни (факты сессии 00)

- **Экран.** Зонд запускается поверх экрана блокировки (`setShowWhenLocked`). Иначе Oplus замораживает фоновый процесс посреди потока, и кадры не приходят.
- **GPU_DATA_BUFFER.** Для RAW gralloc его не поддерживает. `AImageReader` создаётся без ошибки, но камера не отдаёт ни одного кадра. Рабочий usage — `GPU_SAMPLED_IMAGE` (0x100).
- **stride RAW10.** Для RAW10 `AHardwareBuffer.stride` — в байтах (5120 для ширины 4096).
- **Рычаги CamX на ширике.** `disableFPSLimits`/`overrideSensorFrameDuration` на ширике (камера 3) дают 0 кадров и, похоже, перезапуск HAL.

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

## Сценарий `shading`: карта затенения объектива (сессия 14f)

Снимает RAW10 из ПРОДУКТОВОЙ конфигурации режима камеры (RAW10 + пустой YUV-компаньон в сессии, ручной AE, непрерывный AF) на плоском поле (рассеиватель на объективе) и сохраняет по каждому кадру полную карту `STATISTICS_LENS_SHADING_CORRECTION_MAP`. Нужен, чтобы понять, почему углы видоискателя темнее центра на 1,0–1,7 EV.

Режимы `mode=`:

| mode | камера | буфер RAW10 | управление |
|---|---|---|---|
| `main43` | 2 | 4096x3072 | ключи CamX `disableFPSLimits=1` + `overrideSensorFrameDuration=16666667`, без AE fps |
| `main169` | 2 | 4096x3072 | AE fps [60,60] в сессии и запросах; HAL берёт режим 4096x2304, данные в верхних строках, ниже нули (валидные строки определяются по данным) |
| `wide43` | 3 | 4000x3000 | AE fps [60,60], без ключей CamX (ломают камеру) |
| `front30` | 1 | 2304x1728 | без fps и без ключей (AE [60,60] роняет провайдер камеры) |

Необязательные параметры: `frames=` (16, кадров на фазу), `settle=` (15, кадров успокоения нового запроса), `target=` (0.40, уровень центра), `phases=` (список через запятую, по умолчанию все).

Ход прогона: прогрев вендорским AE 1,5 с → подбор ручной экспозиции (до 3 корректировок; центр 10% x 10% по зелёному ≈ `target`, самый яркий блок 32x32 < 0.9; потолок выдержки — длительность кадра режима, остальное в ISO) → фазы:

- `base` — ручная экспозиция, карта запрошена, непрерывный AF;
- `map_off` — то же без запроса карты (RAW не должен измениться);
- `iso_high` — выдержка ÷4, ISO ×4;
- `ae_on` — вендорский AE;
- `focus_inf`, `focus_near` — AF выключен, фокус на бесконечность / на минимальную дистанцию (на фронтальной с фиксированным фокусом пропускаются с записанной причиной).

Во всех запросах NR и горячие пиксели выключены (как в паспорте C7), `SHADING_MODE` не трогается: продукт его тоже не трогает.

Файлы в `files/captures/<run>/` (забираются тем же `pull-captures`):

- `<mode>.json` — характеристики камеры, конфигурация запроса, журнал подбора экспозиции и по каждой фазе записи кадров (метка, выдержка, ISO, фокус, режимы, вендорский режим сенсора, усиления AWB, профиль шума, динамический чёрный/белый, полная карта затенения: `columns`, `rows`, `gains` — плоско, строки подряд, в точке `[R, G_even, G_odd, B]`);
- `<mode>_<phase>.blocks.f32` — float32 little-endian, среднее DN (чёрный НЕ вычтен) по блокам 32x32 пикселя отдельно для 4 позиций CFA буфера (строка, столбец) = (0,0), (0,1), (1,0), (1,1); раскладка `[4][blocks_y][blocks_x]`, только валидные строки;
- `<mode>_base.frame0.raw16`, `<mode>_base.frame1.raw16` — два полных кадра, плотный u16 little-endian без паддинга, только валидные строки (для оценки шума).

Каждый собранный кадр спарен со СВОИМ результатом по `SENSOR_TIMESTAMP`; кадры, пока обработка занята, возвращаются камере (колбэки камеры не ждут распаковку). Счётчики потерь — в `rig_counters` и `counters` каждой фазы.

```sh
python3 pc/probe.py run oneplus shading mode=main43          # таймаут по умолчанию 600 с
python3 pc/probe.py pull-captures oneplus ~/fasticamera-captures/shading-14f
~/.cache/fasticamera/isp-reference-venv/bin/python -I pc/shading_analyze.py ~/fasticamera-captures/shading-14f/captures/<run>
~/.cache/fasticamera/isp-reference-venv/bin/python -I pc/shading_analyze.py --selftest
```

Разбор (`pc/shading_analyze.py`) считает по каждому режиму и фазе: угол/центр сырого RAW и радиальный профиль (в разах и EV), остаточную неравномерность после карты при двух гипотезах геометрии (A: валидная область буфера растянута на весь активный массив; B: буфер — центральный кроп активного массива) с макс. отклонением и асимметрией верх/низ, изменчивость карты между кадрами и фазами, совпадение пикселей `base` и `map_off`, шум против предсказания по карте и профилю шума. Сводка печатается Markdown-таблицами и пишется в `shading_analysis.json` рядом с отчётами. Каждый режим снимается отдельным запуском (отдельный `<run>`): передайте разбору несколько каталогов сразу.

### Калибровка затенения по плоскому полю (`pc/shading_calibrate.py`, сессия 14f)

Из нескольких прогонов `shading` (фаза `base`, разные укладки на плоском поле) строит собственную таблицу усилений виньетирования по каждой камере: `device-profiles/<устройство>/lens-shading.json` (схема `fasticamera.lens-shading-calibration/1`, описание — `device-profiles/README.md`).

Код: точка входа `pc/shading_calibrate.py` (CLI), логика в пакете `pc/shading_calibration/` (core, loading, fitting, validation, output, fixture, selftest).

Метод: вычитание динамического чёрного по позициям CFA → деление на (белый − чёрный) → раскладка по каналам Camera2 `[R, G_even, G_odd, B]` (G_even — зелёный в чётных строках) → нормировка каждого канала на центр (4x4 блока) → среднее по укладкам → МНК-полином 2D полной степени 16 (по каналу) → `gain = max(модель) / модель` в узлах сетки 33x25 (все gain >= 1, максимум модели ищется по центрам блоков и узлам вместе). Степень 16, а не 8: на степени 8 ошибка у края кадра 9–28 %. Конвенция сетки: узел (c, r) лежит в пикселе `((W-1)/(columns-1)*c, (H-1)/(rows-1)*r)` активного массива, точки построчно, в точке 4 gain `[R, G_even, G_odd, B]`.

Что предупреждает отчёт: leave-one-out по каждой укладке (углы скорректированного G относительно центра, средний EV, макс. отклонение), проверка на отложенном прогоне (`--held-out`, из подгонки исключается) и кросс-проверка на DNG приложения (`--cross-bench`, другой стенд). Без экстраполяции: данные есть лишь до центров крайних блоков (16 px от края), а полином степени 16 на этих последних 16 px ненадёжен (угловой gain между степенями 12 и 16 отличался на ~17 %, шум подгонки там усиливается в десятки раз). Поэтому координаты узлов и поиск максимума модели зажимаются в прямоугольник центров крайних блоков (u и v независимо): узлы во внешних 16 px берут значение модели в ближайшей измеренной точке.

```sh
cd <корень репозитория>
F=~/fasticamera-captures/14f
~/.cache/fasticamera/isp-reference-venv/bin/python -I tools/device-probe/pc/shading_calibrate.py \
    --device-manufacturer OnePlus --device-model CPH2447 --session 14f --date 2026-11-23 \
    --bench "flat field: phone flat, 4 sheets of office paper on the lens, cold LED bulb above" \
    --camera 2 "$F/led-*/captures/*/main43.json" --held-out 2 "$F/led-dg/captures/064406-0515d0/main43.json" \
    --camera 3 "$F/led-*/captures/*/wide43.json" \
    --camera 1 "$F/led-front/captures/*/front30.json" \
    --cross-bench 2 "$F/app-anchor/17884.dng" --cross-bench 3 "$F/app-anchor/17886.dng" --cross-bench 1 "$F/app-anchor/17888.dng" \
    --output device-profiles/oneplus-cph2447/lens-shading.json
# маски раскрывает сам инструмент (в кавычках); отложенный прогон исключается из подгонки, даже если попал под маску
~/.cache/fasticamera/isp-reference-venv/bin/python -I tools/device-probe/pc/shading_calibrate.py --selftest
# фикстура плоского поля для тестов Rust (отложенный прогон, 256x192 «уменьшенный сенсор»):
~/.cache/fasticamera/isp-reference-venv/bin/python -I tools/device-probe/pc/shading_calibrate.py \
    --write-fixture "$F/led-dg/captures/064406-0515d0/main43.json" --fixture-dir test-assets/isp-model --fixture-name flat-field-main-heldout
```

Самотест (`--selftest`): синтетические укладки с известной разной по каналам асимметричной виньеткой и шумом; таблица должна совпасть с известной в узлах в пределах 1 %, gain >= 1, минимум по каналу ≈ 1. Заведомо неверные G_even/G_odd, конвенция сетки и степень обязаны ронять проверку.

## Куда идут результаты

- Основной канал — файл `files/runs/<run>.jsonl` в данных приложения. ПК забирает его через `run-as` (debug-сборка).
  - Logcat (`FasticameraProbe`) — только для наблюдения: его кольцевой буфер на телефоне 256 КиБ, и CamX вытесняет записи за секунды.
- Прогоны сохраняются в `~/fasticamera-captures/probe-runs/<дата>/`, эталонные снимки C7 — в `~/fasticamera-captures/c7-reference/`. Это вне репозитория. В `user/` попадают только выводы.

## Подводные камни (факты сессии 00)

- **Экран.** Зонд запускается поверх экрана блокировки (`setShowWhenLocked`). Иначе Oplus замораживает фоновый процесс посреди потока, и кадры не приходят.
- **GPU_DATA_BUFFER.** Для RAW gralloc его не поддерживает. `AImageReader` создаётся без ошибки, но камера не отдаёт ни одного кадра. Рабочий usage — `GPU_SAMPLED_IMAGE` (0x100).
- **stride RAW10.** Для RAW10 `AHardwareBuffer.stride` — в байтах (5120 для ширины 4096).
- **Рычаги CamX на ширике.** `disableFPSLimits`/`overrideSensorFrameDuration` на ширике (камера 3) дают 0 кадров и, похоже, перезапуск HAL.

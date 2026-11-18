# OnePlus 11 (CPH2447) — факты камеры и GPU (сессия 00, 2026-10-25)

Источник истины с доказательствами: `user/foundation/results/00.md`. Зонд для перепроверки после обновлений прошивки: `tools/device-probe` (README там же).

## Система
- OxygenOS 16.0.5.1301(EX01), Android 16 / SDK 36, загрузчик заблокирован, HAL Qualcomm CamX/CHI, `ro.hardware.camera=oemlayer`.
- Logcat main-буфер 256 КиБ — CamX вытесняет записи за секунды; данные зонда идут JSONL-файлом через `run-as`.
- Фоновый процесс Oplus замораживает (`isFrozen`) — Activity с камерой должна быть на переднем плане (`setShowWhenLocked`/`setTurnScreenOn` при запуске с ПК).

## Камеры и доступ
- Видны 0–3 (0 логическая 2+3, 1 фронт S5K3P9, 2 основная IMX890, 3 ширик IMX581). 4 (телевик) и 5 закрыты фреймворком: `SYSTEM_CAMERA` = `signature|privileged|role`; без root недоступны. Имя пакета на это не влияет.
- `com.oneplus.camera` ставится, но ничего не меняет; `com.oppo.camera` — `INSTALL_FAILED_SHARED_USER_INCOMPATIBLE: oplus named app is not match signature`. Решение владельца 20: имя `dev.fasticamera`.
- Публичный RAW: основная 4096×3072 (RAW16/RAW10/RAW_PRIVATE), ширик 4000×3000, фронт 2304×1728/1296; min frame 33 мс (объявлено 30 к/с). Max-resolution карты нет.

## 60 к/с RAW (без root)
- Основная 16:9@60: `CONTROL_AE_TARGET_FPS_RANGE=[60,60]` → режим сенсора 4 (4096×2304@60); буфер 4096×3072, данные в строках 0–2303, ниже нули (центральная вырезка 4:3). 5 мин = 18 000 кадров, 5 пропусков.
- Основная 4:3@60: CamX `org.quic.camera.manualExposure.disableFPSLimits` (byte, 0x804a0000)=1 + `overrideSensorFrameDuration` (int64, 0x804a0001)=16666667, без AE fps range (иначе режим 4); работает и с вендорским AE; `SENSOR_FRAME_DURATION` в результате ложный (33,3 мс) — темп только по SENSOR_TIMESTAMP.
- Ширик 4:3@60: AE `[60,60]` → режим 2 (4000×3000@60). Ключи CamX на ширике ломают поток (0 кадров, похоже перезапуск HAL).
- Вендорские режимы сессии (0x8021 и т.д.) для 60 к/с не нужны; constrained high-speed с RAW отвергнут; 50 Мп публично недоступен.
- Индекс режима сенсора в результате: `org.codeaurora.qcamera3.sensor_meta_data.current_mode` (int32).

## Camera2 NDK (сессия 04)
- 60 к/с через NDK подтверждены: основная 16:9@60 (AE [60,60] в параметрах сессии и запросе) 58,6 к/с в комнате (AE растягивает кадр до 17 мс), 4:3@60 (CamX-ключи только в запросе) 59,8 к/с, ширик 4:3@60 58,9, RAW16 16:9@60 58,4; фронт 30,0.
- Вендорские теги ищутся по имени `ACameraMetadata_getTagFromName` (API 35): disableFPSLimits 0x804a0000, overrideSensorFrameDuration 0x804a0001, is_qcfa_sensor 0x80330000 (на всех камерах). `ACameraManager_getTagFromName` в публичном NDK закрыт (`__ANDROID_VNDK__`).
- Углы обзора по длинной стороне: основная 72,5°, фронт 71,0°, ширик 102,0° (порог роли ширика 90°).
- **Логическая камера 0 не отдаёт финальных результатов запросов**: только частичные (`onCaptureProgressed`), `onCaptureCompleted` = 0; RAW-кадры идут ~30 к/с. Java-зонд даёт то же (`res_fps null`) — поведение HAL, не NDK. Важно для сессии 13 (переключение через логическую камеру).
- RAW-размеры в карте HAL шире, чем знала 00: логическая 0 ещё `raw10 4000×3000`, фронт ещё 2304×1296.

## Поток в Vulkan (сессия 05)
- RAW10 4:3@60 CamX через лизинг: 5 мин = 18 111 кадров, 60,04 к/с, пул 6 (на руках максимум 4), 0 исчерпаний; результат запроса приходит почти всегда раньше буфера (ожидание пары ~0).
- Возраст кадра при приходе в наш reader (начало экспозиции → колбэк): ~30 мс (27–43). Задержка до экрана: p50 67 мс debug / 83 мс release (наша часть — очередь показа и фаза тика vsync, вход для 07).
- Тепло 5 мин: статус none → none, запас 0,58 → 0,67, батарея +1,7 °C.
- Первые кадры после старта потока почти чёрные (AE не сошлась); через ~1,5 с нормальные.
- ~3 % кадров видоискатель вытесняет (камера 60,04 и экран 60,00 расходятся по фазе).
- sysfs Adreno (`/sys/class/kgsl/...`) без root — Permission denied: загрузку GPU так не снять.
- Reader-ы закрытых сессий удаляются после возврата кадров (camera-stream: 5 закрыто, 5 удалено).

## Видоискатель с ISP (сессия 07, 27.10.2026)
- В сессии из одного RAW-потока (наш режим, 4:3@60 RAW10) HAL отдаёт `SENSOR_NEUTRAL_COLOR_POINT` как `0/0` во всех трёх каналах — весь поток. Временная нейтраль до нашего AWB берётся из `COLOR_CORRECTION_GAINS` того же кадра (`render-loop` `isp_params::NeutralSource`).
- `BatteryManager.CURRENT_NOW` на OxygenOS 16 — в **мА**, а не в мкА, как в документации Android (≈ 490 при камере; `dumpsys battery`, блок Oplus «Battery current : 509» — те же мА). Конфиг `camera.scenarios.power.current_now_unit = milliamps`; проверка правдоподобия ловит неверную единицу. `CHARGE_COUNTER` — мкА·ч (≈ 1 952 000 при 57 %).
- Swapchain на SurfaceView даёт `DISPLAY_P3_NONLINEAR_EXT` + 10 бит (с `VK_EXT_swapchain_colorspace`); выбран P3.
- Время GPU прохода ISP (timestamp-запросы) ~9 мс debug / ~12 мс release, но отладочная мозаика 05 стоит те же ~13 мс: время «растягивается» до бюджета кадра — почти наверняка DVFS Adreno (частоту снять нельзя: perfetto `power/gpu_frequency` пуст, `/sys/class/devfreq` закрыт). Мс GPU на этом телефоне — не мера стоимости работы; мера — ток (мА) и вычитание A/B.
- Цена ISP (A/B тока, окна 60 с, 07a): весь видоискатель +119 ± 11 мА к камере без ISP (358 мА), из них статистика AWB ≈ 55 ± 12 мА; остальные стадии ≤ ~15 мА (часто «не различим»). Мс GPU стадий после `subgroupAdd` неотличимы от нуля (DVFS).
- 07a (буфер на 1 кадр): release ISP 59,88 к/с при сенсоре 60,03, вытеснений 0, 100 % кадров ровно один раз; задержка p50 77–81 мс (профиль «как было» — 69 мс: сход на ближнюю линию не срабатывает из-за DVFS-растянутого времени GPU, решение 57). ADPF время GPU не меняет.
- В RAW-only сессии `COLOR_CORRECTION_GAINS` = `[1,1,1,1]` (не только нейтральная точка `0/0`): баланса HAL нет, видоискатель зелёный до AWB 08. AWB HAL внутри считает (провайдер пишет `vendor.oplus.camera.awb.pregain`, ~3700 K), но в результат не отдаёт.
- Logcat: буфер `main` на этой прошивке ≈ 8–9 с (SystemUI ~90 КБ/с); записи нашего процесса во всплеске теряются молча (`__android_log_write` отвечает успехом) — решение 58.

## Вендорская обработка RAW
- Коррекция виньетирования уже в RAW (`lensShadingApplied=true`, карта 17×13, угол ×4,33) и НЕ отключается (`SHADING_MODE_OFF`, `enableShadingCorrection`, `EnableIdealRAW`, `RawCbSourceType` не меняют пиксели).
- Шумодава в RAW нет (NR OFF = HQ). Black 64 статический, white 1023, `dynamicBlackLevel` не отдаётся. CFA BGGR (3).
- OIS: характеристики объявляют только OFF, но основная по умолчанию отвечает ois_mode=1; явный OFF принимается.

## Vulkan / кодер
- Импорт без копий: `AImageReader` usage `GPU_SAMPLED_IMAGE` (0x100); `GPU_DATA_BUFFER` для RAW gralloc не выделяет (камера молча без кадров). RAW16 → `VkImage R16_UINT` storage; RAW10 → `VkBuffer` (AHB stride в байтах, 5120 для 4096). AHB props: format UNDEFINED, externalFormat 0. Acquire-fence приходит сигнальным; release через `AImage_deleteAsync` + sync_fd. Сумма GPU = CPU.
- Adreno 740, Vulkan 1.3.128; нет present_wait, cooperative_matrix.
- Vulkan swapchain прямо на входную surface `c2.qti.hevc.encoder.hdr`: 3840×2160, A2B10G10R10, цветовые пространства HDR10 PQ/HLG; 300/300 кадров, PTS через VK_GOOGLE_display_timing.
- Системная камера: 4K60 = op mode 0x8021, HEVC Main 8 бит SDR ~68 Мбит/с; HDR-видео только 30 к/с (0x80A7); Pro RAW = 0x8009 + SuperRAW на DSP.

## Показ и Vulkan (сессия 03)
- Swapchain на SurfaceView: `A2B10G10R10_UNORM_PACK32` + `SRGB_NONLINEAR`, 1080×2412, `currentTransform=Identity` (портрет), 5 картинок FIFO; есть `VK_EXT_swapchain_maintenance1` (present-fence-ы) и `VK_GOOGLE_display_timing`.
- OxygenOS держит uid приложения на 60 Гц до первого касания (`dumpsys display` → `frameRateOverride`), потом разрешает 120; при цели 60 Гц (решение 36) не мешает.
- `vkGetRefreshCycleDurationGOOGLE`: при attach 8,33 мс, через ~250 мс реальный период; шум в единицы мкс. Android хранит 10 последних записей времени показа (`MAX_TIMING_INFOS`) — забирать каждый кадр.
- desiredPresentTime ровно на vsync → 4–5 % кадров по 2 периода; с упреждением 0,5 периода — 1133/1133 ровных.
- Освобождение surface рендером: медиана 7–8 мс, максимум ≤ 56 мс (debug + validation).
- AHB RAW16/RAW10 4096×3072 выделяются `AHardwareBuffer_allocate` с `GPU_SAMPLED_IMAGE|CPU_WRITE_OFTEN`; импорт без копии работает, VUID 02387/02384 неизбежны (решение 35).
- Порядок «GPU ждёт sync_fd» наблюдается на Adreno (зазор 3–6 мс); работа fence_busy 500k итераций ≈ 60 мс.
- Под validation layer уничтожение instance, работавшего с AHB, иногда убивает поток (драйвер: деструктор `BsQtiMapperExtensions`) — бэклог п. 2.

## Платформа
- Гироскоп BMI26x 402 Гц, шкала BOOTTIME (как SENSOR_TIMESTAMP REALTIME). Rolling shutter skew: основная 4:3 13,66 мс, 16:9@60 15,97, ширик@60 16,28.
- Хаптика: примитивы TICK/LOW_TICK/CLICK/THUD не поддерживаются; готовые эффекты TICK/CLICK/HEAVY_CLICK/DOUBLE_CLICK — `YES` (сессия 02); амплитуда управляется. Sustained performance mode не поддерживается. Флеш ~1,8 ГиБ/с последовательно. 4 физических микрофона. Экран 60/90/120 Гц.
- Жизненный цикл (сессия 02): HOME → `onPause` → `surfaceDestroyed`; возврат интентом лаунчера → `onResume` **раньше** `surfaceCreated` → `surfaceChanged`. Возврат интентом, отличным от корневого, создаёт новую Activity поверх.
- Гироскоп через NDK `ASensorEventQueue` 400 Гц → 402 Гц, без пропусков; акселерометр 50 Гц → ~47 Гц. Тепло NDK `AThermal_*`: статус 0, запас ~0,5 в покое, подписка регистрируется.
- `adb shell pm revoke/grant` → `SecurityException` без опции разработчика «отключить мониторинг разрешений». Диалог разрешения — пакет `com.google.android.permissioncontroller`, id кнопок `com.android.permissioncontroller:id/permission_deny_button` и т.д.

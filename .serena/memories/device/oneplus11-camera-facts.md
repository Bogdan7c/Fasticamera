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

## Вендорская обработка RAW
- Коррекция виньетирования уже в RAW (`lensShadingApplied=true`, карта 17×13, угол ×4,33) и НЕ отключается (`SHADING_MODE_OFF`, `enableShadingCorrection`, `EnableIdealRAW`, `RawCbSourceType` не меняют пиксели).
- Шумодава в RAW нет (NR OFF = HQ). Black 64 статический, white 1023, `dynamicBlackLevel` не отдаётся. CFA BGGR (3).
- OIS: характеристики объявляют только OFF, но основная по умолчанию отвечает ois_mode=1; явный OFF принимается.

## Vulkan / кодер
- Импорт без копий: `AImageReader` usage `GPU_SAMPLED_IMAGE` (0x100); `GPU_DATA_BUFFER` для RAW gralloc не выделяет (камера молча без кадров). RAW16 → `VkImage R16_UINT` storage; RAW10 → `VkBuffer` (AHB stride в байтах, 5120 для 4096). AHB props: format UNDEFINED, externalFormat 0. Acquire-fence приходит сигнальным; release через `AImage_deleteAsync` + sync_fd. Сумма GPU = CPU.
- Adreno 740, Vulkan 1.3.128; нет present_wait, cooperative_matrix.
- Vulkan swapchain прямо на входную surface `c2.qti.hevc.encoder.hdr`: 3840×2160, A2B10G10R10, цветовые пространства HDR10 PQ/HLG; 300/300 кадров, PTS через VK_GOOGLE_display_timing.
- Системная камера: 4K60 = op mode 0x8021, HEVC Main 8 бит SDR ~68 Мбит/с; HDR-видео только 30 к/с (0x80A7); Pro RAW = 0x8009 + SuperRAW на DSP.

## Платформа
- Гироскоп BMI26x 402 Гц, шкала BOOTTIME (как SENSOR_TIMESTAMP REALTIME). Rolling shutter skew: основная 4:3 13,66 мс, 16:9@60 15,97, ширик@60 16,28.
- Хаптика: примитивы TICK/LOW_TICK/CLICK/THUD не поддерживаются; готовые эффекты TICK/CLICK/HEAVY_CLICK/DOUBLE_CLICK — `YES` (сессия 02); амплитуда управляется. Sustained performance mode не поддерживается. Флеш ~1,8 ГиБ/с последовательно. 4 физических микрофона. Экран 60/90/120 Гц.
- Жизненный цикл (сессия 02): HOME → `onPause` → `surfaceDestroyed`; возврат интентом лаунчера → `onResume` **раньше** `surfaceCreated` → `surfaceChanged`. Возврат интентом, отличным от корневого, создаёт новую Activity поверх.
- Гироскоп через NDK `ASensorEventQueue` 400 Гц → 402 Гц, без пропусков; акселерометр 50 Гц → ~47 Гц. Тепло NDK `AThermal_*`: статус 0, запас ~0,5 в покое, подписка регистрируется.
- `adb shell pm revoke/grant` → `SecurityException` без опции разработчика «отключить мониторинг разрешений». Диалог разрешения — пакет `com.google.android.permissioncontroller`, id кнопок `com.android.permissioncontroller:id/permission_deny_button` и т.д.

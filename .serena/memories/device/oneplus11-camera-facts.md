# OnePlus 11 (CPH2447) — факты камеры и GPU (снимок 2026-10-25, сессия планирования)

Источник: `getprop`, `dumpsys media.camera`, `cmd gpu vkjson`, `dumpsys display` по беспроводному adb, только чтение. Сессия 00 плана `user/foundation/` перепроверяет и дополняет экспериментами — после неё сверять с `user/foundation/results/00.md`.

- Система: OxygenOS 16.0.5.1301(EX01), Android 16 / SDK 36, загрузчик заблокирован, `user`-сборка, HAL Qualcomm CamX/CHI (`vendor_qti`), `ro.hardware.camera=oemlayer`.
- Белый список: `persist.camera.privapp.list=com.oneplus.camera,com.oppo.camera,com.oplus.engineercamera,com.oplus.camera`; установлены только `com.oplus.camera` и `com.oplus.engineercamera` (system).
- Камеры: 0 логическая (2+3); 1 фронт S5K3P9 (fixed focus); 2 основная IMX890; 3 ширик IMX581; 4 телевик IMX709 — `SYSTEM_CAMERA`; 5 логическая (2+3+4) — `SYSTEM_CAMERA`. Обычному приложению видны 0–3.
- Публичный RAW только биннинговый, 10 бит (white 1023, black 64), ≤30 к/с; RAW12 нет; один RAW-поток на сессию; `lensShadingApplied=TRUE`; OIS-управления нет; UHR-capability нет.
- Режимы сенсора (вендорская таблица `org.quic.camera2.sensormode.info.SensorModeTable`): IMX890 4096×2304@60/@120, 2048×1152@240, 8192×6144@24; IMX581 4000×3000@60; IMX709 3136×1760@60.
- Рычаги для 60 к/с и чистого RAW (не проверены): `org.quic.camera.manualExposure.disableFPSLimits`/`overrideSensorFrameDuration`, `StreamBasedFPS`, `customhfrfps`, `com.oplus.sensor.mode`, `org.codeaurora.qcamera3.sessionParameters.EnableIdealRAW`/`enableShadingCorrection`.
- GPU: Adreno 740, Vulkan 1.3.128; есть AHB external memory, external semaphore/fence fd, ycbcr, timeline, sync2, dynamic rendering, fp16/int8, 16bit storage, display_timing, swapchain_maintenance1, QCOM image processing; нет present_wait, cooperative_matrix.
- Экран 1440×3216 / 1080×2412, 60/90/120 Гц.
- Видео: `c2.qti.hevc.encoder` (4K@120 perf point, ≤160 Мбит/с), `c2.qti.hevc.encoder.hdr` (≤4096×2176, 4K@60, hdr-editing), `c2.qti.avc.encoder`; Dolby Vision `c2.qti.dv.encoder` закомментирован в `/vendor/etc/media_codecs_kalama_vendor.xml` (выключен). Гироскоп Bosch BMI26x до 400 Гц. Теги HDR в сенсоре: `inSensorSHDRMode`, `EnableSHDR`, `EnableMFHDR`, `com.oplus.DolExposureInfo` (не проверены).

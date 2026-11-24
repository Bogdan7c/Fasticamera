# Стенд насыщения приложения (16a, P4b)

Сценарий `gpu-saturation-bench`: цена работ GPU = пропускная способность при кадрах подряд (не мс в темпе vsync).

- Код: `crates/fasticamera-app/src/saturation_bench/` (catalog — имена работ как данные; runner — порядок/пауза/отмена/тепло без GPU;
  gpu_executor — настоящий GPU, работы по одной, проверка живых объектов Vulkan после освобождения; inputs — сенсор/вид/тон «как у продукта»;
  screen_scene — списки рисования PHOTO/CALIBRATE настоящим `camera-screen`; backdrop — фон из ISP; services — шов `SaturationGpu`;
  summary — строка результата). Сценарий: `scenario/gpu_saturation.rs`. Конфиг: `camera.scenarios.saturation` (`saturation_scenario_config.rs`).
- Шов GPU: `GpuServices::saturation` (`SaturationServices`), на телефоне = долгоживущий контекст `VulkanGpuDiagnostics` (+ карта теней из `shell_wiring`).
- Тишина продукта: камера закрывается `claim_camera_for_scenario` + `close`, ждём `Closed`; экран снят (SuspendForScenario); в результате `render.delta`.
- Тесты: `tests/saturation_gpu.rs` (RADV, все 14 работ), `event_router_saturation_tests.rs` (сценарий на подставном GPU),
  `scripts/tests/test_smoke_saturation.py`. Команда цифр: `cargo test -p fasticamera-app --release --test saturation_gpu -- --nocapture`.
- Известное: у `still` нет времён проходов (в `photo` нет timestamp-запросов); батарея (BatterySample) не снимается.

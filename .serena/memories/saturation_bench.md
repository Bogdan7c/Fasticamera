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

## Доработка стенда (16a, пакет A0)
- Группы работ для флага/конфига `--saturation-workloads`: `baseline` (14, УМОЛЧАНИЕ, порядок не меняется; `baseline_names()`), `ui_ablation` (7: `display_{calibrate,photo}_{glass_only,no_glass,...}`), `isp_ablation` (14: `viewfinder_awb_never|awb_every_frame|without_<стадия>|minimal`). Группа — поле `CatalogEntry::group`; `expand_name` раскрывает имя группы/работы. Фильтр абляций UI — чистая `saturation_bench::draw_filter::DrawFilter`; вариант видоискателя → стадии/каденция AWB — `viewfinder_variant::configure`. `viewfinder_minimal` = только Levels+Demosaic (4 отсчёта RAW на пиксель).
- Дайджест выхода (`gpu_vulkan::saturation::digest`: FNV-1a 64, `OutputDigest`, `WorkloadDigest` из частей с метками, `DigestError`): сырые байты блокирующим readback ПОСЛЕ окна и `wait_idle`, до освобождения работы (`GpuWorkloadExecutor::digest_of`), без `test-support` (для этого в gpu-vulkan снята cfg-защита с readback-модулей и `kit_test_support`). Части: viewfinder `image`; scopes `counters`+`picture` (ТОЛЬКО с `test-support`: ColorScopes не открывает их без фичи — на телефоне `unavailable`); display `target`; V4 `output`; still `image`+`thumbnail`. В результате: `report.digests` (`имя|16hex` или `имя|-`, только если влезает в `SUMMARY_BUDGET_WITH_DIGESTS_BYTES`, иначе `digests_in_detail_only`; таблица `table` не менялась), в `FASTICAMERA_DETAIL` у строки работы `digest{state, combined, parts[], frame_note}`; у видоискателя `frame_note` = был ли последний кадр со статистикой AWB.
- Сравнение прогонов: `python3 scripts/smoke_checks_saturation.py compare A.txt B.txt` (`parse_saturation_rows`/`compare_runs`).
- Запасной сенсор на телефоне: сценарий брал снимок статуса до первой публикации каталога движком (пустой каталог) → `Phase::AwaitingCatalog` + `saturation_bench::catalog_wait` + конфиг `catalog_wait_ms`; причина запасного набора описывает каталог (`describe_catalog`).
- Геометрия стекла в `SceneSummary`: сумма и объединение прямоугольников `bounds` групп (`glass_bounds_area_px`, `glass_union_area_px`) и доли экрана; `Measurement::cost` — цена кадра с 3 значащими цифрами, p95/max по шагам шкалы.

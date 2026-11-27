# Стенд насыщения и методика оптимизации GPU (сессия 16a)

**Зачем**: на Adreno мс GPU в темпе vsync — не цена работы (DVFS растягивает). Цена = 1 / пропускная способность при кадрах подряд без показа. Итоги сессии и цифры — `user/foundation/results/16a.md`.

## Правила работы с шейдерами (выстраданы в 16a, обязательны)
- **«Бит-в-бит на ПК» на Adreno ничего не гарантирует**: компилятор устройства иначе склеивает fma. Три пойманных случая: размер массива shared через константу специализации (миниатюра), слитый демозаик с `precise`, форма выражения блика стекла. Правило: ветку ставить вокруг дорогого, итоговую формулу оставлять прежней строкой, размеры — литералами. Каждую правку шейдера проверять **отпечатком выхода на телефоне**, чередуя «основа → новое → основа» в одном заходе.
- **Стенд — прибор цены, не приёмка.** После правки, меняющей набор вариантов шейдера, порядок работ или цикл кадра, — камерные сценарии на телефоне (`viewfinder`, `glass`, `screen`, `zoom`). 16a внесла и поймала так дефект: компиляция варианта шейдера ISP посреди сессии = 0,9 с остановки рендера (в отчёте видно по `latch_to_submit.max_ms` и `pipeline_variants.created_in_frame`).
- Компиляция compute-пайплайна видоискателя на Adreno 740 — 145–180 мс; драйвер между запусками не кэширует. Вариантов шейдера, возникающих посреди сессии, быть не должно (`isp-vulkan::viewfinder::variant_family`).
- Отрицательный результат — тоже результат: правка с выигрышем < ~2–5 % своей работы откатывается и записывается с цифрами.

## Код
- Ядро: `gpu_vulkan::saturation` — `SaturationBench` (слоты по кругу, сабмит подряд), трейт `SaturationWorkload`, `PassTimeLog`, `HostRawFrame` (синтетический RAW10 без тестовых фич), `digest` (FNV-1a 64 байтов выхода), `image_diff` (max |Δ| в кодах 10 бит, число пикселей с Δ ≥ 1 и ≥ 2), `output_pixels`.
- Работы: `isp_vulkan::saturation` (`ViewfinderSaturation`, `ScopesSaturation`, `HeadroomSaturation`, `StillSaturation`; `SceneContent::{Natural, UniformGray, Probe}` — пробник покрывает диапазон для max Δ), `ui_render_vulkan::saturation::UiFrameSaturation` (`FrameComposition`, `BackdropCadence`).
- Приложение: `crates/fasticamera-app/src/saturation_bench/` — `catalog.rs` + `catalog/<группа>.rs` (имена работ как данные; обход `catalog::entries()`, таблицы `GROUP_TABLES`), `runner`, `gpu_executor` (работы по одной, сверка живых объектов), `inputs` (сенсор, вид, тон как у продукта; калибровка настоящей камеры — ждёт каталог движка, `catalog_wait`), `screen_scene` (списки рисования PHOTO/CALIBRATE настоящим `camera-screen`), `compare_pairs`/`pair_comparison` (пара «вариант → эталон» → Δ в результате), `draw_filter`, `glass_ablation`, `viewfinder_tuning`, `headroom_tuning`, `still_variant`. Сценарий `scenario/gpu_saturation.rs`, конфиг `camera.scenarios.saturation`.
- Группы для `--saturation-workloads` (имя группы или работы): `baseline` (14, умолчание, порядок не менять), `ui_ablation`, `isp_ablation`, `isp_tuning`, `scopes_ablation`, `scopes_merging` (14r: слияние накоплений, отделено от `scopes_ablation` — та не влезала в строку результата), `scopes_pages` (14r: `scopes_luma_waveform`), `v4_tuning`, `still_tuning`. Каждая группа обязана влезть в одну строку logcat (`TooManyForOneResultLine`, тест `saturation_scenario_config_tests`). Стенд меряет ПРОХОД прибора на наибольшей частоте GPU; цена прибора в продукте — только током (`--surface calibrate_screen --stages color_scopes`), каденция 2 делит работу на кадр вдвое. Группа `glass_ablation` удалена в 14p (стекла на экранах нет); `ui_ablation` у PHOTO — `display_photo_shapes_only` / `display_photo_text_only`; учёт стекла проверяется на синтетических группах `saturation_bench/test_glass.rs`. Новая группа = новый файл `catalog/<группа>.rs` + регистрация в `catalog.rs` (enum, `ALL`, имя, `GROUP_TABLES`).
- Отпечаток: части с метками (viewfinder `image` + `awb_statistics`; scopes `counters` + `picture` через `ColorScopes::digest_last_output_blocking`; display `target`; V4 `output`; still `image` + `thumbnail`). Сравнивать ЧАСТИ, не общий (`compare` показывает, какие части различаются).
- Сравнение прогонов: `python3 scripts/smoke_checks_saturation.py compare A.log B.log`.
- Тишина продукта во время стенда: камера закрыта (`claim_camera_for_scenario` + `close`), экран снят; в результате `render.delta`.

## Запуск на телефоне
- `scripts/device-smoke.sh --variant release [--no-build] --timeout <с> --saturation-workloads <список> [--saturation-measure-ms MS --saturation-warmup-ms MS] --stop-app-after gpu-saturation-bench` (без `--stop-app-after` приложение остаётся на экране и греет телефон). На зарядке работает (ток в мА — нет).
- Основа для сравнения — сохранённая сборка (`scripts/` + APK) вне репозитория; `--no-build` ставит APK из дерева, где лежит скрипт.
- Журнал тепла: `python3 scripts/device_thermal_log.py record --out <файл.jsonl> --period-s 5 --duration-s N`, `summarize <файл>`. Зоны `gpuss-*` и `gpubusy` — справочные (в темпе vsync цену не показывают).

## Ток A/B на настоящем экране
`viewfinder-stage-budget --surface bare_viewfinder|photo_screen|calibrate_screen --stages color_scopes,ui_overlay,whole_product,display_120hz` (только с батареи; пар не меньше 3). Рычаг в рендере — `ViewfinderCommand::SuppressGpuWork(GpuWorkSuppression)`. **Ток не складывается из долей** (A/B показывает экономию в данной точке нагрузки) — приоритеты брать по мс при насыщении.

## После 14m
- Работы стекла CALIBRATE убраны из `glass_ablation` и `ui_ablation` (в разделе стекла нет); у `display_calibrate` пирамида ≈ 0.
- `scopes_waveform` = три плоскости `ParadeRule`: часть `counters` совпадает с `scopes_parade` — ожидаемо. Картинки всех приборов после 14m другие по замыслу (512 столбцов, масштаб столбца); `picture` `scopes_parade_gray` совпадает с базой только на ПК (там отсчёты делятся на столбцы ровно), на телефоне — нет.
- Телефон: отпечатки одной и той же сборки РАЗНЫЕ между прогонами в разное время (вход зависит от состояния на старте, не расследовано), внутри короткого окна стабильны. «База ↔ новое» сравнивать только чередованием в одном окне (новая → база → новая). Стабильные битовые проверки на телефоне: `target` `display_photo`, `counters` вектороскопов.
- Сохранённая база 14m (APK + scripts) — `~/fasticamera-captures/14m-base/` (вне репозитория).

## После 14p
- PHOTO без стекла: `display_photo` на Adreno 1,82 → 0,89–1,03 мс (пирамида 0,45 → 0, UI 1,08 → 0,58–0,65); у `display_photo*` пирамида 0, групп стекла 0. `target` CALIBRATE совпал с базой — путь раздела прежний.
- Сохранённая база 14p (APK обеих сборок + scripts коммита `534c2a8`) — `~/fasticamera-captures/14p-base/` (вне репозитория); логи прогонов — `~/fasticamera-captures/14p-phone/`.
- Быстрая база: `git checkout <коммит>` в основном дереве + `scripts/ci-checks.sh apk` (общий target, ≈ 1 мин), скопировать APK в `<база>/android/app/build/outputs/apk/<variant>/` и `scripts/` рядом, вернуться на `main`.

## После 14o (цена и паритет вида; итог — `user/foundation/results/14o.md`)
- Группы `look_cost` (10 работ: `look_neutral`, `look_product`, `look_tone_base_{legacy,agx,aces,filmic}`, пары колёс и тона «вкл/выкл», `diff_vs=` в итоге) и `look_parity` (16 работ `look_parity_{agx,aces,filmic,legacy}_{product,bands,wheels,tone}`). Каталог — `catalog/look_cost.rs` и соседний; виды — `saturation_bench/look_variant.rs` (`ParityLook`, `ParityCase`), прогон паритета — `look_parity_run.rs`, сводка — `parity_report.rs`. `WorkloadExtras` у строки: `parity`, `pipelines_created_during_run`, `scope_picture` (размер картинки `scopes_*` по настоящей раскладке CALIBRATE: 1011×827, вектороскоп 827×827).
- Вид продукта на стенде = активный пресет телефона (`ProductInputs::product_look`); сводка пишет `look: {preset, tone_base}`. `look_product` меняется вместе с пресетом владельца.
- Линейка паритета — `isp_vulkan::saturation::parity` (`ParityRuler`, `ParityMeasure`, сцены, `ErrorAllowance`); её же используют тесты ПК. Параметры паритета без виньетирования (`parity_params_with_look`). Допуск выбирает рецепт (`look_parity_run::allowance_for`): тронутые полосы → `NearNeutralBandsAmplification` 3e-5, иначе `Standard` 1e-5. Причина доказана: ULP-разница основы тона GPU/CPU × наклон ≈ 63 веса «не серый» `c²/(c²+F²)` у почти серых пикселей; тригонометрия вида точная. fp16 на GPU пишется с отбрасыванием хвоста (допуск — один шаг), rgb10a2 — к ближайшему (полшага).
- Бюджет строки результата: сводка не повторяет имена работ (`requested_count`, `requested_missing`); `initialize` отвергает список, чей отчёт не влезет в строку logcat (`TooManyForOneResultLine`, `summary::worst_case_summary_bytes` против `summary_budget_bytes()`). На практике — **одна группа за прогон** (`baseline,look_cost,look_parity` разом отвергается).
- Цифры Adreno 740 (14o): вид +2,7…3,1 мс из 7,8; основы против legacy AgX +1,8, ACES +3,2, filmic +2,6 (повтор до 0,02 мс); приборы 0,76…1,20 мс между прогонами (DVFS — сравнивать только парой подряд).
- Строки тока вида в `viewfinder-stage-budget --stages`: `look_layer`, `look_wheels`, `look_tone`, `look_tone_base_{agx,aces,filmic}`; в отчёте `look_rows` с отпечатком рецепта «вкл/выкл». Окно A/B обязано быть длиннее разогрева (`power.ab.warmup_ms` 10 000) — `--ab-window-ms 15000` минимум, пар ≥ 3; иначе отказ `initialize` (причина в отчёт пока не доходит — бэклог).

## Известное
- Стенд снимка подтверждает проявку маркерной работой (`2·слотов + 1` проявителей) — граница `photo` ради стенда не менялась.
- Цепочка V4: 65,9 → 51,7 мс точными приёмами при цели 4,2 — сессия 16b (`user/foundation/16b-video-chain-4k60.md`).

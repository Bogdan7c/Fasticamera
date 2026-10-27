# Сессия 07a: приёмка видоискателя на телефоне и закрытие сессии 07

Приложи этот файл и скопируй абзац ниже в сообщение.

> Выполни сессию 07a плана `user/foundation/`: доведи сессию 07 до закрытия прогонами на телефоне. Весь код 07 уже написан и закоммичен по указанию владельца до приёмки (`a778ab7`, «Сессия 07 (не закрыта)»); на ПК `scripts/ci-checks.sh all` = 0. Сначала прочитай `user/foundation/README.md` (решения 49–54), черновик `user/foundation/results/07.md` и `user/foundation/07-gpu-viewfinder-isp.md`, потом этот файл.

## Зачем отдельная подсессия

Сессия 07 разрослась: код, два независимых ревью, первый заход на телефон, исправления по его итогам. Контекст агента переполнился. Работа на ПК закончена и проверена; осталась приёмка на телефоне, отчёт, memories, коммит. Номер с буквой — по общему правилу README («тема требует отдельной вставки»).

## Состояние на начало 07a

- **Git:** код 07 закоммичен владельцем до приёмки (`a778ab7`, база `7d35ded`) — отступление от решения 44 по прямому слову владельца. Первым делом — `git status` (чисто) и `scripts/ci-checks.sh all` (ожидается 0).
- **Черновик отчёта** `results/07.md` — с фактами ПК и первого захода на телефон; дописать.
- **README** — решения 49–54 уже записаны.
- **Serena memories** обновлены не полностью: `device/oneplus11-camera-facts` — да (раздел «Видоискатель с ISP (сессия 07)»), `core`/`conventions`/`suggested_commands`/`tech_stack` — нет (список ниже).

## Что сделано в 07 и проверено на ПК (кратко)

- `isp-vulkan` (новый): видоискатель — слитый compute-проход по `isp-model` Preview, паритет ≤ ½ LSB, 0 пикселей > 1 LSB; статистика AWB в том же проходе (с 07 — быстрый путь через `subgroupAdd`, точное равенство с эталоном, RADV 1,47 → 0,46 мс); стенд V4: Menon 2007 на GPU бит-в-бит + временной шумодав.
- `gpu-vulkan`: публичный набор (решение 52), Display P3, ожидание fence камеры на `ALL_COMMANDS` (дефект с 05).
- `render-loop`: ISP-видоискатель, кадр камеры возвращается fence-ом своего сабмита, параметры ISP только своего потока, нейтраль — точка HAL или коэффициенты `COLOR_CORRECTION_GAINS` (HAL отдаёт точку `0/0`), команды стадий/мозаики/стенда/политики показа/ADPF, ADPF-подсказки (по умолчанию выключены), время GPU в оценке стоимости кадра, пропуск одного vsync для схода на ближнюю линию (решение 54; по умолчанию включён).
- `raw-frame-stream`: видоискатель с буфером на 1 кадр (решение 53; по умолчанию включён), пул 6 → 7.
- Сервис батареи (`PowerSource`; на OnePlus 11 `CURRENT_NOW` в мА — конфиг `current_now_unit = milliamps`), A/B тока (ABBA), сценарии `viewfinder`, `viewfinder-latency` (два профиля), `viewfinder-stage-budget` (`--stages`, `--ab-window-ms`), `viewfinder-endurance`, `video-headroom-bench`; автостарт видоискателя (решение 49).
- **ПК-стенд задержки** (`cargo test -p render-loop --release --test latency_profiles_bench -- --nocapture`, ~70 с), подставная камера 60,03 к/с с разбросом прихода 27–43 мс:

  | Профиль | Не «ровно один раз» | Вытеснено | Новых кадров/с | Сенсор → задуманный показ p50 / p95 |
  |---|---|---|---|---|
  | как было (Preferred, LatestWins) | 32,5 % | 16,6 % | 50,4 | 91,6 / 100,3 мс |
  | с исправлениями (продукт по умолчанию) | 0,00 % | 0 | 60,4 | 74,9 / 75,6 мс |

  На ПК не воспроизводятся: SurfaceFlinger и реальное время показа, DVFS Adreno, ADPF.

## Факты первого захода на телефоне (27.10, до исправлений буфера и планировщика)

- debug: ISP 59,8 к/с, Display P3, 10 бит, validation 0, задержка p50 60 мс.
- release: ISP 57,1 к/с из 60 (вытеснение `LatestWins`), задержка p50 71 / p95 91 мс; проход ISP ~12 мс, но и мозаика 05 ~13 мс — время GPU растягивается до бюджета (DVFS), мс GPU на этом телефоне — не мера стоимости, мера — ток.
- Таблица стадий (окна 15 с): AWB 3,3 мс (до оптимизации), остальные 0–0,6 мс; весь видоискатель ≈ +125 мА поверх ≈ 400 мА.
- Стенд V4 не дал результата: беспроводной `adb` потерял телефон посреди 10 минут.

## Подготовка телефона

- Разблокирован, шторка закрыта, на экране наше приложение; неподвижно, камерой на освещённую сцену; одна яркость.
- **Не на зарядке** (только беспроводной `adb`) для всех замеров тока и тепловых прогонов.
- Телефон не должен засыпать и рвать Wi-Fi (в 07 стенд оборвался). Проверять `adb devices -l` перед каждым долгим прогоном; длинные прогоны запускать в фоне и не трогать телефон.
- Остывание между тепловыми прогонами (10 минут ISP и стенд V4): статус тепла `none`, температура батареи как в начале.
- Скриншоты — только в scratchpad, личные данные на них (шторка, уведомления) не сохранять.

## Прогоны (по порядку, ≈ 1–1,5 ч)

1. `scripts/device-smoke.sh --timeout 120 viewfinder` (debug): к/с ISP = к/с сенсора, «ровно один раз», validation 0, P3, AWB, задержка. Посмотреть скриншоты (видоискатель, а не шторка).
2. `scripts/device-smoke.sh --variant release --timeout 120 viewfinder`: то же + мс GPU. Сравнить с 57,1 к/с и 71/91 мс первого захода.
3. `scripts/device-smoke.sh --variant release --timeout 240 viewfinder-latency`: A/B «как было» против «с исправлениями + ADPF». По итогу — **включать ли ADPF по умолчанию** (решение агента с цифрами; если меняет энергию заметно — вопрос владельцу) и записать вердикт.
4. Задержка «до/после» в один день: release `raw-stream` из сборки базы (`git worktree add --detach <scratchpad>/baseline 7d35ded`, `assembleRelease`, `device-smoke.sh --no-build` из worktree) чередуя с п. 2.
5. `scripts/device-smoke.sh --variant release --timeout 1200 --ab-window-ms 15000 viewfinder-stage-budget` — мс GPU всех стадий после оптимизации AWB; затем полными окнами (60 с) только `--stages awb_statistics,whole_viewfinder` для мА (≈ 12 мин). Сравнить статистику AWB `subgroup_when_supported` против `shared_memory_only` (конфиг `ViewfinderIspConfig::awb_reduction`), если цифры GPU это позволяют.
6. `scripts/device-smoke.sh --variant release --timeout 720 video-headroom-bench` — стенд V4, 10 минут, 120 Гц (скрипт сам касается экрана).
7. `scripts/device-smoke.sh --variant release --timeout 720 viewfinder-endurance` — 10 минут с ISP: тепло, ток, к/с, мс по минутам.
8. Регрессия (debug): `hello`, `panic`, `raw-stream`, `camera-stream`, `camera-evict`, `render`, `ahb-import`, `gpu-fence` (`--timeout` — в шапке `device-smoke.sh`). С 07 камера открывается при запуске — проверить, что некамерные сценарии не сломались. `render-lifecycle` (~7 мин) — если хватает времени.
9. По желанию владельца: `viewfinder --neutral-card` с серой картой в центре кадра.

Любой провал — причина, а не следствие (`AGENTS.md`): исправление + тест, который падает без него; затем повтор прогона.

## Критерий выхода (из 07)

- Живой видоискатель из RAW на телефоне, к/с видоискателя = к/с сенсора (с буфером на 1 кадр — каждый кадр ровно один раз, кроме сбросов дрейфа часов), паритет подтверждён, 0 CPU-копий.
- `BackdropSource` и статистика AWB готовы для 08 и 11 (сделано).
- Таблица «стадия → мс GPU → мА» в `results/07.md` по методике решения 45 (с честной пометкой про DVFS) + строка «весь видоискатель» и «камера без ISP».
- 10-минутный прогон: без троттлинга или с найденной и записанной причиной.
- Стенд V4: мс GPU по проходам на 4096×2304@60 и тепло за 10 минут.

## Закрытие сессии 07

1. Дописать `results/07.md`: снять пометку «ЧЕРНОВИК», статус, SHA итога, раздел «Проверено на телефоне», таблицы, задержка до/после, V4, 10 минут, известные ограничения, обновлённые memories, self-review. Решения 53–54 добавить в таблицу решений владельца.
2. Serena memories:
   - `core`: сессия 07 сделана; карта кода — `isp-vulkan` (viewfinder, quality, `awb_reduction`), набор `gpu-vulkan` (compute, storage_image, host_buffer, device_buffer, gpu_timing, compositor, backdrop, frame_sink), `render-loop` (ISP-видоискатель, `isp_params` + `NeutralSource`, команды, ADPF, `present_depth`, `RenderLoopParts.performance_hints`), `raw-frame-stream` (`ViewfinderPacing`, `poll_for_vsync`, `set_viewfinder_pacing`, пул 7), `shell-contract` (`power`, `performance_hint`, линии кадра в `VsyncTick`), `android-shell` (`power_sampler`, `detached_worker`, `performance_hint_ffi`), `fasticamera-app` (автостарт, `power_measurement`, сценарии видоискателя, `viewfinder_control`, `viewfinder_pacing_control`); «продукт камеру сам не открывает» — убрать; решения 49–54.
   - `conventions`: Лизинг (кадр ISP возвращается fence-ом своего сабмита; буфер на 1 кадр; одна выдача на показанный тик), Рендер (политика показа по умолчанию NearestFeasible + поздний захват + сход пропуском vsync; стоимость = CPU + GPU; ADPF), GPU (`collect`/`release_slot_after_fence` до перезаписи слота; пул запросов на слот; `FrameSink::abort_frame`; `is_backdrop_current`; варианты SPIR-V `_subgroup` по возможностям; ожидание fence камеры на `ALL_COMMANDS`; `fma`/деление в Vulkan неточны — точное деление через `divide_correctly_rounded`), правило замера «мс GPU на Adreno ≠ стоимость (DVFS), мера — мА».
   - `suggested_commands`: сценарии 07 и таймауты, `--stages`, `--ab-window-ms`, `--neutral-card`, ПК-стенд задержки.
   - `tech_stack`: новых внешних крейтов нет; NDK `APerformanceHint` (API 33/35); `libc` в `render-loop`.
   - `device/oneplus11-camera-facts`: обновить цифры AWB и к/с после прогонов.
3. `scripts/ci-checks.sh all` = 0, self-review диффа.
4. Решение 44: коммит «Сессия 07a: приёмка видоискателя на телефоне, закрытие 07» и `git push origin main` — только когда всё зелёное и критерий выхода закрыт или открытые пункты честно записаны как ограничения с причиной. Незакрытое — стоп-вопрос владельцу.

Memories: `mem:core`, `mem:conventions`, `mem:device/oneplus11-camera-facts`.

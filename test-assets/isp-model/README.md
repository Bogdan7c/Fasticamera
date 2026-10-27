# Фикстуры `isp-model` (сессия 06)

Маленькие фикстуры эталонного ISP. Формат: манифест `<имя>.json` (размер, число каналов, тип
отсчётов, описание, команда генерации, поле `extra`) + отсчёты `<имя>.bin` — little-endian,
порядок строк, каналы подряд (`u16le` или `f32le`). Читатель — `crates/isp-model/tests/support/mod.rs`,
писатель — `tools/isp-reference/asset_io.py`.

Эталоны считаются только на ПК, в отдельном окружении вне репозитория (установка — `mem:suggested_commands`):

```
uv venv ~/.cache/fasticamera/isp-reference-venv
uv pip install --python ~/.cache/fasticamera/isp-reference-venv/bin/python \
    rawpy numpy scipy pillow colour-science colour-hdri colour-demosaicing
```


## Демозаик

Фикстуры паритета качественного демозаика (Menon 2007 с шагом уточнения) с независимым эталоном
colour-demosaicing 0.2.7 (BSD-3-Clause), `demosaicing_CFA_Bayer_Menon2007(CFA, pattern, refining_step=True)`.

| Файл | Содержимое |
|---|---|
| `demosaic-scene.{json,bin}` | RGB-сцена 40×40 f32 (гладкий фон, цветные края, решётка, шум; seed в манифесте). Мозаику каждой раскладки строит тест (`test_patterns::mosaic_from_rgb`) |
| `demosaic-menon-{rggb,grbg,gbrg,bggr}.{json,bin}` | Результат эталона для сцены, прошедшей через CFA этой раскладки, RGB 40×40 f32, без обрезки |

Перегенерация (из корня репозитория):

```
~/.cache/fasticamera/isp-reference-venv/bin/python -I tools/isp-reference/make_demosaic_fixtures.py
```

Тест: `crates/isp-model/tests/demosaic_parity.rs` (допуск 1e-6; измеренное расхождение 1,2e-7 — один ULP f32).
Суммарный размер ≈ 100 КБ.

## Реальные кропы (C7 «лампа», основная камера)

Кропы 48×48 мозаики RAW из DNG набора C7 (00; сами снимки — вне репозитория) и их внешние эталоны.
В манифестах только числа для ISP (уровни, CFA, матрицы DNG, нейтраль) и префикс SHA-256 DNG;
путей, серийников и времени съёмки нет.

| Файл | Содержимое |
|---|---|
| `real-main-lamp-{gray-tile,towel,jar,tap-highlight}.{json,bin}` | Мозаика u16 (BGGR); `extra` — метаданные кадра, белая точка и матрица colour-hdri, средние LibRaw и его матрица `rgb_cam` |
| `real-main-lamp-*-xyz.{json,bin}` | XYZ D50 каждого пикселя по независимой цепочке: формулы уровней и баланса + colour-demosaicing (Menon 2007) + colour-hdri (DNG) с нормировкой ForwardMatrix как в Adobe DNG SDK |

Перегенерация (из корня репозитория; вывод детерминирован — побайтно тот же):

```
~/.cache/fasticamera/isp-reference-venv/bin/python -I tools/isp-reference/make_real_crops.py \
    --capture-dir ~/fasticamera-captures/c7-reference/captures/204802-f42af9 --stem 2_lamp \
    --out-dir test-assets/isp-model [--full-frame-dir <каталог вне репозитория>]
```

`--full-frame-dir` дополнительно пишет полный кадр для визуальной проверки
(`cargo run -p isp-model --release --example render_capture -- <каталог> full-main-lamp out.ppm`).
Полный кадр в репозиторий не кладётся.

Тест: `crates/isp-model/tests/real_crops.rs`.

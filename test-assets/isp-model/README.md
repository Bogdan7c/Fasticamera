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

## Белая точка (сессия 08)

Эталоны для `crates/isp-model/src/color/white_point.rs` и `DngColorProfile::neutral_for_white`.
Все числа лежат в поле `extra` манифеста (JSON хранит f64 без потерь); `.bin` пустой (0×0, `u16le`) —
формат отсчётов здесь не нужен.

| Файл | Содержимое |
|---|---|
| `white-point-reference.{json,bin}` | `forward` — (CCT, Duv) → uv/xy на сетке 9 температур (2500–10 000 К) × Duv {−0,02; 0; +0,02}: Робертсон 1968 (colour-science 0.4.7) и Ohno 2014 (для сведения); `inverse` — xy → (CCT, Duv) для 8 точек; `neutral` — нейтраль камеры (G = 1) для белых Планка 2000/2800/4000/5500/6500 К, двух смещений по Duv при 4000 К и D65 по `colour_hdri.xy_to_camera_neutral` (colour-hdri 0.2.6) на калибровке OnePlus 11 из `test_support.rs`; `conventions` — различия соглашений |

Перегенерация (из корня репозитория):

```
~/.cache/fasticamera/isp-reference-venv/bin/python -I tools/isp-reference/make_white_point_fixtures.py
```

Тест: `crates/isp-model/tests/white_point_reference.rs` (допуски и их причины — у констант теста).
Знак Duv — по CIE (плюс — выше кривой Планка, к зелёному). Размер ≈ 17 КБ.

## Плоское поле основной камеры (сессия 14f)

Эталон для проверки калибровки затенения `device-profiles/oneplus-cph2447/lens-shading.json`.

| Файл | Содержимое |
|---|---|
| `flat-field-main-heldout.{json,bin}` | u16le, 256x192, один канал: «уменьшенный сенсор» основной камеры (режим `main43`, активный массив 4096x3072, прогон `064406-0515d0`). Ячейка 2x2 (i, j) хранит четыре средних блока 32x32 (по 16 кадрам) блока (i, j) в ИХ позициях CFA буфера, то есть фаза CFA как у буфера (BGGR). Чёрный НЕ вычтен: динамический чёрный по позициям, белый уровень, CFA, исходный прогон и `scaled_from_active_array` — в `extra` манифеста |

Отложен от калибровки: в подгонку `lens-shading.json` не входил, поэтому на нём честно проверяется, что таблица выравнивает НЕ виденное поле.
Размер ≈ 98 КБ.

Перегенерация (из корня репозитория; прогон лежит вне репозитория):

```
~/.cache/fasticamera/isp-reference-venv/bin/python -I tools/device-probe/pc/shading_calibrate.py \
    --write-fixture ~/fasticamera-captures/14f/led-dg/captures/064406-0515d0/main43.json \
    --fixture-dir test-assets/isp-model --fixture-name flat-field-main-heldout
```

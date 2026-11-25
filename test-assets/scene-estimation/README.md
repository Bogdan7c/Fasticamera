# Фикстуры сцен ColorChecker (сессия 08)

Независимые эталоны статистики плиток под разными светами. Фикстура ЖИВАЯ: её читают стенды AE и белой в `render-loop`; собственного оценщика света в продукте больше нет (white-balance 03). Формат — тот же, что у `test-assets/isp-model/`:
манифест `<имя>.json` (размер, число каналов, тип отсчётов, описание, команда генерации, поле `extra`) +
отсчёты `<имя>.bin` (little-endian, порядок строк, каналы подряд). Писатель — `tools/isp-reference/asset_io.py`.

Эталоны считаются только на ПК в отдельном окружении вне репозитория (установка — см. README `test-assets/isp-model`):
colour-science 0.4.7 и colour-hdri 0.2.6 (BSD-3-Clause). Rust-кода в расчёте нет — поэтому проверка не круговая.

## Сцены ColorChecker под разными источниками

| Файл | Содержимое |
|---|---|
| `awb-scenes.{json,bin}` | 24 сцены статистики плиток 32×24 (по 4 вида × 6 источников). `.bin`: u16, высота 24 (сцены в порядке `extra.scenes`), ширина 768 плиток по строкам, 3 канала — квантованные отсчёты ячейки `(q_r, q_g, q_b)` (0…4095, как `isp_model::statistics::quantize_sample`); `65535` — плитка пересвечена целиком. Суммы статистики тест восстанавливает по контракту: `sum_r = q_r·ячеек`, `sum_g = 2·q_g·ячеек`, `used = ячеек` (`extra.cells_per_tile`) |

Источники (`extra.lights`): чёрное тело 2800 К и 4000 К, CIE D55, D65, F2, F11. Ожидаемый свет (цветность, CCT и Duv по Робертсону, знак Duv по CIE) — в `extra.lights` и в `extra.scenes[].expected`.

Виды сцен: `checker` (все 24 образца поровну, 6 нейтральных), `dominant_green` (21 из 24 плиток раскладки — зелёные образцы, 3 — нейтральные), `no_neutrals` (только цветные образцы), `single_colour` (весь кадр — образец foliage). Раскладка: образцы 6×4 по 5×6 плиток (столбцы 0–29), столбец 30 — тёмные плитки, столбец 31 — пересвеченные.

Камера — OnePlus 11 (матрицы `ColorMatrix1/2`, `CameraCalibration1/2` в `extra.calibration` — те же числа, что `crates/isp-model/src/test_support.rs`); отклик плитки считает `colour_hdri.matrix_XYZ_to_camera_space`.

Перегенерация (из корня репозитория; вывод детерминирован):

```
~/.cache/fasticamera/isp-reference-venv/bin/python -I tools/isp-reference/make_awb_scene_fixtures.py
```

Читают: `crates/render-loop/tests/awb_support/mod.rs` (общая загрузка), `crates/render-loop/tests/vulkan_awb_scene.rs`
(сцена как мозаика RAW 64×48 через боевой путь на RADV: ISP → статистика → белая из указания → изображение) и стенды AE. Размер ≈ 125 КБ.

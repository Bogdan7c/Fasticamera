# Ассеты ui-draw: шрифт и иконки

Всё здесь — вход для `build.rs` (решения 81, 87, 88 плана `user/foundation/`). Что и как запекать, задаёт
`bake.json`; в приложение попадает только готовый SDF-атлас и таблицы, сам шрифт и SVG не встраиваются.
Ассеты Apple (SF, SF Symbols) не используются (решение 12).

Снимок источников: 19 ноября 2026.

## Шрифт

| Файл | Источник | SHA-256 |
|---|---|---|
| `fonts/Inter-VF.ttf` | Inter 4.001, вариативный (оси `opsz` 14–32, `wght` 100–900), репозиторий `google/fonts`, путь `ofl/inter/Inter[opsz,wght].ttf`, коммит `cc5661d43cd4f9aad5567033a8e15ca28dc35c86` (последний по `ofl/inter` на дату снимка) | `29160a80ff49ddcab2c97711247e08b1fab27a484a329ce8b813d820dc559031` |
| `fonts/OFL.txt` | Лицензия SIL OFL 1.1, Copyright 2020 The Inter Project Authors, оттуда же | `5b9321a4298cfeb6b34354164a1c3afc3db114569984c502b9b35d988fd58c57` |

Почему вариативный файл, а не статические инстансы из релиза `rsms/inter`: один файл вместо двух и больше,
оба нужных начертания получаются координатами осей прямо в `bake.json` (`regular` — `wght` 400,
`semibold` — `wght` 600), а добавить третье начертание — одна строка манифеста. Лицензия та же (OFL).

Ось `opsz` задана 14 (оптический размер «для текста»): подписи камеры 11–20 pt, это ближе к SF Text, чем к
Display. Оба начертания используют один `opsz` — разные оптические размеры внутри одного экрана дали бы
скачки ширины. Выбор — стартовый `[estimate]`, меняется правкой `bake.json`.

## Иконки

Исходные SVG лежат как есть; лицензия каждого набора — рядом.

| Файл | Источник | SHA-256 |
|---|---|---|
| `icons/phosphor/lightning.svg` | Phosphor Icons (MIT), `phosphor-icons/core`, `assets/regular/`, коммит `2b75f3ad12b420c9504ef05df8d2564a28f8500e` | `3a50c776266b21812f8ec5b5a841a81f54aa942569a31eb03a36fb0a5bfbe66e` |
| `icons/phosphor/lightning-slash.svg` | там же | `e0444fc03c39d830ed46d38c2a633b01c69fcb7f740022aafe07cad04a66146f` |
| `icons/phosphor/lightning-a.svg` | там же — автовспышка (молния с «A»), решение 153 | `e4df6e557f2f0aa125d2ca1bf3b274ebae6b26e1d7e099a5a74c328dfcaade1f` |
| `icons/phosphor/camera-rotate.svg` | там же | `c7f308fd058ac9c62cf8164ea5d4ae6e5513199cf348d70d57a8e1a6f62c8ee3` |
| `icons/phosphor/caret-up.svg` | там же | `bccc26004e73e5de783ac6dde607e3671e776261d81972ccdbd296b4aca21ca9` |
| `icons/phosphor/sun.svg` | там же | `8df1c7b747774f8b2c1fe353b1b785c74aac35bfa50f2ed7d8d9a85a4191b2ac` |
| `icons/phosphor/LICENSE` | MIT, Copyright 2023 Phosphor Icons | `b5b1f1da112d18ea2147decfd48ddc1bf2b5aeb6c265381579340e95b15a2bb2` |
| `icons/lucide/grid-3x3.svg` | Lucide (ISC), `lucide-icons/lucide`, `icons/`, коммит `742ea78395b4122e13fd46743caba5140f47fb66` | `6c24155397cf0e751c05931bae68e67e4fe0fd58baeb7b29e9f80c90ed1d5194` |
| `icons/phosphor/timer.svg` | там же (`timer`) — таймер | `eccb04ff7903c451948aba91d91606eba71d9e050ed52b9e14a0e11199379d0e` |
| `icons/phosphor/x.svg` | там же (`x`) — закрыть панель/меню | `7397b15e1fa66eaf6ff9b59fe91d483cb5c3bcf75b2fe052097ab6f0593b513a` |
| `icons/phosphor/arrow-counter-clockwise.svg` | там же — сброс экспозиции | `4eb160d5ae781107c674481ac081962e6bce6281a646129e6c3f960b9dd5dac6` |
| `icons/phosphor/dots-six.svg` | там же — кнопка меню Camera Controls («шесть точек», `layout.md`/`behavior.md`; `dots-nine` не взят: у Apple шесть) | `98d4801caca44a0be87d8aa5c63d3c31834047e5c0a862d7d751f33266332bbc` |
| `icons/lucide/ratio.svg` | Lucide (ISC), `lucide-icons/lucide`, `icons/ratio.svg`, коммит `742ea78395b4122e13fd46743caba5140f47fb66` — соотношение сторон (у Phosphor аналога нет, `assets.md`); два пересекающихся скруглённых прямоугольника 12×20 и 20×12, `stroke` ширины 2 | `d30cb7b90d5164cfe6b14457cc0e333143a0a1e5ebdc281ba64facaf7eaf3125` |
| `icons/own/level.svg` | **Своя** (лицензия проекта), нарисована в сессии 12 для пункта меню Level: у Phosphor и Lucide уровня нет (`assets.md`). viewBox 24, `stroke` ширины 2 с круглыми концами: горизонт с разрывом в центре (`M5 12h5`, `M14 12h5`) и два коротких боковых штриха (`M2 9v6`, `M22 9v6`) | `cc07f860b145327898bba2b5f1ca3415de814f9f7c0756b34ffc20fc0bf19eab` |
| `icons/own/zoom-marker-triangle.svg` | **Своя** (лицензия проекта), нарисована в сессии 13 (правка после c8631a6) для маркера колеса зума: у iOS маркер — жёлтый треугольник 3,3 × 8,7 pt (`docs/ios-camera/layout.md`, `[measured: R00]`, правило 96). viewBox 24, `fill`: треугольник на весь квадрат — основание вдоль верха `M0 0H24`, вершина внизу (12, 24); экран растягивает квадрат до 3,3 × 8,7 pt. Направление вершины (к центру диска) — `[estimate]`, в замере не снято | `e4345338ac58cecd8a09220abd175bc016978730be32360aec8fec0471c274b3` |
| `icons/lucide/LICENSE` | ISC, Copyright 2026 Lucide Icons and Contributors | `b495047bd93a9b06913511076f504daba17d5bbeb3e0650f3bb53a4220329c57` |

Phosphor — иконки-заливки (`kind: fill`, ненулевая обмотка), Lucide — иконка-контур (`kind: stroke`,
ширина штриха 2 единицы viewBox 24 — доказывает путь «расстояние до средней линии минус полуширина»).
Набор из 14 иконок: 6 стартовых плюс 6 для экрана камеры v1 (сессия 12: `timer`, `aspect_ratio`, `close`, `reset`, `controls_menu` взяты как есть из закреплённых коммитов; `level` — своя, см. таблицу; 13-я — своя `zoom_marker_triangle`, сессия 13; 14-я — `flash_auto` (`lightning-a`), сессия 14d). `RAW` — подписью, шестерёнка настроек — сессия 16.

## Манифест `bake.json`

JSON без комментариев, поэтому пояснения здесь. Разбор строгий: неизвестное поле — ошибка сборки.

* `faces` — начертания (`name` → вариант `FontFace`), `axes` — координаты осей.
* `charset` — ASCII 0x20–0x7E плюс `× − – — ° … · ’ “ ” ± • ƒ` (множитель зума, минус экспозиции, диапазоны,
  градусы, многоточие, кавычки, диафрагма). Любой символ вне набора вёрстка отвергает ошибкой.
* `disabled_features` — `calt`, `liga`, `clig` выключены: вёрстка на телефоне знает только продвижение,
  кернинг пар и табличные цифры (`tnum`), контекстные формы Inter (например, поднятое двоеточие в «4:3»)
  не поддерживаются сознательно (решение 87).
* `atlas.em_texels = 64`: подписи 11–34 pt × 2,4545 px/pt = 27–83 px, типичная 49 px (20 pt) — атлас чуть
  больше кегля, увеличение при показе 1,1–1,2× (анимации) и до ~1,3× для самых крупных подписей остаётся в
  зоне, где SDF почти не скругляет углы (решение 88).
* `atlas.spread_texels = 6`: расстояние хранится до 6 текселей от края — этого хватает для сглаживания
  края на 1–2 px при любом показе, для контура/тени вокруг букв и для запаса при анимации масштаба.
  Больше — раздувает ячейки (площадь растёт квадратично), меньше — не хватает на контуры.
* `atlas.icon_texels = 64`: сторона квадрата viewBox иконки в атласе (иконки ~24–30 pt = 60–74 px).
* `icons[].id` → вариант `IconId`; `stroke_width` обязателен для `stroke` и сверяется с атрибутом
  `stroke-width` в SVG (расхождение — ошибка сборки).

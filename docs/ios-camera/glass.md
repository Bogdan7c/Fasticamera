# Материал Liquid Glass

Часть спецификации экрана камеры (сессия 09). Читатель — сессия 11 (`ui-render-vulkan`: SDF-фигуры и стекло поверх `BackdropSource`), значения по умолчанию для `GlassStyle` берутся отсюда. Пометки происхождения — см. [README](README.md).

Снимок источников: 19.11.2026. Транскрипты WWDC скачаны с developer.apple.com и разобраны по тексту, не пересказом. Таймкоды — по транскрипту страницы.

- WWDC25 «Meet Liquid Glass» — [219](https://developer.apple.com/videos/play/wwdc2025/219/)
- WWDC25 «Get to know the new design system» — [356](https://developer.apple.com/videos/play/wwdc2025/356/)
- WWDC25 «Build a SwiftUI app with the new design» — [323](https://developer.apple.com/videos/play/wwdc2025/323/)
- WWDC25 «Build a UIKit app with the new design» — [284](https://developer.apple.com/videos/play/wwdc2025/284/)
- WWDC26 Keynote — [101](https://developer.apple.com/videos/play/wwdc2026/101/), Platforms State of the Union — [102](https://developer.apple.com/videos/play/wwdc2026/102/)
- HIG Materials — [ссылка](https://developer.apple.com/design/human-interface-guidelines/materials)

## 0. Главный вывод

**Apple не публикует ни одного числа физики стекла**: ни толщины, ни показателя преломления, ни ширины зоны линзирования, ни радиуса размытия, ни силы блика, ни длительности «материализации». Единственное число в документации — затемнение 35 % под вариантом Clear (раздел 4). Поведение описано подробно и качественно. Поэтому:

- **поведение** (что от чего зависит) — закон, сверяемся с цитатами ниже;
- **числа** `GlassStyle` — стартовые значения из открытых реимплементаций `[reverse]`, подгоняются на глаз по видео и (позже) по эталонному iPhone. Каждое такое число в коде помечается «подгонка, не Apple».

## 1. Из чего состоит материал (поведение, `[doc]`)

| Свойство | Как ведёт себя | Цитата |
|---|---|---|
| **Линзирование** — главный признак | стекло не рассеивает, а изгибает и собирает свет фона | 219, 1:55: *"The primary way Liquid Glass visually defines itself is through something called Lensing."*; 2:29: *"this new set of materials dynamically bends, shapes, and concentrates light in real time."* |
| Толщина растёт с размером | большая форма — сильнее линза и преломление, глубже тень, мягче рассеяние | 219, 7:02: *"its material characteristics change to simulate a thicker, more substantial material. It casts deeper, richer shadows, has more pronounced lensing and refraction effects, and a softer scattering of light."* |
| Размер и непрозрачность | большое — непрозрачнее, малое — прозрачнее и само переключается светлое/тёмное | 284, 21:30: *"A larger size is more opaque. A smaller size is clearer, and switches between light and dark mode automatically, to increase contrast."* |
| Светлое/тёмное | малые элементы (кнопки, панели) переворачиваются светлое ↔ тёмное по фону, большие (меню) — нет; значки поверх стекла переворачиваются вместе со стеклом | 219, 15:16–15:40: *"Bigger elements, like menus or sidebars also adapt based on context, but they don't flip from light to dark. Their surface area is too big"*; *"symbols and glyphs on top of Liquid Glass, do the same."* |
| Адаптация к фону | тон и динамический диапазон подстраиваются, чтобы надписи читались, пропуская максимум фона | 219, 6:27: *"The amount of tint and the dynamic range shift to always ensure buttons remain legible, while letting as much of the content through as possible."* |
| Тень адаптивная | над текстом тень плотнее, над светлым однотонным фоном — слабее | 219, 11:47 |
| Свет соседнего контента | цветной фон рядом слегка «подсвечивает» поверхность и тень | 219, 8:09: *"Light from colorful content nearby can subtly spill onto its surface"* |
| **Блики** | источники света «в окружении» дают блики по геометрии формы; при некоторых взаимодействиях свет «обегает» силуэт | 219, 11:07–11:19 |
| **Блик и наклон телефона** | «в некоторых случаях» освещение реагирует на движение устройства. Какие элементы — не сказано | 219, 11:29: *"And in some cases, the lighting responds to device motion, making it feel like Liquid Glass is aware of its position in the real world"* |
| **Касание** | стекло мгновенно «гнётся» и наполняется светом; свечение начинается под пальцем и растекается на соседние стеклянные элементы | 219, 3:38: *"instantly flexing and energizing with light"*; 12:05: *"Starting right under your fingertips, the glow spreads throughout the element and onto any Liquid Glass elements nearby"* |
| Касание (API) | масштаб, отскок, мерцание | 323, 19:31: *"Glass reacts to user interaction by scaling, bouncing, and shimmering"*; 284, 23:09: *"When tapping the button, it scales and bounces."* |
| «Гелевая» упругость | форма движется вместе с пальцем, с податливостью | 219, 3:51: *"an inherent gel-like flexibility"* |
| Появление/исчезновение | не растворение по прозрачности, а нарастание линзирования («материализация») | 219, 2:55: *"Instead of fading, Liquid Glass objects materialize in and out by gradually modulating the light bending and lensing"*; 284, 23:29: *"Always prefer setting the effect property over the alpha"* |
| Морфинг | при смене состояния стекло перетекает между контролами, как одна плавающая плоскость; меню «лопается» пузырём | 219, 4:43–5:05: *"the bubble simply pops open to reveal the content"* |
| Стекло на стекле — нельзя | | 219, 13:24: *"always avoid glass on glass"* |
| Элементы поверх стекла | заливки, прозрачность и «вибрантность» — тонкий слой, часть материала | 219, 13:36 |

## 2. Форма и слияние

| Правило | Источник |
|---|---|
| Форма по умолчанию — **капсула**, радиус = половина высоты | `[doc]` 323, 18:36: *"By default, a glass effect will be applied within a capsule shape"*; 356, 3:49: *"Capsules use a radius that's half the height of the container."* |
| **Концентричность**: радиус вложенной формы = радиус родителя − отступ | `[doc]` 356, 3:49: *"concentric shapes calculate their radius by subtracting padding from the parent's."* Самый внешний «родитель» — скругление экрана: у нашего телефона 96 px (см. `layout.md`) |
| У края экрана телефона — капсула с дополнительным полем | `[doc]` 356, 5:48 |
| Слияние близких форм — «как капли воды», когда расстояние меньше `spacing` контейнера | `[doc]` 284, 24:07: *"Only if they get closer, they start merging like small droplets of water."*; API `GlassEffectContainer(spacing: CGFloat? = nil)`, по умолчанию числа нет; в примере Apple `spacing: 40.0` |
| Стекло **сэмплирует область больше себя** и не видит другое стекло → соседние стёкла обязаны быть в одном контейнере, общая область сэмплирования | `[doc]` 323, 20:00: *"This effect is achieved by sampling content from an area larger than itself. However, glass can not sample other glass"* |
| Контейнер выравнивает адаптацию: соседние кнопки выглядят одинаково | `[doc]` 284, 24:46: *"It enforces a uniform adaptation!"* |
| Переходы: `matchedGeometry` (внутри `spacing` новая форма вырастает из соседней), `materialize` (без подгонки геометрии), `identity` | `[doc]` API `GlassEffectTransition` |

Для сессии 11 это значит `[estimate]`: стеклянные элементы экрана камеры рисуются **одним проходом на группу** над одним снимком фона (`BackdropSource`); слияние — SDF «гладкий минимум» с радиусом из `spacing`.

## 3. Варианты Regular и Clear

| Вариант | Поведение | Источник |
|---|---|---|
| **Regular** | универсальный, размывает и подстраивает яркость фона, адаптивный, читаемость в любом контексте | `[doc]` 219, 14:00; HIG Materials: *"The regular variant blurs and adjusts the luminosity of background content to maintain legibility"* |
| **Clear** | постоянно прозрачнее, **без адаптации**, нужен затемняющий слой | `[doc]` 219, 14:16: *"It is permanently more transparent ... it needs a dimming layer to darken the underlying content."* |
| Когда Clear | над «богатым медиа» (фото, видео), затемнение не мешает, содержимое сверху яркое и крупное | `[doc]` 219, 14:54 |
| **Затемнение под Clear** | **35 %** чёрного, если фон яркий | `[doc]` HIG Materials: *"If the underlying content is bright, consider adding a dark dimming layer of 35% opacity."* |
| Не смешивать в одном интерфейсе | | `[doc]` 219, 13:51: *"They should never be mixed"* |
| Тон (tint) | цвет превращается в набор тонов по яркости фона; сплошная заливка «ломает» стекло | `[doc]` 219, 16:31–17:03 |

Экран камеры iOS — ровно случай Clear: живое видео под кнопками, иконки яркие. Какой вариант у Apple в камере — прямо не сказано `[estimate]`: по описаниям обзоров (*"a more subtle Liquid Glass ring"* у затвора, MacRumors) — прозрачный.

## 4. Системные настройки и iOS 27

| Настройка | Что меняет | Источник |
|---|---|---|
| Reduce Transparency | стекло «матовее», скрывает больше фона | `[doc]` 219, 18:22: *"makes Liquid Glass frostier and obscures more of the content behind it."* |
| Increase Contrast | элементы в основном чёрные или белые, с контрастной рамкой | `[doc]` 219, 18:29 |
| Reduce Motion | эффекты слабее, упругость выключена | `[doc]` 219, 18:35: *"disables any elastic properties for the material."* |
| iOS 26.1: Clear / Tinted | Tinted — выше непрозрачность и контраст, везде в системе | `[review]` [MacRumors](https://www.macrumors.com/2025/10/20/ios-26-1-liquid-glass-toggle/): *"tinted increases the opacity of Liquid Glass and adds more contrast."* |
| **iOS 27**: стекло сильнее рассеивает сложный фон, **тёмный край** и **ярче блики** | | `[doc]` WWDC26 102, 22:43: *"we tuned Liquid Glass so it more effectively diffuses complex content behind it. And to establish more depth and separation, we also introduced a darkened edge along with brighter specular highlights."* |
| **iOS 27**: ползунок «от ультрапрозрачного до полностью тонированного» вместо переключателя | | `[doc]` WWDC26 102, 22:57: *"a new slider in settings to adjust Liquid Glass anywhere from ultra clear to fully tinted"*; `[review]` [MacStories](https://www.macstories.net/stories/ios-and-ipados-27-review/2/): Settings → Appearance → Liquid Glass, опорные точки More Clear / Default / More Tinted; *"the new 'Default' option in iOS 27 is more tinted than the 'Clear' setting of iOS 26."* |

Процентов непрозрачности ни для одного положения нет нигде.

Для нас (эталон — iOS 27, решение владельца в 09, см. `scope-v1.md`): целимся в положение Default. Ползунок прозрачности — один параметр `GlassStyle` (`tint_level` 0…1), в настройки — позже. Reduce Transparency / Increase Contrast — привязать к системным «Специальным возможностям» Android `[estimate]`, не в v1.

## 5. Числа из открытых реимплементаций (`[reverse]`)

Основа для формул — **Kyant0/AndroidLiquidGlass** (библиотека `backdrop`, **Apache-2.0**, AGSL для Android). Это прямое чтение их кода, не измерение Apple.

**Линза** (RoundedRectRefractionShader):
1. SDF скруглённого прямоугольника `sd` (внутри отрицательный).
2. Глубже зоны `refractionHeight` от края — пиксель фона без смещения.
3. В зоне: `x = 1 − (−sd) / refractionHeight`, смещение `d = (1 − √(1 − x²)) · refractionAmount` — профиль «четверть окружности»: максимум у самого края, к центру плавно 0. Закона Снелла и показателя преломления в коде нет — это эвристика.
4. Направление — нормализованный градиент SDF (плюс опционально «глубина» к центру).
5. Дисперсия (опция): 7 сэмплов по цветам, сила растёт к диагональным углам.

| Параметр Kyant | Кнопка 48 dp | Панель вкладок | Лист 32 dp |
|---|---|---|---|
| Насыщенность (vibrancy) | 1,5 | 1,5 | 1,5 |
| Размытие | 2 dp | 8 dp | 4 dp |
| Высота зоны линзы | 12 dp | 24 dp | 16 dp |
| Сила смещения | 24 dp | 24 dp | 32 dp |
| Заливка | — | — | белый, α 0,5 |
| Нажатие: масштаб | 1 + 4 dp / высота | 1 + 16 dp / ширина | — |

Блик: белый, α 0,5, режим «сложение», угол 45°, интенсивность `|dot(градиент, направление)|^1` (вариант Plain — α 0,38). Тень: радиус 24 dp, чёрный α 0,1, смещение вниз на радиус/6; внутренняя — α 0,15. Свечение от пальца: радиальное, радиус 1,5 × меньшая сторона. Пружина нажатия у них — `spring(1, 300, 0.5)`.

Прочие реализации — для сравнения, не как основа:

| Проект | Что полезно | Лицензия |
|---|---|---|
| [kube.io](https://kube.io/blog/liquid-glass-css-svg/), статья | закон Снелла, IOR 1,5 (выбор автора); профиль края «выпуклый суперэллипс» `y = (1 − (1 − x)⁴)^(1/4)` — по словам автора, его предпочитает Apple; блик — «ободок» с фиксированным направлением света | не указана — код не берём, только идею |
| [Prismal](https://github.com/styropyr0/Prismal) | двойное `refract()`, Френель по Шлику, дисперсия R/B; толщина ободка ≤ 0,35 × половины меньшей стороны | MIT |
| react-native-liquid-glassmorphism ([статья](https://dev.to/wrack/liquid-glass-on-android-in-react-native-i-refracted-the-backdrop-with-a-real-time-agsl-shader-1dp4)) | дисперсия 0,018 — «у стекла iOS почти нет радуги»; зеркальное «эхо» фона у края, полоса ≈ треть поверхности | не проверена |

## 6. Стартовые значения `GlassStyle` для сессии 11 (`[estimate]` на основе `[reverse]`)

Всё ниже — **подгонка**, а не Apple. Единицы — pt (см. `layout.md`, 1 pt = 2,4545 px у нас).

| Параметр | Кнопка (≈ 44–48 pt) | Панель/меню | Почему |
|---|---|---|---|
| Профиль края | выпуклый суперэллипс (kube.io) | то же | Apple *"favors"* (реверс) |
| Ширина зоны линзы | 25 % высоты | 24 pt | Kyant 12 dp на 48 dp |
| Сила смещения | 50 % высоты | 24–32 pt | Kyant; у больших форм сильнее (Apple 219, 7:02) |
| Размытие фона | 2 pt | 8 pt | Kyant; большие формы мягче (Apple) |
| Насыщенность | 1,5 | 1,5 | Kyant |
| Дисперсия | 0,02 | 0,02 | «почти нет радуги» |
| Блик | белый, α 0,5, сложение, ободок; направление — от гироскопа, с покоем 45° | то же | Kyant + Apple 219, 11:29 |
| Тёмный край (iOS 27) | есть, сила — подгонка | есть | WWDC26 102 |
| Затемнение под стеклом | до 35 % на ярком фоне, адаптивно по яркости фона | — | HIG Materials |
| Нажатие | масштаб ~1,1, свечение от точки касания, пружина без перелёта → отскок | — | Apple 323, 19:31; числа — `motion.md` |

## 7. Что стеклянное на экране камеры (`[review]`)

- Все кнопки «плавают» над фоном (MacRumors iOS 26 guide: *"Buttons are more rounded and 'float' over the background of the app thanks to a new depth effect."*).
- Миниатюра — круглая (там же).
- Затвор — *"a more subtle Liquid Glass ring"* вместо белого кольца (там же).
- Переключатель режимов — *"Liquid Glass segmented control"*, кратко расширяется при смене режима (MacStories iOS 26).
- Меню настроек — стеклянное, пункт морфится в подменю, анимации *"playfully interact with the Liquid Glass material and content underneath in real time"* (MacStories iOS 26).
- Кнопки зума — прямой цитаты, что они стеклянные, нет; см. разбор кадров в `layout.md`.

## 8. Чего не нашли

- Любые числа Apple, кроме 35 % затемнения.
- Проценты Clear/Tinted и положений ползунка iOS 27.
- Какие именно элементы отвечают бликом на наклон (*"in some cases"*).
- Насколько сильны «тёмный край» и «ярче блики» iOS 27.
- Проверять глазами: на видео и, когда будет возможность, на эталонном iPhone (`results/09.md`, список роликов).

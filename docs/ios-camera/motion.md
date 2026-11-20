# Анимации, пружины и прокрутка

Часть спецификации экрана камеры (сессия 09). Пометки происхождения — см. [README](README.md). Читатель — сессия 10 (`ui-core`: пружины, прокрутка, часы анимаций) и сессии 12–15 (конкретные анимации экрана).

Сокращение: `D` = `https://developer.apple.com/documentation/`. Все числа `[doc]` сверены по сырому JSON документации и тексту транскриптов WWDC (не по пересказу), снимок 19.11.2026. Отличий iOS 26 → 27 в этих API нет.

## 1. Модель пружины

### 1.1. Параметры Apple

SwiftUI описывает пружину двумя равносильными парами чисел. Наш `ui-core` принимает обе и сводит к одной физической модели «масса — жёсткость — затухание».

| Пара | Что значит | Источник |
|---|---|---|
| `duration` + `bounce` | `duration` — «воспринимаемая длительность», темп пружины. `bounce` — упругость: 0 — без перелёта (критическое затухание), (0, 1] — колебания (1 — без затухания), [−1, 0) — передемпфированная | `[doc]` D `swiftui/spring/init(duration:bounce:)`: *"Defines the pace of the spring. This is approximately equal to the settling duration, but for springs with very large bounce values, will be the duration of the period of oscillation"*; D `swiftui/spring/bounce`: *"negative values indicate overdamped springs with a minimum value of -1.0"* |
| `response` + `dampingFraction` (`dampingRatio`) | `response` — *"Defines the stiffness of the spring as an approximate duration in seconds."*; `dampingFraction` — доля критического затухания | `[doc]` D `swiftui/spring/init(response:dampingratio:)` |
| `mass` + `stiffness` + `damping` | физическая модель; коэффициент затухания ζ = `damping / (2·√(stiffness·mass))` | `[doc]` D `uikit/uispringtimingparameters/init(mass:stiffness:damping:initialvelocity:)` |

### 1.2. Перевод между парами

| Формула | Происхождение |
|---|---|
| `mass = 1` | `[review]` форум Apple [739811](https://developer.apple.com/forums/thread/739811) со ссылкой на слайд WWDC23 «Animate with springs» |
| `stiffness = (2π / duration)²` (при `mass = 1`) | `[derived]` — сходится с двумя примерами Apple ниже; та же формула в открытом Flutter `spring_simulation.dart` с пометкой *"same result as SwiftUI spring(duration:bounce:)"* `[reverse]` |
| `ζ = 1 − bounce` при `bounce ≥ 0`; `ζ = 1 / (1 + bounce)` при `bounce < 0` | `[derived]` для `bounce ≥ 0` (сходится с примерами Apple), ветка `bounce < 0` — только `[reverse]` (Flutter) |
| `damping = 4π·ζ·mass / duration` | `[derived]` из ζ = damping / (2√(k·m)) |
| `response = duration`, `dampingFraction = ζ` | `[derived]` — Apple прямо не пишет; следует из того, что обе пары задают одну и ту же пружину |

Контрольные точки для тестов солвера (`[doc]`, D `swiftui/spring`, примеры в коде страницы):
- `Spring(duration: 0.5, bounce: 0.3)` → `(mass 1.0, stiffness 157.9, damping 17.6)`;
- `mass 1, stiffness 100, damping 10` → `(duration 0.63, bounce 0.5)`.

`[doc]` D `quartzcore/caspringanimation/settlingduration`: при `mass 1, stiffness 100` с `damping 5` время успокоения *"approximately 2.85 seconds"*, с `damping 15` — чуть больше 1 с. Критерий покоя SwiftUI `settlingDuration` — `epsilon 0.001` (D `swiftui/spring/settlingduration`: *"an epsilon of 0.001"*). В `ui-core` критерий покоя — параметр, по умолчанию 0,001 от расстояния.

### 1.3. Форма решения

`[doc]` WWDC23 «Animate with springs» ([10158](https://developer.apple.com/videos/play/wwdc2023/10158/), 11:45–16:00): при `bounce = 1` — *"just a cosine curve, and the time is divided by the duration"*; 0 < bounce < 1 — затухающий косинус; `bounce = 0` — линейная функция на экспоненту (критическое затухание); `bounce < 0` — сумма двух экспонент. Отрицательный `bounce` *"useful for modeling decaying velocity, like what happens in a scroll view"*.

### 1.4. Прерывание и скорость

Главное свойство «как iPhone»: анимацию можно перехватить в любой момент, и она не дёргается.

| Правило | Источник |
|---|---|
| Новая пружина **заменяет** старую и **наследует её скорость** | `[doc]` D `swiftui/animation/spring`: *"each animation will be replaced by their successor, preserving velocity from one animation to the next"* |
| Смена цели на лету: скорость в момент смены — начальная скорость к новой цели | `[doc]` WWDC23 10158 (14:56): *"uses the velocity it had when it was retargeted as the initial velocity towards its new destination"* |
| Скорость жеста передаётся в пружину при отпускании пальца | `[doc]` WWDC23 10158 (5:57): *"SwiftUI will now automatically track velocities any time a gesture is changing properties"* |
| Вариант «сложения» (`interpolatingSpring`): скорости перекрывающихся анимаций **суммируются** | `[doc]` D `swiftui/animation/interpolatingspring(mass:stiffness:damping:initialvelocity:)`: *"Preserves velocity across overlapping animations by adding the effects of each animation."* |
| Начальная скорость UIKit — в долях расстояния в секунду: `dx = v_x / dist_x`, при нулевом расстоянии 0 | `[doc]` D `uikit/uispringtimingparameters/initialvelocity` |
| Анимацию можно отменить, не дожидаясь конца | `[doc]` HIG Motion: *"Let people cancel motion. ... don't make people wait for an animation to complete before they can do anything"* |
| Время успокоения не ждать для изменений, видимых пользователю | `[doc]` WWDC23 10158 (16:45): *"you shouldn't wait for the settling duration for user-facing changes"* |

Для `ui-core` из этого следует: прерывание — это новая пружина от **текущих** позиции и скорости. Режим по умолчанию — замена (`spring`), сложение (`interpolatingSpring`) — отдельная политика.

### 1.5. Стандартные пружины Apple

| Имя | duration | bounce | Эквивалент (m = 1) | Источник |
|---|---|---|---|---|
| `.smooth` | 0,5 | 0 | k 157,91, c 25,13 | `[doc]` D `swiftui/spring/smooth(duration:extrabounce:)`: *"base bounce of 0"* |
| `.snappy` | 0,5 | 0,15 | k 157,91, c 21,36 | `[doc]` D `swiftui/spring/snappy(duration:extrabounce:)`: *"base bounce of 0.15"* |
| `.bouncy` | 0,5 | 0,3 | k 157,91, c 17,59 | `[doc]` D `swiftui/spring/bouncy(duration:extrabounce:)`: *"base bounce of 0.3"* |
| `Animation.default` (iOS 17+) | response 0,55 | dampingFraction 1,0 | — | `[doc]` D `swiftui/animation/default` |
| `Animation.spring(response:dampingFraction:)` по умолчанию | response 0,5 | dampingFraction 0,825 | — | `[doc]` D `swiftui/animation/spring(response:dampingfraction:blendduration:)` |
| `interactiveSpring` | response 0,15 | dampingFraction 0,86, blendDuration 0,25 | — | `[doc]` D `swiftui/animation/interactivespring(response:dampingfraction:blendduration:)` |
| `CASpringAnimation` по умолчанию | — | — | m 1, k 100, c 10 | `[doc]` D `quartzcore/caspringanimation/stiffness`: *"The default stiffness coefficient is 100."* |

Колонка «эквивалент» — `[derived]` по формулам 1.2. `response = 0` — *"infinitely-stiff spring, suitable for driving interactive animations"* (`[doc]`), то есть элемент идёт за пальцем 1:1.

Рекомендации Apple по упругости (`[doc]` WWDC23 10158, 21:00–21:40): `bounce` 0 по умолчанию, около 0,15 — «бодро», 0,3 — заметно, *"cautious about using values higher than around 0.4"*. WWDC18 «Designing Fluid Interfaces» ([803](https://developer.apple.com/videos/play/wwdc2018/803/), 36:20–37:40): *"start with 100% damping"*; приложение Музыка: тап — 100 %, свайп закрытия — *"we use 80% damping to have a little bounce and squish"*.

## 2. Прокрутка, бросок и «резинка»

Нужна для карусели режимов, колеса зума и колеса ручных параметров (15).

| Параметр | Значение | Источник |
|---|---|---|
| Замедление по умолчанию | `.normal` | `[doc]` D `uikit/uiscrollview/decelerationrate-swift.property`: *"The default rate is normal."* |
| `.normal` | 0,998 за 1 мс | `[review]` [teletype](https://teletype.in/@swift-discipline/uiscrollview-deceleration); в текущей документации Apple числа нет |
| `.fast` | 0,99 за 1 мс | `[review]` там же |
| Скорость после броска | `v(t) = v₀ · d^(1000·t)`, t в секундах | `[derived]` из определения «во сколько раз падает скорость за 1 мс» |
| Куда приедет бросок | `проекция = (v₀ / 1000) · d / (1 − d)`, v₀ в pt/с | `[review]` код со слайда WWDC18 803 в [gist](https://gist.github.com/shaps80/641b61165fd5f8bd497e40f4d16d69be): `return (initialVelocity / 1000.0) * decelerationRate / (1.0 - decelerationRate)`; для `.normal` ≈ 0,499·v₀, для `.fast` ≈ 0,099·v₀ |
| Проекцию применять и к «прилипанию» (режимы, зум) | бросок проецируется, затем выбирается ближайшая точка прилипания и туда едет пружина | `[doc]` WWDC18 803 (44:40–47:40): *"you can also use it for scales, or even for rotations"* |
| **«Резинка» за краем** | `смещение = (1 − 1 / (x·c / d + 1)) · d`, x — насколько палец ушёл за край, d — размер области, **c = 0,55** | `[reverse]` Grant Paul, реверс UIScrollView iOS 6, запись в [gist](https://gist.github.com/originell/6961057): *"c = constant value, UIScrollView uses 0.55"*. Для iOS 26/27 не проверено |
| Возврат из-за края | пружина к границе | `[doc]` WWDC18 803 (31:20): *"the content uses elasticity to pull back within the boundaries"* |
| Остановка | постоянное трение, остановку можно перехватить сразу | `[doc]` WWDC18 803 (29:15): *"You don't have to wait for anything to finish."* |

Контрольные точки «резинки» при d = 960: x = 5 → 2,74; x = 100 → 52,02; x = 960 → 340,65 (`[derived]`, совпадает с gist).

## 3. Часы и частота

- Время кадра приходит снаружи (vsync рендера, решение 36: 60 Гц по умолчанию). Пружины считаются аналитически от времени, а не шагами, поэтому на 60 и 120 Гц положение одинаковое `[estimate]`.
- HIG Motion про частоту в приложениях чисел не даёт (кроме игровых *"30 to 60 fps"*) `[doc]`.
- Reduce Motion: упругость выключается, эффекты слабее (`[doc]` WWDC25 «Meet Liquid Glass», 18:35). На Android аналог — системная «Удалить анимации» (`ANIMATOR_DURATION_SCALE = 0`) `[estimate]`; поддержка — решение 12/16.

## 4. Анимации экрана камеры

Измерено покадрово по записям экрана iOS 27 (видео A и B, `layout.md` раздел 6). Оба видео **30 к/с**: каждая отметка времени ±33 мс, на 60/120 Гц подробностей не видно. Метод: ffmpeg + numpy (сдвиг полосы, средняя яркость, резкость по дисперсии Лапласа, площадь цвета). Свободная подгонка пружины (амплитуда, начальная скорость, ω, ζ) по 15–20 точкам определяет ζ и ω слабо, поэтому для каждой анимации дана и устойчивая величина — **скорость спада λ** (во сколько раз за секунду уменьшается остаток, `остаток ∝ e^(−λt)`).

### 4.1. Измерено

| Анимация | Видео, время | Длительность | Форма | Числа | Метка |
|---|---|---|---|---|---|
| Прилипание карусели режимов после свайпа | A, 29,733–30,93 с | ≈ 1,2 с (основное движение до 30,4 с) | без перелёта, чистая экспонента: остаток вдвое каждые ≈ 0,13 с | λ ≈ 5,2 1/с; подгонка пружины ζ ≈ 1,15, ω ≈ 7,4 рад/с → `duration ≈ 0,85 с, bounce ≈ −0,13`; остаток подгонки 0,18 pt (≈ 1 px видео) | `[measured]` |
| Открытие меню Controls | B, 209,97–210,83 с | 0,87 с | затвор, режимы и зум размываются и исчезают за ≈ 0,2 с (210,07–210,27); на месте кнопки вырастает матовая панель; пункты **не масштабируются**, а въезжают снизу вверх без перелёта | сдвиг пункта: 55 → 0 px видео за 0,53 с, λ ≈ 9,9 1/с; подгонка ζ ≈ 1,12, ω ≈ 14 рад/с → `duration ≈ 0,45 с, bounce ≈ −0,11` | `[measured]` |
| Открытие панели Exposure (альбом) | B, ≈ 328,2–328,65 с | 0,45–0,5 с | капсула режимов растворяется, на её месте вырастает стеклянная панель; перелёта не видно | — | `[measured]` длительность, форма `[estimate]` |
| Затвор: «моргание» видоискателя | B, 1042,467–1042,767 с | 0,30 с | **затемнение, не белая вспышка**: за 1 кадр до ~15 % яркости, держится 2 кадра (≈ 66 мс), затем возврат без перелёта за ≈ 8 кадров | яркость 128,7 → 19,2 → 19,3 → 28,3 → 57,3 → 87,8 → 107,4 → 118,3 → 126,2 → 127,9 → 128,6; λ возврата ≈ 18 1/с | `[measured]` |
| Миниатюра после снимка | B, 1042,77–1042,93 с | 0,13–0,17 с | перекрёстное растворение старой картинки в новую, без масштаба и «подпрыгивания»; начинается через ≈ 0,30 с после затемнения | — | `[measured]` |
| Смена режима Photo → Video (видоискатель) | B, 84,6–85,23 с | 0,63 с | размытие включается за 1 кадр, держится ≈ 0,43 с (перенастройка камеры), резкость возвращается за ≈ 0,2 с | — | `[measured]` |
| Кнопка записи: круг → скруглённый квадрат (для плана `video`) | A, 56,03–56,57 с | 0,4–0,5 с | без перелёта | остаток ×0,69 за кадр, λ ≈ 11 1/с; круг Ø 60 pt → квадрат 36 pt | `[measured]` |

Замеры 12a по R00 (iOS 26, до 120 к/с) и C (iOS 27, 60 к/с):

| Анимация | Источник | Длительность | Форма | Числа | Метка |
|---|---|---|---|---|---|
| Открытие шторки, **iOS 27** | C | ≈ 0,25 с | простое **растворение** без роста рамки; пункты едут вверх на 4,9 pt и масштаб 0,97 → 1,014 → 1,0 | — | `[measured: C]` |
| Открытие шторки, **iOS 26** | R00 t=21.30–21.95 | ≈ 0,6 с всего; к финальной рамке за 0,24 с | растёт из подписи PHOTO, **перелёт верха −15 pt**, низ закреплён; пункты не масштабируются, обрезаются растущим стеклом | пружина `duration 0,50, bounce +0,30` (`.bouncy`) | `[measured: R00]` |
| Закрытие шторки, iOS 26 | R00 t=110–115 | 0,22–0,25 с | без перелёта, сжатие к пилюле PHOTO | — | `[measured: R00]`; в 27 не снято |
| Открытие карточки инструмента, iOS 27 | C t=18, t=54 | 0,15–0,2 с | перекрёстное растворение | — | `[measured: C]` |
| Закрытие карточки, iOS 27 | C | 0,3 с | растворение; возврат «‹» в шторку ≈ 0,6 с, слегка с перелётом | — | `[measured: C]` |
| Шторка → панель Exposure, iOS 26 | R00 t=34.8–35.1 | 0,37 с (хвост до 0,6) | морф, без перелёта | λ ≈ 9,5 1/с | `[measured: R00]` |
| Закрытие панели Exposure, iOS 26 | R00 | 0,35–0,4 с | сжатие в нижний ряд | — | `[measured: R00]` |
| Резинка линейки Exposure на концах | R00 t=35.1–40.8 | возврат за долю секунды | без перелёта | растяжение 14–22,7 pt, λ 7–10 1/с | `[measured: R00]` |
| Нажатие пункта шторки, iOS 26 | R00 t=21.9–34.8 | рост 0,17 с; отпускание 0,47 с | круг ×1,26, затем пружина с недолётом ×0,96; заливка светлеет 38 → 50; старый значок гаснет, новый появляется через ≈ 0,35 с | `duration 0,47, bounce 0,5` (ζ 0,5) | `[measured: R00]` |
| Лента зума: сдвиг после тапа | R00 t=88.5–100 | 0,27–0,30 с | **без перелёта**, выбранная подпись всегда на x = 220 | прогресс по кадрам через 33 мс: 0 / .04 / .14 / .28 / .46 / .65 / .81 / .93 / .98 / 1 | `[measured: R00]` |
| Появление рамки фокуса | R00 t≈97.0–97.2 | 0,12–0,13 с | сжатие с ≈ 1,7× до 1,0, прозрачность растёт **линейно**, без перелёта; авто-рамка 180 → 124 pt за 0,23 с | — | `[measured: R00]` |
| Новый тап по кадру | R00 t=99–110 | 1 кадр | старая рамка исчезает мгновенно, без растворения | — | `[measured: R00]` |
| Рамка при открытии шторки | R00 t=110–115 | 0,17 с | гаснет; после закрытия возвращается тусклой, α ≈ 0,3, висит ≥ 4,5 с | — | `[measured: R00]` |
| Линия солнца | R00 t=99–109.5 | 1 кадр | появляется под пальцем, гаснет за 1 кадр | — | `[measured: R00]` |
| Плашка (баннер) | R00 t=61.8–68.2; 83.5–87.9 | появление 0,31 с, исчезание 0,33 с, держится 1,4–1,65 с | растворение; новая заменяет старую мгновенно | λ ≈ 14 1/с | `[measured: R00]` |
| Колесо зума: появление | R00 t=88.5–100 | 0,2 с | подъём 10,7 pt, перелёт 1,7 pt | — | `[measured: R00]` |
| Колесо зума: уход | R00 | 0,11 с, через ≈ 2,15 с после остановки | вниз на 33 pt + растворение | — | `[measured: R00]` |
| Зум и затвор при открытии шторки, iOS 26 | R00 t=21.3–21.95 | зум — 1 кадр; затвор тускнеет 0,09 с | — | — | `[measured: R00]` |

Общий вывод (исправлено в 12a): прежнее «у всех анимаций камеры перелёта нет» верно для **карусели режимов, меню (по видео B), ленты зума, морфа Exposure, появления рамки и панелей**, но **открытие шторки iOS 26 — упругое** (`.bouncy`, перелёт −15 pt), колесо зума перелетает на 1,7 pt, а возврат «‹» в шторку iOS 27 — слегка с перелётом. В iOS 27 шторка открывается без роста, простым растворением, поэтому для нашей цели (решение 102) перелёта у шторки нет. Совет Apple начинать со 100 % затухания (WWDC18 803, раздел 1.5) и `.smooth` по умолчанию остаётся рабочим правилом для того, что не измерено.

### 4.2. Не измерено (кадров нет)

Измеренное из прежнего списка перенесено в раздел 4.1 (фокус, колесо, лента зума, закрытие шторки в 26, пункт → панель). Остаётся только то, чего нет в R00 и C.

| Анимация | Стартовое значение для реализации (`[estimate]`) |
|---|---|
| Тап по кнопке зума: подпись меняется, круг | круг переезжает вместе с лентой (4.1); подпись — растворение |
| Закрытие шторки в iOS 27 | зеркально открытию (растворение ≈ 0,25 с) |
| Пункт → карточка Flash и Timer (iOS 27) | как у Aspect: растворение 0,15–0,2 с, закрытие 0,3 с (4.1) |
| Исчезновение рамки фокуса по времени | в записях нет — ярко ≥ 8,3 с, 3 с не опровергнуто; параметр, прозрачность |
| Плашка AE/AF LOCK | на видео A появляется на склейке, держится ≈ 4,5 с — возможно, монтаж; стиль — как баннеры 26, кадра LOCK нет |
| Переворот камеры | размытие последнего кадра + смена потока (как смена режима выше); поворота «карточкой» не видно |
| Расширение капсулы режимов во время свайпа | `.snappy` |
| Поворот значков по ориентации | `.smooth` (0,5 с), поворот на месте |

Сессия, которая реализует эти строки, держит значения параметрами с пометкой «подгонка, не Apple».

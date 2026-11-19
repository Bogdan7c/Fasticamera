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

Общий вывод: у всех измеренных анимаций камеры **перелёта нет** (bounce ≤ 0). Это сходится с советом Apple начинать с 100 % затухания (WWDC18 803, раздел 1.5) и со `.smooth` по умолчанию.

### 4.2. Не измерено (нет чистых кадров в найденных видео)

Причины: склейки монтажа, нарисованные поверх «пальцы» и стрелки авторов роликов, нет нужного действия в кадре.

| Анимация | Стартовое значение для реализации (`[estimate]`) |
|---|---|
| Тап по кнопке зума (выбранный кружок переезжает, подпись меняется) | `.snappy` (0,5 с, bounce 0,15) для кружка; подпись — растворение |
| Колесо зума: появление из ряда кнопок и исчезновение после отпускания | `.smooth` (0,5 с); исчезновение — с задержкой после отпускания, задержка — параметр |
| Закрытие меню Controls | та же пружина, что открытие (`duration 0,45, bounce ≈ −0,1`), в обратную сторону |
| Пункт меню → подменю (морфинг стекла) | `.smooth` (0,5 с) на рамку стекла, содержимое — растворение |
| Квадрат фокуса: появление, затухание через несколько секунд | появление — масштаб с увеличенного к 1,0 пружиной `.snappy`; затухание — прозрачность; время показа — параметр |
| Плашка AE/AF LOCK | на видео A появляется на склейке; держится ≈ 4,5 с — `[measured]`, но, возможно, это монтаж |
| Переворот камеры | размытие последнего кадра + смена потока (как смена режима выше); поворота «карточкой» на видео не видно |
| Расширение капсулы режимов во время свайпа | `.snappy` |
| Поворот значков по ориентации | `.smooth` (0,5 с), поворот на месте |

Уточнение этих строк — по записи экрана на эталонном iPhone (список роликов — `user/foundation/results/09.md`). Сессия, которая их реализует, держит значения параметрами с пометкой «подгонка, не Apple».

# Поведение элементов и жесты

Часть спецификации экрана камеры (сессия 09). Читатель — сессии 10 (распознаватели жестов и арбитраж), 12–15 (экран). Пометки происхождения — см. [README](README.md).

Источники функций — iPhone User Guide для iOS 27 и 26. Сокращение `UG/<страница>` = `https://support.apple.com/guide/iphone/<страница>/27/ios/27`:
- `BASICS` = `camera-basics-iph263472f78`;
- `TOOLS` = `use-camera-tools-iph3dc593597`;
- `VIDEO` = `record-a-video-iph61f49e4bb`;
- `BURST` = `take-burst-mode-shots-ipha42c55cd0`;
- `SELFIE` = `take-a-selfie-iph1b88429a6`;
- `LENS` = `customize-the-main-camera-lens-iph72395b28f`;
- `SAVE` = `save-camera-settings-iph62000de98`.

Страницы читались сырым HTML 19.11.2026, цитаты дословные. Значки в тексте Apple — глифы; их смысл взят из alt-текстов картинок.

## 1. Пороги жестов (для `ui-core`)

`D` = `https://developer.apple.com/documentation/`.

| Параметр | Значение | Источник |
|---|---|---|
| Долгий тап: время | **0,5 с** | `[doc]` D `uikit/uilongpressgesturerecognizer/minimumpressduration`: *"The default duration is 0.5 seconds."* |
| Долгий тап: допустимый сдвиг пальца | **10 pt** | `[doc]` D `uikit/uilongpressgesturerecognizer/allowablemovement`: *"The default distance is 10 points."* |
| Порог начала перетаскивания (pan) | **10 pt** | `[doc]` WWDC18 «Designing Fluid Interfaces» ([803](https://developer.apple.com/videos/play/wwdc2018/803/), 51:52): *"This distance is called hysteresis, and is usually 10 points in iOS."* В API-документации числа нет |
| Тап: смещение, после которого это уже не тап | = порогу pan, 10 pt | `[estimate]` — следует из того, что тап проигрывает пану, начавшемуся по гистерезису |
| Задержка одиночного тапа, если ждём двойной | «около полсекунды» | `[doc]` WWDC18 803 (56:17): *"tapping to show the app menu is delayed by about half a second"*. Максимальный интервал двойного тапа в документации не найден . В `ui-core` (10) — параметр `TapSpec.max_interval_between_taps_ms` = 300 мс `[reverse]` Android `ViewConfiguration.DOUBLE_TAP_TIMEOUT` (исходник AOSP, 19.11.2026); уточнить на эталонном iPhone |
| Порог начала щипка | не найден | `[estimate]` старт при изменении расстояния между пальцами на 10 pt |
| Скорость пальца | pt/с по x и y | `[doc]` D `uikit/uipangesturerecognizer/velocity(in:)`: *"expressed in points per second"*; алгоритм оценки Apple не раскрывает. В `ui-core` — наименьшие квадраты по последним ~100 мс `[estimate]` (WWDC18 803, 52:34: *"We use the history of the touch"*). Уточнено в 10 по исходнику AOSP `libs/input/VelocityTracker.cpp` `[reverse]`: полином 2-й степени (LSQ2) без весов, горизонт 100 мс, история 20 точек, палец стоит дольше 40 мс → скорость 0 |
| Пауза пальца перед отпусканием | определять по ускорению, не по таймеру | `[doc]` WWDC18 803 (10:41–11:35): *"it turns out that's too slow ... instead we look at your finger's acceleration"* |

### 1.1. Состояния и арбитраж (UIKit)

`[doc]` D `uikit/uigesturerecognizer`: дискретные жесты (тап) идут `possible → recognized | failed`, непрерывные (пан, щипок, долгий тап) — `possible → began → changed* → ended | cancelled`. После `ended`, `cancelled`, `failed` — снова `possible`.

| Правило | Источник |
|---|---|
| `require(toFail:)`: жест ждёт в `possible`, пока другой не провалится; если другой распознан — этот проваливается | `[doc]` D `uikit/uigesturerecognizer/require(tofail:)`: *"If otherGestureRecognizer transitions to recognized or began, the current gesture recognizer transitions to failed."* |
| Одновременное распознавание по умолчанию **запрещено** | `[doc]` D `uikit/uigesturerecognizerdelegate/gesturerecognizer(_:shouldrecognizesimultaneouslywith:)`: *"The default implementation returns false—no two gestures can be recognized simultaneously."* |
| Распознаватели получают касания раньше вида; распознанный жест отменяет касания у вида | `[doc]` `cancelsTouchesInView` = true, `delaysTouchesBegan` = false по умолчанию |
| Кнопка: подсветка **сразу** при касании, срабатывание — по отпусканию; увёл палец — отмена и снятие подсветки, вернул — подсветка снова | `[doc]` WWDC18 803 (50:24–51:09): *"the button should highlight immediately when I touch down on it ... we shouldn't confirm the tap until my touch goes up"* |
| Свайп не ждать до конца: решать по ходу движения | `[doc]` WWDC18 803 (54:42): *"you should avoid methods that are only detected at the end of the gesture"* |
| Все возможные жесты отслеживать с самого начала, лишние отменять по мере уверенности | `[doc]` WWDC18 803 (55:45): *"detect all possible gestures from the beginning of the action"* |
| Точка касания закреплена относительно содержимого (1:1) | `[doc]` WWDC18 803 (52:50): *"never use the center of the image as the dragging point"* |

### 1.2. Какие жесты конкурируют на экране камеры

Таблица — вход для арбитража `ui-core` (10). Победитель — `[estimate]` по поведению, описанному в документации ниже; в 12a подтверждено записями R00/C: тап по кадру ставит рамку (R00 t=97–110), вертикаль рядом с рамкой тянет солнце, при открытой шторке рамка гаснет за 0,17 с (`[measured: R00 t=110–115]`). Остальное — на эталонном iPhone.

| Место касания | Конкуренты | Правило |
|---|---|---|
| Видоискатель | тап (фокус), долгий тап (AE/AF Lock), щипок (зум), горизонтальный свайп (смена режима), вертикальный свайп вверх (меню Controls: в iOS 26 — по кадру, в iOS 27 по тексту Apple — от переключателя режимов), вертикальный жест «солнца» рядом с рамкой | 2 пальца → щипок сразу, остальные отменяются. 1 палец: до 10 pt и 0,5 с — ничего; отпустил раньше — тап; держит 0,5 с без сдвига — долгий тап; сдвиг > 10 pt — направление решает: горизонталь → режимы, вертикаль рядом с рамкой фокуса → экспозиция, иначе вертикаль вверх → меню |
| Капсула режимов | тап по подписи (Photo ↔ Video), горизонтальный свайп (карусель), свайп вверх (меню Controls) | тап — после отпускания без сдвига; сдвиг по доминирующей оси |
| Кнопка зума | тап (смена объектива), удержание и перетаскивание (колесо зума) | долгий тап (0,5 с) или сдвиг > 10 pt → колесо; иначе тап |
| Затвор | тап (снимок), удержание (QuickTake — у нас позже), сдвиг влево (серия — позже) | в v1 только тап; удержание не перехватывается |
| Меню Controls | тап по пункту (открывает карточку, раздел 2.6), закрытие: тап по кадру, свайп вниз, «‹» в карточке | `[doc]` UG/TOOLS (цитаты — раздел 2.6); на записях закрытие не снято |
| Колесо зума / ползунок | горизонтальное перетаскивание с броском | прокрутка с замедлением и прилипанием (`motion.md` раздел 2) |

## 2. Элементы экрана Photo

### 2.1. Видоискатель

| Жест | Поведение | Источник |
|---|---|---|
| Тап | ставит фокус и экспозицию в точку, жёлтая рамка | `[doc]` UG/TOOLS: *"Tap the screen where you want to move the focus area."* |
| Вертикальное перетаскивание «солнца» рядом с рамкой | экспозиция вверх/вниз | `[doc]` UG/TOOLS: *"Next to the focus area, drag [sun] up or down to adjust the exposure."* Диапазон и шаг Apple не публикует |
| Долгий тап | AE/AF Lock; снимается тапом | `[doc]` UG/TOOLS: *"touch and hold the focus area until you see AE/AF Lock; tap the screen to unlock settings."* |
| Щипок | зум | `[doc]` UG/BASICS: *"Pinch the screen to zoom in or out."* |
| Горизонтальный свайп | смена режима | `[doc]` UG/BASICS: *"swipe left or right to choose other modes."* |
| Свайп вверх | меню Controls | `[review]` MacStories iOS 26: *"swipe up anywhere on the camera"*; в iOS 27 Apple пишет только про свайп от переключателя режимов (UG/TOOLS) |

Диапазон и шаг экспозиции:
- **Диапазон −2,0…+2,0, шаг 1/3 EV** `[measured: R00 t=35.1–40.8]`: значения −0,3 … −2,0 … 0 … +2,0 … −2,0 прочитаны с экрана. Было: верх +2,0 `[measured]` по видео B, низ `[estimate]` по симметрии — теперь оба конца измерены. Подтверждено и в iOS 27: «±2,0» `[measured: C]`.
- **Панель Exposure — карточка внизу** (решение 103; геометрия — `layout.md` раздел 2.2). Заголовок EXPOSURE, крупное значение, линейка рисок шаг 20,3 pt на 1/3 EV (≈ 60 pt на 1 EV), жёлтый маркер, «×» (с верхнего ряда) или «‹» (из шторки) слева, «↻» справа `[measured: C]`. `[doc]` UG/TOOLS: *"Tap Exposure, then drag the slider to adjust the level. The exposure locks until the next time you open Camera."* и *"Tip: Tap [the Undo button] to reset the exposure."*
- **Поведение линейки** `[measured: R00 t=35.1–40.8]`: содержимое едет за пальцем, значение = ближайшая риска, под пальцем без прилипания, магнита у 0 нет. «Резинка» на концах 14–22,7 pt, возврат с λ ≈ 7–10 1/с, без перелёта. «↻» видна сразу, у 0,0 тускнеет до 44 %.
- **Яркость рисок**: ±4 шага — полная, ±5 — 60 %, ±6 — 10 % `[measured: R00]`; целые EV ярче дробных (L137 против 97).
- **Значение в верхнем ряду**: ненулевой EV остаётся индикатором после закрытия панели (26: «EV −2,0» слева сверху `[measured: R00 t=71.5–74.9]`; 27: «0.7 / EV» в ряду `[measured: C]`).
- **Солнце у рамки** `[measured: R00 t=99–109.5]`: вертикальная линия 1 pt появляется **только пока палец на солнце**, длина 143 pt фиксированная; ход, который видели: −35,3 / +14,5 pt. Индикатор EV слева сверху от перетаскивания солнца **не меняется**. Это та же величина EV, что в панели, — `[estimate]`; наш AE ограничивает компенсацию сам (08a, решение 72).
- В iOS 26 «сброс» тускнеет на нулевом значении (44 %) — в 27 то же ожидаем (`[estimate]`).

### 2.2. Значки над кадром (набор Essentials iOS 27)

Состав и положения — `layout.md` раздел 2.2. Решение 103 и записи C:

| Значок | Тап | Долгий тап | Источник |
|---|---|---|---|
| Вспышка (+ «A» при Auto) | по кругу Auto → On → Off | подменю `[estimate]` | `[doc]` UG/TOOLS: *"tap the Flash control above the camera frame to switch between Auto, On, or Off."*; `[review]` 9to5Mac iOS 26: *"A long press on these icons will bring up expanded configuration menus."* |
| Live | Auto / On / Off | — | `[doc]` UG Take Live Photos |
| EV (два ряда текста: «0.7» и «EV») | **открывает карточку Exposure напрямую**, с «×» вместо «‹» | — | `[measured: C]` (значок верхнего ряда → та же карточка, у Exposure ещё «↻») |
| RAW / формат («RAW» и «48») | **открывает карточку Format** (HEIF / RAW, 12MP / 48MP) | — | `[measured: C]`; `[doc]` UG Take Apple ProRAW photos: *"Tap [Camera Controls], then tap Format."* |
| Ночь | в 27 есть в ряду; карточка не снята | — | `[measured: C]` положение; поведение `[estimate]` |

- **Набор по умолчанию (Essentials)** — Flash и Live: `[doc]` UG/TOOLS: *"The Flash and Live Photo tools appear above the camera frame for quick access. You can add or edit the tools that appear there, so the ones you use most are always available."* и *"Tip: Tap Reset to Essentials to return to the default tools."* Остальное в ряду (EV, RAW, ночь) появляется, когда значение не по умолчанию `[measured: C]`.
- Правка набора: *"Tap [Camera Controls], then tap Edit to reveal the tools. Tap a tool in the menu to add it above the camera frame … or tap [Minus] next to a tool above the camera frame to remove it."*; *"Tap Done, then swipe down on the tool menu to close it."* — Edit у нас позже (`scope-v1.md`).
- Наш набор v1 — `[estimate]` под утверждённый объём (решение 80): вспышка (вкл/выкл), RAW, таймер (когда включён), EV.

### 2.3. Кнопки зума и колесо

| Жест | Поведение | Источник |
|---|---|---|
| Тап по кнопке | быстрая смена увеличения | `[doc]` UG/BASICS: *"tap 0.5x, 1x, 2.5x, 3x, 4x, 5x, or 8x to quickly zoom in or out (depending on your model)."* |
| Удержание и перетаскивание | колесо зума | `[doc]` UG/BASICS: *"Touch and hold the zoom control at the bottom of the frame, then drag the slider left or right."*; UG/VIDEO: *"For a more precise zoom control, touch and hold 1x, then slide the dial."* |
| Тап по основной (1×) повторно | переключает фокусные 24 / 28 / 35 мм | `[doc]` UG/LENS: *"Tap the Main camera lens to switch between the additional lenses you chose."* — у нас позже |

Подписи кнопок (видео A): невыбранные — `.5`, `2`, `4`, `8` (без «x»), выбранная — `1×` в круге. У нас — `.5`, `1×`, `2` (`scope-v1.md`).

Шаг тиков колеса, диапазон, инерция — в документации нет; измерения — `motion.md` раздел 4.

### 2.4. Переключатель режимов

| Жест | Поведение | Источник |
|---|---|---|
| Горизонтальный свайп | соседний режим; капсула кратко расширяется | `[doc]` UG/BASICS; `[review]` AppleInsider iOS 27: *"It still expands the selector sideways when you start swiping"* |
| Тап по подписи | переход на этот режим | `[doc]` UG/BASICS (iOS 26): *"Tap Photo or Video to switch between the two"* |
| Тап по текущему режиму | меню Controls | `[review]` Cult of Mac iOS 27: *"You can also bring up this menu by tapping on the current mode at the bottom (like Photo)"* |
| Свайп вверх от режимов | меню Controls | `[doc]` UG/TOOLS: *"swipe up from the camera modes at the bottom of the screen."* |

Порядок режимов: слева от Photo — Video, Time-lapse, Slo-mo; справа — Portrait, Pano (`[doc]` шаги «swipe left/right» на страницах режимов). Полный порядок не найден. У нас — только реализованные режимы (вопрос владельцу).

### 2.5. Затвор

| Жест | Поведение | Источник |
|---|---|---|
| Тап, любая кнопка громкости | снимок | `[doc]` UG/BASICS: *"Tap the Shutter button or press either volume button to take a photo."* |
| Удержание | QuickTake-видео; сдвиг на замок — продолжить без пальца | `[doc]` UG/VIDEO: *"Touch and hold the Shutter button to start recording."*, *"Slide the Shutter button onto the lock"* — у нас позже |
| Сдвиг влево | серия | `[doc]` UG/BURST: *"Drag the Shutter button left to begin taking photos in rapid succession."* — у нас позже |
| Тап при открытом меню Controls | **шторка скрывает затвор** (и зум, миниатюру, капсулу, переворот, кнопку меню) `[measured: C]`; в iOS 26 зум исчезает за 1 кадр, затвор тускнеет за 0,09 с `[measured: R00 t=21.3–21.95]`. Снимка из-под шторки нет | `[measured]` вместо прежнего `[estimate]` |
| Тап при открытой карточке инструмента | затвор **виден и поднят** на ≈ 100–104 pt, диаметр прежний; снимок возможен `[estimate]` (кадра нажатия нет) | `[measured: C t=18, t=54]` положение |

### 2.6. Миниатюра, переворот, меню

| Элемент | Поведение | Источник |
|---|---|---|
| Миниатюра (круг) | тап — просмотр последнего снимка | `[doc]` UG Record video: *"Tap the thumbnail at the bottom of the screen to view the video in the Photos app."* У нас — системная галерея |
| Переворот камеры | тап — фронт/тыл | `[doc]` UG/SELFIE: *"Tap [flip] to switch to the front-facing camera."* |
| Зеркальный фронт | настройка | `[doc]` UG/SELFIE: *"go to Settings > Camera, then turn on Mirror Front Camera."* |
| Меню Controls: открыть | тап по кнопке «точки» или свайп вверх от режимов | `[doc]` UG/TOOLS |
| Меню Controls: закрыть | тап по экрану / свайп вниз / «‹» из карточки | `[doc]` UG/TOOLS: *"Tap the screen to close it."* (Flash, Timer, Aspect), *"swipe down on the tool menu to close it."* (Edit), *"Tap [the Back button], then swipe down on the camera tools menu to close it."* (Exposure), *"Tap [the Close button] to close the menu."* (Styles). Закрытие шторки в 27 на записи не снято. У нас (12a): свайп вниз закрывает меню и любую карточку, откуда бы она ни была открыта `[estimate]` |
| Пункт меню (iOS 27) | **превращается в нижнюю карточку** своего инструмента; «‹» возвращает в шторку (≈ 0,6 с, слегка с перелётом); с значка верхнего ряда — та же карточка, но с «×» | `[measured: C]` для Aspect, Exposure, Format; `[review]` MacStories iOS 26: *"Select any item from this menu, and it will elegantly morph into the related sub-menu with dedicated controls."* |
| Пункт меню (iOS 26, история) | Flash / Live / Timer / Aspect / Night **переключаются по кругу на месте** при открытой шторке и дают плашку сверху; Exposure и Styles морфятся в нижнюю панель | `[measured: R00 t=21.9–34.8; 75.5–87.9]` |
| Состав меню Photo (iOS 27) | **FLASH, LIVE, ASPECT / TIMER, EXPOSURE, DEPTH / NIGHT MODE, FORMAT** (RAW 48), 3 колонки; последний ряд слева; «Edit» вне шторки слева сверху | `[measured: C]`; `[doc]` UG/TOOLS alt-текст: *"Flash, Depth, Live, Timer, Exposure, and Styles."* |
| Grid / Level | в iOS 27 их **нет** ни в шторке, ни в каталоге Edit — в Settings → Camera | `[doc]` UG/TOOLS: *"Go to the Settings app on your iPhone. Tap Camera, then turn on Grid and Level."*; `[measured: C]`. В бете 27 (июнь) пункты GRID, LEVEL, RESOLUTION в шторке были `[measured: D t=15.5]` — в выпуске их убрали. **Мы оставляем их в своём меню как адаптацию Android до сессии 16** (настройки) |
| Таймер | 3, 5 или 10 с; выключить — снова тап по значку таймера. Карточка Timer не снята — содержимое по аналогии с Aspect и Flash/Night (видео D: Night — «Auto (1s)» и ряд из трёх значков `[measured: D t=21.0]`) `[estimate]` | `[doc]` UG/TOOLS: *"Tap Timer, then tap 3, 5, or 10."*, *"tap the timer icon to turn it off."* iOS 26: off → 3 → 5 → 10 → off, цифра в жёлтом кольце `[measured: R00 t=31–33.5]` |
| Соотношение | 1:1, 4:3, 16:9; карточка: заголовок, крупное значение, три значка, выбранный жёлтый | `[doc]` UG/TOOLS: *"Tap Aspect, then tap a ratio."*; `[measured: C]`. iOS 26: 4:3 → 16:9 → 1:1 по кругу `[measured: R00 t=77–83]` |
| Вспышка в меню | *"Tap Flash, then tap to switch between Auto, On, or Off."* Карточка: заголовок FLASH, крупно «Auto», ряд значков Auto / On / Off, выбранный жёлтый, «‹» слева — форма по видео D (бета 27, съёмка камерой, без чисел) `[measured: D t=27.3]`; размеры — как у снятой карточки Aspect `[estimate]`. iOS 26: off → auto → on → off `[measured: R00]` | `[doc]` UG/TOOLS |
| Ползунок Exposure | значение держится до следующего запуска (или всегда, с Preserve Settings) | `[doc]` UG/TOOLS: *"The exposure locks until the next time you open Camera."* |

Расхождения источников (разрешены в 12a):
- Кнопка меню в iOS 27: alt-текст Apple — *"top-right corner"* (устарел), три обзора, видео A и запись C — **внизу справа**. Берём низ `[measured: C]`.
- Сетка и уровень в iOS 27: страница Apple отправляет в Settings, записи C не показывают их ни в шторке, ни в Edit. Apple и C сходятся; обзоры с переключателями в меню камеры — **устарели**. У нас они остаются в меню как адаптация до сессии 16.
- Флэш/Live в наборе: обзоры 26 говорят про значки сверху справа; в 27 — по центру `[measured: C]`.
### 2.7. Ориентация

- Раскладка всегда портретная; при повороте телефона поворачиваются значки на месте `[estimate]` (общеизвестное поведение iOS, документального источника не найдено). Ориентация — наш классификатор (решение 29).
- iOS 27 точнее определяет ориентацию, в том числе при съёмке «в пол» `[review]` Cult of Mac: *"In iOS 27, Apple says it has improved the orientation detection to be more accurate."* — наш классификатор уже держит «лежит плашмя — не менять».

## 3. Адаптация под Android и наш телефон

| Что у iOS | Что у нас | Почему |
|---|---|---|
| Настройки камеры — в системном приложении Settings | свой экран на Compose внутри приложения (16); вход — **открытый вопрос 12/16**, рекомендация — пункт-шестерёнка в меню Controls | решение 5; системных настроек камеры нет |
| Кнопки громкости = затвор, удержание — видео/серия | только тап = снимок | видео и серия позже |
| Camera Control (аппаратная кнопка) | нет; его колесо — идиома ручного режима (15) | нет кнопки |
| Свайп от нижнего края — домой | системная навигация спрятана, свайп от края показывает её поверх | приложение прячет панели (сессия 02); жесты у самого края не назначаем |
| View Outside the Frame | чёрное вне кадра | `scope-v1.md` |

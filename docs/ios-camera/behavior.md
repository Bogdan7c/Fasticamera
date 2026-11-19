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
| Задержка одиночного тапа, если ждём двойной | «около полсекунды» | `[doc]` WWDC18 803 (56:17): *"tapping to show the app menu is delayed by about half a second"*. Максимальный интервал двойного тапа в документации не найден |
| Порог начала щипка | не найден | `[estimate]` старт при изменении расстояния между пальцами на 10 pt |
| Скорость пальца | pt/с по x и y | `[doc]` D `uikit/uipangesturerecognizer/velocity(in:)`: *"expressed in points per second"*; алгоритм оценки Apple не раскрывает. В `ui-core` — наименьшие квадраты по последним ~100 мс `[estimate]` (WWDC18 803, 52:34: *"We use the history of the touch"*) |
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

Таблица — вход для арбитража `ui-core` (10). Победитель — `[estimate]` по поведению, описанному в документации ниже. Проверить на эталонном iPhone.

| Место касания | Конкуренты | Правило |
|---|---|---|
| Видоискатель | тап (фокус), долгий тап (AE/AF Lock), щипок (зум), горизонтальный свайп (смена режима), вертикальный свайп вверх (меню Controls, iOS 26), вертикальный жест «солнца» рядом с рамкой | 2 пальца → щипок сразу, остальные отменяются. 1 палец: до 10 pt и 0,5 с — ничего; отпустил раньше — тап; держит 0,5 с без сдвига — долгий тап; сдвиг > 10 pt — направление решает: горизонталь → режимы, вертикаль рядом с рамкой фокуса → экспозиция, иначе вертикаль вверх → меню |
| Капсула режимов | тап по подписи (Photo ↔ Video), горизонтальный свайп (карусель), свайп вверх (меню Controls) | тап — после отпускания без сдвига; сдвиг по доминирующей оси |
| Кнопка зума | тап (смена объектива), удержание и перетаскивание (колесо зума) | долгий тап (0,5 с) или сдвиг > 10 pt → колесо; иначе тап |
| Затвор | тап (снимок), удержание (QuickTake — у нас позже), сдвиг влево (серия — позже) | в v1 только тап; удержание не перехватывается |
| Меню Controls | тап по пункту, свайп вниз (закрыть) | — |
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
- Панель Exposure iOS 27 (видео B, 328–346 с, значения прочитаны с экрана при перетаскивании): 0.0, 0.3, 1.0, 1.7, 2.0, …, −0.3, −1.0, −1.3 — **шаг 1/3 EV** (0,3 и 0,7 — округлённые 1/3 и 2/3), **верхний предел +2.0** (значение стоит на нём несколько кадров при движущемся пальце) `[measured]`. Нижний предел на видео не достигнут; `[estimate]` −2.0 по симметрии и по устаревшей статье [gadgethacks](https://ios.gadgethacks.com/how-to/control-focus-exposure-locks-separately-your-iphones-camera-0320703/) времён iOS 14 (*"from –2 to +2 EV"*).
- Панель Exposure: вертикальная подпись EXPOSURE, число, линейка с тиками и жёлтой отметкой, кнопка «×», кнопка сброса появляется после первого изменения (видео B) `[measured]`; `[doc]` UG/TOOLS: *"Tap [reset] to reset the exposure."*
- Перетаскивание «солнца» у рамки фокуса — та же величина EV `[estimate]`; наш AE ограничивает компенсацию сам (08a, решение 72).

### 2.2. Значки над кадром (набор Essentials iOS 27)

| Значок | Тап | Долгий тап | Источник |
|---|---|---|---|
| Вспышка | по кругу Auto → On → Off | подменю | `[doc]` UG/TOOLS: *"tap the Flash control above the camera frame to switch between Auto, On, or Off."*; `[review]` 9to5Mac iOS 26: *"A long press on these icons will bring up expanded configuration menus."* |
| Live | Auto / On / Off | — | `[doc]` UG Take Live Photos |
| EV (iOS 27, на видео A показано «0.0 EV») | открывает ползунок экспозиции `[estimate]` | — | видео A, кадр 52 с |
| RAW / формат | в iOS 27 — через инструмент Format | — | `[doc]` UG Take Apple ProRAW photos: *"Tap [Camera Controls], then tap Format."* |

iOS 27 позволяет менять набор значков (Edit, Reset to Essentials) — у нас позже (`scope-v1.md`). Наш набор v1 — `[estimate]` под утверждённый объём (решение 80): вспышка (вкл/выкл), RAW, таймер (когда включён), EV.

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
| Тап при открытом меню Controls | `[estimate]`: снимок без закрытия меню | на видео B затвор при открытом меню скрыт под шторкой — **проверить**. Сессия 12 решает по этой строке |

### 2.6. Миниатюра, переворот, меню

| Элемент | Поведение | Источник |
|---|---|---|
| Миниатюра (круг) | тап — просмотр последнего снимка | `[doc]` UG Record video: *"Tap the thumbnail at the bottom of the screen to view the video in the Photos app."* У нас — системная галерея |
| Переворот камеры | тап — фронт/тыл | `[doc]` UG/SELFIE: *"Tap [flip] to switch to the front-facing camera."* |
| Зеркальный фронт | настройка | `[doc]` UG/SELFIE: *"go to Settings > Camera, then turn on Mirror Front Camera."* |
| Меню Controls: открыть | тап по кнопке «точки» или свайп вверх от режимов | `[doc]` UG/TOOLS |
| Меню Controls: закрыть | свайп вниз по меню или тап по видоискателю | `[doc]` UG/TOOLS: *"swipe down on the tool menu to close it."*, *"Tap the screen to close it."* |
| Пункт меню | превращается («морфится») в своё подменю | `[review]` MacStories iOS 26: *"Select any item from this menu, and it will elegantly morph into the related sub-menu with dedicated controls."* |
| Состав меню Photo (iOS 27) | Flash, Depth, Live, Timer, Exposure, Styles + Format, Level, Grid | `[doc]` UG/TOOLS alt-текст: *"Flash, Depth, Live, Timer, Exposure, and Styles."*; `[review]` AppleInsider: *"there are extras, including Depth, Format, Level, and Grid."* |
| Таймер | 3, 5 или 10 с | `[doc]` UG/TOOLS: *"Tap Timer, then tap 3, 5, or 10."* |
| Соотношение | 1:1, 4:3, 16:9 | `[doc]` UG/TOOLS: *"Tap Aspect, then tap a ratio."* |
| Ползунок Exposure | значение держится до следующего запуска (или всегда, с Preserve Settings) | `[doc]` UG/TOOLS: *"The exposure locks until the next time you open Camera."* |

Расхождения источников:
- Кнопка меню в iOS 27: alt-текст Apple — *"top-right corner"*, три обзора и видео A — внизу справа. Берём низ.
- Сетка и уровень в iOS 27: страница Apple отправляет в Settings, обзоры показывают переключатели в меню камеры. Берём меню (и дублируем в настройках).

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

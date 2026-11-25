//! Всё, что меняет состояние экрана: жесты, клавиши, факты камеры, итоги снимка, датчики, время.
//!
//! Моменты времени приходят **в действиях** (метка касания или vsync): редьюсер часов не читает.

use shell_contract::clock::MonotonicNanos;
use shell_contract::gravity::GravityVector;
use shell_contract::orientation::DeviceOrientation;
use shell_contract::safe_area::SafeAreaInsets;
use shell_contract::surface::SurfaceSize;
use ui_core::geometry::Point;

use camera_contract::control::{ColorTemperature, ExposureBias, Tint};
use camera_contract::zoom::ZoomRatio;
use look_contract::{
    BandAxis, GlobalKnob, HueBandId, LookControl, LookFacts, LookIntent, LookKnob, PresetId, ToneBaseModel,
    UserPresetId, WheelPush, WheelZone,
};
use scope_contract::{ScopeIntent, ScopeKind, VectorscopeAxes};
use shell_contract::text_input::{TextInputResult, TextRequestId};

use crate::calibration::{CalibrationMenuItem, ControlViewKind};
use crate::facts::CameraFacts;
use crate::modes::CaptureMode;
use crate::shutter::{CaptureOutcome, ShotRequestId};
use crate::tools::{ToolId, ToolOption};
use crate::white::WhiteAxis;
use crate::white_config::WhiteLightIndex;

/// Фаза аппаратной клавиши.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyPhase {
    /// Нажатие — единственная фаза с действием в v1.
    Pressed,
    /// Отпускание. В v1 без действия (место под удержание: QuickTake, серия).
    Released,
    /// Автоповтор при удержании. В v1 без действия.
    Repeated,
}

/// Действие над экраном. С сессии 13 не `Copy`: факты камеры несут общий каталог зума (`Arc`).
#[derive(Debug, Clone, PartialEq)]
pub enum ScreenAction {
    /// Тап по видоискателю (фону): точка в pt. При открытой панели закрывает её.
    ViewfinderTapped { point: Point, at: MonotonicNanos },
    /// Долгий тап по видоискателю: блокировка AE/AF.
    ViewfinderLongPressed { point: Point, at: MonotonicNanos },
    /// Перетаскивание «солнца» или ползунка панели: запрошенное значение (квантуется редьюсером).
    ExposureDragged {
        requested: ExposureBias,
        at: MonotonicNanos,
    },
    /// Палец лежит на «солнце» рамки фокуса (любое событие пана, даже без смены шага EV): индикатор фокуса
    /// не должен гаснуть под пальцем. Эффекта камере НЕ порождает и значение EV не трогает.
    FocusSunHeld { at: MonotonicNanos },
    /// Кнопка сброса на панели экспозиции.
    ExposureResetRequested,
    /// Синхронизация шкалы экрана с значением, которое **уже действует** в камере. Эффекта камере
    /// НЕ порождает — иначе установка «из фактов» вернулась бы в камеру эхом.
    ///
    /// Контракт: экран не сообщает камере восстановленное значение. Если источник значения —
    /// сохранённые настройки (а не камера), приложение обязано само отправить камере
    /// `SetExposureBias` с тем же значением и только потом (или одновременно) прислать это действие.
    /// В v1 восстановления нет: по `behavior.md` 2.6 значение держится «до следующего запуска
    /// камеры» (запуск начинается с 0 EV), а `CameraFacts` значение EV не несут — единственный
    /// источник этого действия сейчас приложение. Отдельное действие «восстановить и применить»
    /// заводить, пока нет сохранения настроек (сессия 16), не стали: лишний путь к камере из экрана.
    ExposureRestored { bias: ExposureBias },
    /// Закрытие панели экспозиции (кнопка «×»).
    ExposurePanelCloseRequested,
    /// Выбор режима в карусели; повторный выбор текущего открывает меню Controls.
    ModeSelected(CaptureMode),
    /// Кнопка меню или свайп вверх от режимов.
    ControlsMenuOpenRequested,
    /// Свайп вниз по меню.
    ControlsMenuCloseRequested,
    /// Тап по пункту меню Controls.
    ControlsItemSelected(ToolId),
    /// Выбор варианта в карточке инструмента: настройка применяется, карточка остаётся открытой.
    SubmenuOptionSelected(ToolOption),
    /// Левая кнопка карточки инструмента (подменю или Exposure): «‹» из меню Controls возвращает в
    /// меню, «×» карточки, открытой значком над кадром, закрывает панель.
    SubmenuBackRequested,
    /// Тап по значку над кадром.
    StripToolTapped(ToolId),
    /// Тап по затвору.
    ShutterTapped { at: MonotonicNanos },
    /// Клавиша громкости (снимает и при открытом меню, решение 94).
    VolumeKey { phase: KeyPhase, at: MonotonicNanos },
    /// Тап по миниатюре.
    ThumbnailTapped,
    /// Снимок фактов камеры.
    CameraFactsChanged(CameraFacts),
    /// Кадр снимка захвачен (фаза 1, решение 140): затвор свободен и можно снимать снова, пока снимок
    /// сохраняется. Итог сохранения придёт отдельно (`CaptureFinished`).
    CaptureSecured { request: ShotRequestId },
    /// Итог снимка от сервиса съёмки (фаза 2): для снимка в `Capturing` (ошибка до захвата освобождает
    /// затвор) и для уже захваченного, сохраняемого.
    CaptureFinished {
        request: ShotRequestId,
        outcome: CaptureOutcome,
    },
    /// Новая ориентация телефона (классификатор, решение 29).
    DeviceOrientationChanged(DeviceOrientation),
    /// Новое показание гравитации (для уровня).
    GravityChanged(GravityVector),
    /// Время: отсчёт таймера, затухание индикатора, ожидание блокировки.
    Tick(MonotonicNanos),
    /// Размер surface.
    SurfaceResized(SurfaceSize),
    /// Безопасная зона поверхности (вырез, системные панели, полосы жестов): от неё зависит раскладка раздела
    /// калибровки, поэтому это действие, а не фильтр входа, как зоны системных жестов.
    SafeAreaChanged(SafeAreaInsets),
    /// Тап по кнопке ленты зума: плавный переход к упору (номер — позиция в `LensGroup::stops`).
    ZoomStopTapped { stop: usize, at: MonotonicNanos },
    /// Палец лёг на ленту кнопок зума (решение 124: дольше порога тапа — разбудить тёплую камеру).
    ZoomStripPressBegan { at: MonotonicNanos },
    /// Палец поднят с ленты кнопок или жест отменён (тап, долгое нажатие, пан, ушедший в колесо).
    ZoomStripPressEnded { at: MonotonicNanos },
    /// Палец лёг на колесо зума (долгий тап или пан по ленте открыл его; прикосновение к открытому колесу).
    /// Закрытое колесо открывается. Продлевает жизнь колеса.
    ZoomWheelTouched { at: MonotonicNanos },
    /// Палец поднят с колеса: через `zoom.wheel.idle_dismiss_ms` покоя колесо уйдёт.
    ZoomWheelReleased { at: MonotonicNanos },
    /// Колесо сдвинуто: запрошенный коэффициент (зажимается в пределы группы).
    ZoomWheelDragged { requested: ZoomRatio, at: MonotonicNanos },
    /// Начался щипок по видоискателю: запоминается коэффициент начала жеста.
    ZoomPinchBegan { at: MonotonicNanos },
    /// Щипок идёт: `scale` — отношение текущего расстояния между пальцами к начальному.
    ZoomPinchChanged { scale: f64, at: MonotonicNanos },
    /// Щипок закончился (или отменён: значение остаётся там, куда дошло).
    ZoomPinchEnded { at: MonotonicNanos },
    /// Кнопка переворота: фронтальная ↔ задняя камера.
    FlipCameraRequested { at: MonotonicNanos },
    /// Намерение по виду картинки и пресетам (решение 178): «вид = эти значения», «выбрать пресет»,
    /// «сохранить как…». Экран ничего не считает и не пишет на диск — он пересылает намерение исполнителю
    /// эффектов ([`crate::Effect::Look`]), который владеет библиотекой пресетов. Состояние экрана не меняется:
    /// истина о виде живёт в библиотеке, а не в экране (виджеты ручек — сессия 14b).
    Look(LookIntent),
    /// Намерение по цветовому прибору (решение 183): «показать такой-то» или «скрыть». Экран ничего не считает и
    /// не хранит: состояние прибора живёт у рендера, раздел калибровки с раскладкой придёт в 14b. Уходит наружу
    /// одним эффектом ([`crate::Effect::Scopes`]).
    Scopes(ScopeIntent),

    // ----- Раздел калибровки и пресеты (14b). Значения ручек экран не хранит: они берутся из `LookFacts`. -----
    /// Новый снимок состояния вида от библиотеки пресетов (список пресетов, текущий рецепт, отличия от пресета,
    /// полосы, неподдержанные ручки, проблема). Эффектов не порождает; проблема с новым серийным номером поднимает плашку.
    LookFactsChanged(Box<LookFacts>),
    /// Глобальная ручка потянута до значения. Уходит точечным намерением `SetKnob`: заменяется только эта ручка
    /// (не `SetLook` с полным набором: два пальца в одном кадре не должны затирать друг друга).
    KnobDragged { knob: GlobalKnob, value: LookKnob },
    /// Ось полосы оттенка потянута до значения.
    BandKnobDragged {
        band: HueBandId,
        axis: BandAxis,
        value: LookKnob,
    },
    /// Выбран вид колонки ручек: шкалы или колёса (переключатель SCALES | WHEELS, решение 204). Эффектов нет.
    ControlViewSelected(ControlViewKind),
    /// Выбрано колесо цвета (вкладка LIFT · GAMMA · GAIN · OFFSET, решение 205). Эффектов нет.
    WheelSelected(WheelZone),
    /// Точка толчка колеса потянута пальцем. Уходит точечным намерением `SetWheelPush`.
    WheelPushDragged { zone: WheelZone, push: WheelPush },
    /// Кольцо яркости колеса потянуто пальцем. Уходит точечным намерением `SetWheelRing`.
    WheelRingDragged { zone: WheelZone, ring: LookKnob },
    /// Выбрана модель основы тона (свайп над ручками).
    ToneBaseSelected(ToneBaseModel),
    /// Тап по «↻» у органа управления: вернуть к значению пресета (решение 196). Тусклая «↻» — `NoChange`.
    ResetControlTapped(LookControl),
    /// Прибор выбран свайпом по нему.
    ScopeKindSelected(ScopeKind),
    /// Стандарт осей вектороскопа выбран в меню.
    VectorscopeAxesSelected(VectorscopeAxes),
    /// Кнопка «чистяк» нажата: показать основу без вида (решение 189).
    CleanViewPressed,
    /// Кнопка «чистяк» отпущена: вернуть вид с правками.
    CleanViewReleased,
    /// Выбрана полоса оттенка (точка в ряду цветов).
    BandSelected(HueBandId),
    /// Пресет выбран (ряд имён в разделе или карточка Styles в PHOTO).
    PresetSelected(PresetId),
    /// «↻» карточки Styles или кнопка Reset в меню пресетов: отбросить правки поверх пресета.
    DiscardEditsTapped,
    /// Кнопка Save: записать текущий вид в активный СВОЙ пресет (встроенный — отказ).
    OverwritePresetTapped,
    /// Кнопка New: спросить имя и сохранить текущий вид как новый пресет.
    CreatePresetTapped,
    /// Кнопка Rename: спросить новое имя для своего пресета.
    RenamePresetTapped(UserPresetId),
    /// Кнопка Delete: первый тап взводит, второй по тому же пресету удаляет; любое другое действие снимает взвод.
    DeletePresetTapped(UserPresetId),
    /// Приложение выдало номер запросу имени (обратная связь на `Effect::RequestText`, как номер снимка).
    TextRequestIssued { id: TextRequestId },
    /// Запрос имени отправить не удалось (платформа недоступна): диалога не будет.
    TextRequestNotSent,
    /// Ответ системной клавиатуры. Чужой или устаревший номер — `NoChange`.
    TextInputFinished(TextInputResult),
    /// Плашка проблемы с этим номером закрыта (истёк показ или тап по ней). Номер нужен, чтобы запоздалое закрытие
    /// старой плашки не убрало новую, пришедшую в том же кадре (найдено ревью 14b).
    ProblemDismissed {
        /// Номер закрытой плашки (`ProblemBanner::number`).
        number: u64,
    },
    /// Меню «шесть точек» раздела: тап по пункту корня.
    CalibrationMenuItemTapped(CalibrationMenuItem),
    // ----- Панель белого (white-balance 02). Значения белого экран не хранит: уходят намерениями движку. -----
    /// Тап по кнопке белого рядом с затвором: открыть карточку белого, закрыть её или переключить с другой карточки.
    WhiteButtonTapped,
    /// Долгое нажатие на кнопку белого: взять белое телефона, панель не открывается.
    WhiteButtonLongPressed,
    /// Тап «Телефон» на карточке белого: взять белое телефона (без вибрации).
    WhitePhoneTapped,
    /// Тап кнопки света на карточке: белое = числа этой кнопки из конфига.
    WhiteLightSelected(WhiteLightIndex),
    /// Выбрана шкала карточки (Кельвины или оттенок). Эффектов нет.
    WhiteAxisSelected(WhiteAxis),
    /// Шкала Кельвинов тянется пальцем: меняется только температура, оттенок остаётся тем, что держит движок.
    WhiteTemperatureDragged(ColorTemperature),
    /// Шкала оттенка тянется пальцем: меняется только оттенок.
    WhiteTintDragged(Tint),
    /// Исполнитель не доставил команду «взять» (очередь полна, движка нет): для человека это отказ.
    WhiteTakeNotDelivered,
    /// Рендер потерял спрос на прибор (экран UI переустановлен, 14h): экран шлёт запрос заново.
    ScopeDemandLost,
}

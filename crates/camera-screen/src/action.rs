//! Всё, что меняет состояние экрана: жесты, клавиши, факты камеры, итоги снимка, датчики, время.
//!
//! Моменты времени приходят **в действиях** (метка касания или vsync): редьюсер часов не читает.

use shell_contract::clock::MonotonicNanos;
use shell_contract::gravity::GravityVector;
use shell_contract::orientation::DeviceOrientation;
use shell_contract::surface::SurfaceSize;
use ui_core::geometry::Point;

use camera_contract::control::ExposureBias;
use camera_contract::zoom::ZoomRatio;

use crate::facts::CameraFacts;
use crate::modes::CaptureMode;
use crate::shutter::{CaptureOutcome, ShotRequestId};
use crate::tools::{ToolId, ToolOption};

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
    /// Итог снимка от сервиса съёмки.
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
}

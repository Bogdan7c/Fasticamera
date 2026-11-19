//! Всё, что меняет состояние экрана: жесты, клавиши, факты камеры, итоги снимка, датчики, время.
//!
//! Моменты времени приходят **в действиях** (метка касания или vsync): редьюсер часов не читает.

use shell_contract::clock::MonotonicNanos;
use shell_contract::gravity::GravityVector;
use shell_contract::orientation::DeviceOrientation;
use shell_contract::surface::SurfaceSize;
use ui_core::geometry::Point;

use camera_contract::control::ExposureBias;

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

/// Действие над экраном.
#[derive(Debug, Clone, Copy, PartialEq)]
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
    /// Выбор варианта в подменю.
    SubmenuOptionSelected(ToolOption),
    /// Назад из подменю в меню.
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
}

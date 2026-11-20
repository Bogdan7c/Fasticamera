//! Данные жеста пана в намерения элемента: один переход «событие арены → вызов элемента».
//!
//! Экран подписывает на элемент распознаватель `Pan` в [`ui_core::scene::UiScene`], забирает
//! [`ui_core::gesture::GestureEvent`] и передаёт элементу его данные. Без этого модуля экрану пришлось бы
//! самому разбирать `(state, data)` и помнить различия сигнатур (`drag_moved` шкалы принимает
//! время, а карусели — нет; у каждого метода свой порядок аргументов). Здесь это разобрано один раз:
//!
//! ```ignore
//! for event in scene.drain_gesture_events() {
//!     if let Some(update) = PanUpdate::from_gesture(event.state, event.data)? {
//!         // Время — момент события (метка касания), а не время кадра: скорость отпускания
//!         // считается на момент отрыва, пружина стартует от него.
//!         carousel.apply_pan(event.time, update)?;
//!     }
//! }
//! ```
//!
//! # Что делает с отменой
//! [`PanUpdate::Cancelled`] означает «сбросить без действия» (`shell_contract::touch::TouchPhase::Cancelled`).
//! Как именно элемент это выполняет, описано в его документации: карусель возвращается к тому, что
//! было выбрано до жеста; шкала, значение которой менялось вживую, доезжает до упора, как после
//! отпускания без скорости.

use thiserror::Error;
use ui_core::geometry::Vector;
use ui_core::gesture::{GestureData, GestureState, PanTranslation};

/// Что произошло с паном, в терминах, которые понимает элемент.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PanUpdate {
    /// Пан распознан: контент берётся пальцем.
    Began { translation: PanTranslation },
    /// Палец сдвинулся.
    Changed { translation: PanTranslation },
    /// Палец отпущен со скоростью `velocity` (pt/с, экранные оси: `x` вправо, `y` вниз).
    Ended { velocity: Vector },
    /// Система отменила жест: сбросить без действия.
    Cancelled,
}

/// Ошибка разбора события жеста: экран подписал на пан не тот распознаватель.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum PanUpdateError {
    /// Состояние `Began`/`Changed`/`Ended`, но данные не пана (например, это событие тапа).
    #[error("событие {state:?} пришло без данных пана: на элемент подписан не `Pan`")]
    NotPanData { state: GestureState },
    /// То же для долгого тапа, который ведёт шкалу (колесо зума тем же касанием).
    #[error("событие {state:?} пришло без данных долгого тапа: на элемент подписан не `LongPress`")]
    NotLongPressData { state: GestureState },
}

impl PanUpdate {
    /// Превратить состояние и данные события арены в обновление пана.
    ///
    /// * `Ok(None)` — для этого события элементу нечего делать (`Failed`: пан провален и никогда
    ///   не начинался; `Possible` арена не публикует, но и ему делать нечего);
    /// * `Err` — состояние требует данных пана, а их нет: ошибка подписки на экране, молча
    ///   игнорировать её нельзя.
    pub fn from_gesture(state: GestureState, data: GestureData) -> Result<Option<Self>, PanUpdateError> {
        match (state, data) {
            (GestureState::Failed | GestureState::Possible, _) => Ok(None),
            // Отмена данных не требует: она может прийти и до начала жеста (`GestureData::None`).
            (GestureState::Cancelled, _) => Ok(Some(PanUpdate::Cancelled)),
            (GestureState::Began, GestureData::Pan { translation, .. }) => Ok(Some(PanUpdate::Began { translation })),
            (GestureState::Changed, GestureData::Pan { translation, .. }) => {
                Ok(Some(PanUpdate::Changed { translation }))
            }
            (GestureState::Ended, GestureData::Pan { velocity, .. }) => Ok(Some(PanUpdate::Ended { velocity })),
            (GestureState::Began | GestureState::Changed | GestureState::Ended, _) => {
                Err(PanUpdateError::NotPanData { state })
            }
        }
    }

    /// Долгий тап, который после срабатывания ведёт элемент за пальцем (колесо зума: «удержи и води»).
    /// После `Began` долгий тап продолжает отдавать смещения, поэтому для элемента это тот же пан.
    ///
    /// У долгого тапа нет скорости отпускания: `Ended` приходит с нулевой скоростью, то есть шкала
    /// останавливается там, где отпущен палец (магнит опорного значения сработает), без броска.
    pub fn from_long_press(state: GestureState, data: GestureData) -> Result<Option<Self>, PanUpdateError> {
        match (state, data) {
            (GestureState::Failed | GestureState::Possible, _) => Ok(None),
            (GestureState::Cancelled, _) => Ok(Some(PanUpdate::Cancelled)),
            (GestureState::Began, GestureData::LongPress { translation, .. }) => {
                Ok(Some(PanUpdate::Began { translation }))
            }
            (GestureState::Changed, GestureData::LongPress { translation, .. }) => {
                Ok(Some(PanUpdate::Changed { translation }))
            }
            (GestureState::Ended, GestureData::LongPress { .. }) => {
                Ok(Some(PanUpdate::Ended { velocity: Vector::ZERO }))
            }
            (GestureState::Began | GestureState::Changed | GestureState::Ended, _) => {
                Err(PanUpdateError::NotLongPressData { state })
            }
        }
    }
}

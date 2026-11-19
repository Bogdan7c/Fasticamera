//! События шкалы: то, что элемент сообщает наружу (вида нет, рисует `ui-render-vulkan`).

use shell_contract::haptics::HapticPattern;

use crate::drive::MotionPhase;
use crate::event_queue::CarriesHaptic;
use crate::value::Emphasis;

/// В какую сторону шкала прошла рубеж.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TravelDirection {
    /// К большим положениям.
    Increasing,
    /// К меньшим положениям.
    Decreasing,
}

/// Событие шкалы. Приходят из [`super::SnapScale::drain_events`] в порядке возникновения.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ScaleEvent<V> {
    /// Значение изменилось (у ступенчатой модели — на ближайший шаг, у плавной — на каждом кадре).
    ValueChanged { value: V },
    /// Пройден обычный тик.
    TickCrossed { direction: TravelDirection },
    /// Пройден крупный упор (опорное значение, «ноль»).
    ReferenceCrossed { direction: TravelDirection },
    /// Что сыграть пальцу. **Один** паттерн на кадр, сколько бы рисок ни пройдено за кадр:
    /// сильнейший из пройденных (опорное значение сильнее тика). Только для движения от пальца:
    /// программный сброс значения не вибрирует.
    Haptic { pattern: HapticPattern },
    /// Шкала успокоилась на `value`.
    Settled { value: V },
}

/// Срез состояния шкалы на момент `now` — для рисования и проверок.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScaleSnapshot<V> {
    /// Смещение контента, pt (растёт вместе со значением; за краем включает «резинку»).
    pub offset_points: f64,
    /// Положение в единицах модели.
    pub position_units: f64,
    pub value: V,
    pub phase: MotionPhase,
}

/// Риска для рисования.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScaleMark {
    /// Положение риски в тех же pt, что `ScaleSnapshot::offset_points`.
    pub offset_points: f64,
    pub emphasis: Emphasis,
}

/// Итог программной установки значения.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SetValueOutcome<V> {
    /// Значение принято как есть.
    Applied,
    /// Значение было вне пределов (или между шагами): применено ближайшее допустимое.
    Adjusted { applied: V },
}

impl<V> CarriesHaptic for ScaleEvent<V> {
    fn haptic(pattern: HapticPattern) -> Self {
        ScaleEvent::Haptic { pattern }
    }
}

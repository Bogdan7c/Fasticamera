//! Состояния распознавателей, данные жестов и события арены.

use serde::Serialize;

use crate::geometry::{Point, Vector};
use crate::id::NodeId;
use crate::time::MonotonicNanos;

/// Идентификатор распознавателя. Выдаётся ареной по возрастанию и **не переиспользуется**, поэтому
/// порядок идентификаторов = порядок добавления (он же второй ключ приоритета).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct RecognizerId(pub(crate) u32);

impl RecognizerId {
    /// Порядковый номер добавления (для логов и тестов).
    pub const fn ordinal(self) -> u32 {
        self.0
    }
}

/// Состояние распознавателя (модель UIKit, `behavior.md` 1.1).
///
/// Дискретные жесты (тап, свайп): `Possible → Ended | Failed`. Непрерывные (пан, щипок, долгий тап,
/// нажатие): `Possible → Began → Changed* → Ended | Cancelled`, а до `Began` возможен `Failed`.
/// `Possible` событием не публикуется: после терминального состояния распознаватель молча
/// возвращается в него, когда оторваны все пальцы.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum GestureState {
    Possible,
    Began,
    Changed,
    Ended,
    Cancelled,
    Failed,
}

impl GestureState {
    /// Жест идёт: начался, но не закончился.
    pub const fn is_active(self) -> bool {
        matches!(self, Self::Began | Self::Changed)
    }

    /// Конечное состояние последовательности касания (до сброса в `Possible`).
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Ended | Self::Cancelled | Self::Failed)
    }
}

/// Направление свайпа.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SwipeDirection {
    Left,
    Right,
    Up,
    Down,
}

/// Данные жеста на момент события. Вариант соответствует виду распознавателя.
///
/// Для `Failed` данных нет ([`GestureData::None`]); для `Cancelled` отдаются последние данные
/// жеста (или `None`, если он ещё не начинался).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GestureData {
    /// Данных нет (переход в `Failed`, отмена до начала).
    None,
    /// Тап распознан; `tap_count` — сколько тапов сложилось (1 или 2).
    Tap { position: Point, tap_count: u8 },
    /// Долгий тап; смещения нужны, потому что после `Began` палец может тянуть колесо зума.
    LongPress {
        position: Point,
        translation: PanTranslation,
    },
    /// Перетаскивание.
    Pan {
        position: Point,
        translation: PanTranslation,
        velocity: Vector,
    },
    /// Щипок: масштаб относительно начального расстояния, его скорость (1/с), середина между пальцами.
    Pinch {
        scale: f64,
        scale_velocity: f64,
        focus: Point,
    },
    /// Свайп распознан по ходу движения.
    Swipe {
        direction: SwipeDirection,
        position: Point,
        velocity: Vector,
    },
    /// Нажатие кнопки: положение пальца (внутри ли рамки — решает элемент).
    Press { position: Point },
}

/// Два смещения пальца: ведение 1:1 без скачка гистерезиса (решение координатора P2).
///
/// * `from_touch_down` — от точки касания, как `translation(in:)` в UIKit; на `Began` уже равно
///   пройденному гистерезису;
/// * `from_recognition` — от точки, где жест начался (`Began`); на `Began` равно нулю. Элемент,
///   который хочет двигаться за пальцем плавно, берёт его.
///
/// Какое использовать — решает `ui-kit`; арена отдаёт оба.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PanTranslation {
    pub from_touch_down: Vector,
    pub from_recognition: Vector,
}

impl PanTranslation {
    /// Смещение от касания; от распознавания пока ноль (арена заполнит при `Began`).
    pub(crate) fn from_touch_only(from_touch_down: Vector) -> Self {
        Self {
            from_touch_down,
            from_recognition: Vector::ZERO,
        }
    }
}

impl GestureData {
    /// Положение пальца в данных, если оно есть.
    pub fn position(&self) -> Option<Point> {
        match *self {
            Self::Tap { position, .. }
            | Self::LongPress { position, .. }
            | Self::Pan { position, .. }
            | Self::Swipe { position, .. }
            | Self::Press { position } => Some(position),
            Self::Pinch { focus, .. } => Some(focus),
            Self::None => None,
        }
    }

    /// Те же данные, но смещение `from_recognition` посчитано от `origin` — положения на `Began`.
    pub(crate) fn with_recognition_origin(self, origin: Point) -> Self {
        match self {
            Self::LongPress { position, translation } => Self::LongPress {
                position,
                translation: PanTranslation {
                    from_recognition: position.offset_from(origin),
                    ..translation
                },
            },
            Self::Pan {
                position,
                translation,
                velocity,
            } => Self::Pan {
                position,
                translation: PanTranslation {
                    from_recognition: position.offset_from(origin),
                    ..translation
                },
                velocity,
            },
            other => other,
        }
    }
}

/// Событие арены: распознаватель сменил состояние. Потребитель игнорирует `Failed`, но оно
/// публикуется — нужно тестам и кнопке (снять подсветку).
#[derive(Debug, Clone, PartialEq)]
pub struct GestureEvent<I> {
    pub recognizer: RecognizerId,
    pub node: NodeId,
    pub intent: I,
    /// Новое состояние (никогда не `Possible`).
    pub state: GestureState,
    pub data: GestureData,
    /// Момент входа, вызвавшего событие: метка касания или время кадра.
    pub time: MonotonicNanos,
}

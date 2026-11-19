//! Спецификации распознавателей: закрытый набор видов и их пороги.
//!
//! Распознаватели — данные, а не объекты с замыканиями: вид выбирается вариантом
//! [`RecognizerSpec`], пороги лежат в полях и читаются из конфига экрана. Значения по умолчанию
//! взяты из `docs/ios-camera/behavior.md` раздел 1 (метки происхождения у каждого поля).

use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::state::SwipeDirection;
use crate::geometry::Vector;
use crate::time::UiDuration;

/// Что делать однопальцевому распознавателю, когда на экран лёг второй палец
/// (`behavior.md` 1.2: «2 пальца → щипок сразу, остальные отменяются»).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtraFingers {
    /// Возможный жест проваливается, идущий отменяется. Значение по умолчанию.
    #[default]
    FailOrCancel,
    /// Лишние пальцы не замечаются, жест следит за первым.
    Ignore,
}

/// Сколько тапов нужно подряд.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TapCount {
    #[default]
    Single,
    Double,
}

impl TapCount {
    pub(crate) const fn required_taps(self) -> u8 {
        match self {
            Self::Single => 1,
            Self::Double => 2,
        }
    }
}

/// Тап (одиночный или двойной).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct TapSpec {
    pub taps: TapCount,
    /// Сдвиг пальца, после которого это уже не тап, pt. `[estimate]` 10 = порог pan (`behavior.md` 1,
    /// «Тап: смещение»). Подгонка, не Apple. Сравнение строгое: «больше 10 pt».
    pub max_movement_pt: f64,
    /// Наибольший промежуток между отрывом первого тапа и касанием второго, мс. `[reverse]` 300:
    /// Android `ViewConfiguration.DOUBLE_TAP_TIMEOUT` (AOSP `core/java/android/view/ViewConfiguration.java`,
    /// читалось 19.11.2026). В документации Apple числа нет; WWDC18 803 (56:17) упоминает задержку
    /// одиночного тапа «около полсекунды» `[doc]` — возможно, стоит поднять значение. Подгонка, не Apple.
    pub max_interval_between_taps_ms: u32,
    /// Наибольшее расстояние между точками двух касаний, pt. `[reverse]` 100: Android
    /// `DOUBLE_TAP_SLOP` = 100 dp (тот же файл). Подгонка, не Apple.
    pub max_distance_between_taps_pt: f64,
    pub extra_fingers: ExtraFingers,
}

impl Default for TapSpec {
    fn default() -> Self {
        Self {
            taps: TapCount::Single,
            max_movement_pt: 10.0,
            max_interval_between_taps_ms: 300,
            max_distance_between_taps_pt: 100.0,
            extra_fingers: ExtraFingers::FailOrCancel,
        }
    }
}

/// Долгий тап (непрерывный).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct LongPressSpec {
    /// Сколько держать, мс. `[doc]` 500: `UILongPressGestureRecognizer.minimumPressDuration`,
    /// *«The default duration is 0.5 seconds.»* Начало жеста — первый кадр, время которого >= старт + это.
    pub min_duration_ms: u32,
    /// Допустимый сдвиг до начала, pt. `[doc]` 10: `allowableMovement`, *«The default distance is 10 points.»*
    pub allowable_movement_pt: f64,
    pub extra_fingers: ExtraFingers,
}

impl Default for LongPressSpec {
    fn default() -> Self {
        Self {
            min_duration_ms: 500,
            allowable_movement_pt: 10.0,
            extra_fingers: ExtraFingers::FailOrCancel,
        }
    }
}

/// Какое направление принимает перетаскивание.
///
/// Решается по **доминирующей оси** в момент превышения гистерезиса: если `|dx| >= |dy|` — ось
/// горизонтальная, иначе вертикальная; при ничьей считается горизонталь. Направленные варианты
/// (`Left` и т. д.) дополнительно требуют знак смещения: «пан влево» затвора. Ось `y` идёт вниз,
/// поэтому `Up` — отрицательное `dy`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PanDirection {
    #[default]
    Any,
    Horizontal,
    Vertical,
    Left,
    Right,
    Up,
    Down,
}

impl PanDirection {
    /// Подходит ли смещение `displacement` (в момент превышения гистерезиса).
    pub fn accepts(self, displacement: Vector) -> bool {
        let horizontal = displacement.dx.abs() >= displacement.dy.abs();
        match self {
            Self::Any => true,
            Self::Horizontal => horizontal,
            Self::Vertical => !horizontal,
            Self::Left => horizontal && displacement.dx < 0.0,
            Self::Right => horizontal && displacement.dx > 0.0,
            Self::Up => !horizontal && displacement.dy < 0.0,
            Self::Down => !horizontal && displacement.dy > 0.0,
        }
    }
}

/// Перетаскивание (непрерывное).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct PanSpec {
    pub direction: PanDirection,
    /// Гистерезис — смещение, после которого начинается перетаскивание, pt. `[doc]` 10: WWDC18 803
    /// (51:52) *«This distance is called hysteresis, and is usually 10 points in iOS.»* Строгое «больше».
    pub hysteresis_pt: f64,
    pub extra_fingers: ExtraFingers,
}

impl Default for PanSpec {
    fn default() -> Self {
        Self {
            direction: PanDirection::Any,
            hysteresis_pt: 10.0,
            extra_fingers: ExtraFingers::FailOrCancel,
        }
    }
}

/// Щипок двумя пальцами (непрерывный).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct PinchSpec {
    /// Изменение расстояния между пальцами для старта, pt. `[estimate]` 10 (`behavior.md` 1, «Порог
    /// начала щипка»: в документации не найден). Сравнение «не меньше». Подгонка, не Apple.
    pub start_distance_change_pt: f64,
}

impl Default for PinchSpec {
    fn default() -> Self {
        Self {
            start_distance_change_pt: 10.0,
        }
    }
}

/// Свайп (дискретный): решается по ходу движения, не по отрыву (WWDC18 803, 54:42).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct SwipeSpec {
    /// Принимаемые направления.
    pub directions: Vec<SwipeDirection>,
    /// Минимальная дистанция по доминирующей оси, pt. `[estimate]` 30. Подгонка, не Apple.
    pub min_distance_pt: f64,
    /// Наибольшая длительность от касания, мс. `[estimate]` 400. Подгонка, не Apple.
    pub max_duration_ms: u32,
    /// Минимальная скорость вдоль направления, pt/с. `[estimate]` 200. Подгонка, не Apple.
    pub min_velocity_pt_per_s: f64,
    pub extra_fingers: ExtraFingers,
}

impl Default for SwipeSpec {
    fn default() -> Self {
        Self {
            directions: vec![
                SwipeDirection::Left,
                SwipeDirection::Right,
                SwipeDirection::Up,
                SwipeDirection::Down,
            ],
            min_distance_pt: 30.0,
            max_duration_ms: 400,
            min_velocity_pt_per_s: 200.0,
            extra_fingers: ExtraFingers::FailOrCancel,
        }
    }
}

/// Нажатие кнопки: подсветка сразу при касании, срабатывание по отпусканию (`behavior.md` 1.1,
/// WWDC18 803, 50:24). **Не эксклюзивно**: стартует на касании и отменяется, когда любой
/// эксклюзивный распознаватель последовательности начался (как `cancelsTouchesInView`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct PressSpec {
    pub extra_fingers: ExtraFingers,
}

impl Default for PressSpec {
    fn default() -> Self {
        Self {
            extra_fingers: ExtraFingers::FailOrCancel,
        }
    }
}

/// Вид и пороги распознавателя.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecognizerSpec {
    Tap(TapSpec),
    LongPress(LongPressSpec),
    Pan(PanSpec),
    Pinch(PinchSpec),
    Swipe(SwipeSpec),
    Press(PressSpec),
}

/// Ошибка конфигурации распознавателя.
#[derive(Debug, Clone, PartialEq, Error)]
pub enum SpecError {
    #[error("поле `{field}` должно быть конечным и > 0, получено {value}")]
    NonPositive { field: &'static str, value: f64 },
    #[error("поле `{field}` должно быть > 0 мс")]
    ZeroDuration { field: &'static str },
    #[error("свайп без единого направления никогда не сработает")]
    EmptySwipeDirections,
}

fn require_positive(field: &'static str, value: f64) -> Result<(), SpecError> {
    if value.is_finite() && value > 0.0 {
        Ok(())
    } else {
        Err(SpecError::NonPositive { field, value })
    }
}

fn require_nonzero_ms(field: &'static str, millis: u32) -> Result<(), SpecError> {
    if millis > 0 {
        Ok(())
    } else {
        Err(SpecError::ZeroDuration { field })
    }
}

impl RecognizerSpec {
    /// Проверка порогов.
    pub fn validate(&self) -> Result<(), SpecError> {
        match self {
            Self::Tap(spec) => {
                require_positive("max_movement_pt", spec.max_movement_pt)?;
                require_positive("max_distance_between_taps_pt", spec.max_distance_between_taps_pt)?;
                require_nonzero_ms("max_interval_between_taps_ms", spec.max_interval_between_taps_ms)
            }
            Self::LongPress(spec) => {
                require_nonzero_ms("min_duration_ms", spec.min_duration_ms)?;
                require_positive("allowable_movement_pt", spec.allowable_movement_pt)
            }
            Self::Pan(spec) => require_positive("hysteresis_pt", spec.hysteresis_pt),
            Self::Pinch(spec) => require_positive("start_distance_change_pt", spec.start_distance_change_pt),
            Self::Swipe(spec) => {
                if spec.directions.is_empty() {
                    return Err(SpecError::EmptySwipeDirections);
                }
                require_positive("min_distance_pt", spec.min_distance_pt)?;
                require_nonzero_ms("max_duration_ms", spec.max_duration_ms)?;
                require_positive("min_velocity_pt_per_s", spec.min_velocity_pt_per_s)
            }
            Self::Press(_) => Ok(()),
        }
    }

    /// Эксклюзивность: победа такого распознавателя проваливает/отменяет остальных. Не эксклюзивно
    /// только нажатие кнопки.
    pub fn is_exclusive(&self) -> bool {
        !matches!(self, Self::Press(_))
    }

    /// Дискретный жест сразу уходит в `Ended` (без `Began`/`Changed`).
    pub fn is_discrete(&self) -> bool {
        matches!(self, Self::Tap(_) | Self::Swipe(_))
    }

    /// Правило второго пальца; `None` у щипка — он сам работает с двумя пальцами.
    pub fn extra_fingers(&self) -> Option<ExtraFingers> {
        match self {
            Self::Tap(spec) => Some(spec.extra_fingers),
            Self::LongPress(spec) => Some(spec.extra_fingers),
            Self::Pan(spec) => Some(spec.extra_fingers),
            Self::Swipe(spec) => Some(spec.extra_fingers),
            Self::Press(spec) => Some(spec.extra_fingers),
            Self::Pinch(_) => None,
        }
    }
}

/// Длительность из миллисекунд конфига.
pub(crate) fn millis(value: u32) -> UiDuration {
    UiDuration::from_millis(i64::from(value))
}

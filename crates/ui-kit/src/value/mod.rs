//! Модели значений (`components.md`, «Модели значений»): общая основа для шкал K3, выбора K5/K6.
//!
//! Модель связывает **значение** (то, что нужно экрану: кратность, EV, выдержка) и **положение
//! на шкале** (число «единиц»; сколько pt в единице, решает токен плотности). Модель не знает ни
//! о камере, ни о пикселях.
//!
//! # Инварианты
//! * Положение — **абсолютное**: не зависит от пределов. Смена пределов (другая камера) меняет только
//!   допустимый диапазон положений, а положение уже выбранного значения остаётся тем же. Поэтому
//!   шкала при смене пределов не «прыгает», а движок прокрутки плавно уводит её от нового края.
//! * Пределы приходят снаружи ([`ValueModel::set_limits`]); модель заранее их не знает.
//! * **Правило вне пределов:** значение за пределами зажимается к ближайшей границе
//!   ([`ValueModel::clamp_value`]); значение между шагами округляется до ближайшего шага.
//!   Значение, которое модель вообще не может представить (не число, нет в ряду), — ошибка
//!   [`ValueError`], а не молчаливая подмена.
//! * Шаги хранятся **индексом**, а не суммой дробей: значение = `индекс × шаг`, ошибка не копится.
//!
//! # Состав
//! [`QuantizedLinear`] (диапазон, шаг, «ноль»), [`Logarithmic`] (диапазон, опорные точки),
//! [`ValueSeries`] (список допустимых значений), [`AxisPair`] (две оси: кельвины + оттенок);
//! непрерывные оси без ступеней (значение от пальца не округляется): [`ContinuousLinear`] (оттенок ±50) и
//! [`MiredKelvin`] (Кельвины снаружи, шкала равномерна по майредам).

mod continuous;
mod linear;
mod logarithmic;
mod mired;
mod pair;
mod series;

use std::fmt::Debug;

use thiserror::Error;

pub use continuous::{ContinuousLinear, ExactPosition, RulerAxis};
pub use linear::{LinearLimits, QuantizedLinear, ReferenceValue};
pub use logarithmic::{LogLimits, Logarithmic, LogarithmicSpec};
pub use mired::{MiredKelvin, mired_of_kelvin};
pub use pair::{AxisPair, PairValue};
pub use series::{SeriesLimits, ValueSeries};

/// Допуск сравнения долей шага/октавы при переводе значения в индекс (числовая устойчивость:
/// `-2 / (1/3)` даёт `-6.000000000000001`, и без допуска предел `-2` потерял бы шаг `-6`).
pub(crate) const INDEX_EPSILON: f64 = 1e-9;

/// Предел числа рисок модели по умолчанию. Инженерная защита (не число Apple): пределы или шаг,
/// дающие больше рисок, — явная ошибка [`ValueError::TooManyMarks`], а не аллокация миллиардов
/// элементов. Для своего предела — `with_max_marks` у модели.
pub const DEFAULT_MAX_MARKS: u32 = 100_000;

/// Наибольший по модулю индекс шага, который ещё точно представим в `f64`/`i64` (2^53).
pub(crate) const MAX_EXACT_INDEX: f64 = 9_007_199_254_740_992.0;

/// Проверить, что число рисок `count` (может быть нецелым, бесконечным или NaN) в бюджете.
pub(crate) fn ensure_mark_budget(count: f64, max_marks: u32) -> Result<(), ValueError> {
    // Записано через отрицание: NaN тоже не проходит.
    if count <= f64::from(max_marks) {
        Ok(())
    } else {
        Err(ValueError::TooManyMarks { count, max: max_marks })
    }
}

/// Насколько выделен тик/упор. От этого зависит хаптика и толщина риски (рисует `ui-render-vulkan`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Emphasis {
    /// Обычный тик (мелкий упор).
    Tick,
    /// Опорное значение (крупный упор): «ноль» линейной шкалы, якорь логарифмической.
    Reference,
}

/// Риска шкалы: положение в единицах модели и выделение.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Mark {
    pub position: f64,
    pub emphasis: Emphasis,
}

/// Как модель превращает положение в значение.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Quantization {
    /// Значение всегда на одном из шагов: между шагами выбирается ближайший (EV, ряд).
    Stepped,
    /// Значение плавное; риски и опорные точки — только ориентиры (зум).
    Continuous,
}

/// Ошибка модели значений.
#[derive(Debug, Clone, PartialEq, Error)]
pub enum ValueError {
    /// Значение не конечно (NaN или бесконечность).
    #[error("значение не конечно: {0}")]
    NonFinite(f64),
    /// Параметр описания модели некорректен.
    #[error("параметр модели «{name}» некорректен: {reason}")]
    InvalidSpec { name: &'static str, reason: &'static str },
    /// Пределы некорректны (в них нет ни одного допустимого значения).
    #[error("пределы некорректны: {reason}")]
    InvalidLimits { reason: &'static str },
    /// Пределы и шаг дают больше рисок, чем разрешено (`count` — оценка, может быть огромной).
    #[error("пределы дают {count} рисок, разрешено не больше {max}")]
    TooManyMarks { count: f64, max: u32 },
    /// Значения нет в ряду допустимых.
    #[error("значения нет в ряду допустимых")]
    NotInSeries,
}

/// Модель значений. Реализации: [`QuantizedLinear`], [`Logarithmic`], [`ValueSeries`].
pub trait ValueModel {
    /// Значение, которым оперирует экран.
    type Value: Copy + PartialEq + Debug;
    /// Пределы, приходящие снаружи.
    type Limits: Clone + PartialEq + Debug;

    /// Текущие пределы.
    fn limits(&self) -> &Self::Limits;

    /// Сменить пределы. При ошибке модель **не меняется**.
    fn set_limits(&mut self, limits: Self::Limits) -> Result<(), ValueError>;

    /// Ступенчатая или плавная модель.
    fn quantization(&self) -> Quantization;

    /// Крайние положения в пределах `(наименьшее, наибольшее)`, единицы модели.
    fn position_range(&self) -> (f64, f64);

    /// Значение после применения правила «вне пределов» (см. документацию модуля).
    fn clamp_value(&self, value: Self::Value) -> Result<Self::Value, ValueError>;

    /// Положение значения (после `clamp_value`), единицы модели.
    fn position_of(&self, value: Self::Value) -> Result<f64, ValueError>;

    /// Значение в положении `position`. Положение за пределами зажимается к границе (резинка
    /// шкалы показывает растяжение, а значение остаётся крайним); не конечное — к нижней границе.
    fn value_at(&self, position: f64) -> Self::Value;

    /// Риски в пределах, по возрастанию положения.
    fn marks(&self) -> Vec<Mark>;
}

/// Положение `position` в целочисленный индекс шага, зажатое в `[first, last]`.
pub(crate) fn nearest_index(position: f64, first: i64, last: i64) -> i64 {
    if !position.is_finite() {
        return first;
    }
    // `as` насыщается на границах i64, дальше зажим в диапазон.
    (position.round() as i64).clamp(first, last)
}

pub(crate) fn ensure_finite(value: f64) -> Result<(), ValueError> {
    if value.is_finite() {
        Ok(())
    } else {
        Err(ValueError::NonFinite(value))
    }
}

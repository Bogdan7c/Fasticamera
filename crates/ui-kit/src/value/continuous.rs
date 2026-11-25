//! Непрерывные линейные оси: значение берётся ровно с того места шкалы, где стоит палец.
//!
//! Чем это отличается от [`super::QuantizedLinear`]: у неё значение — всегда целое число шагов (шкала
//! притягивается к ступеням). Здесь ступеней нет: положение на шкале и значение связаны гладкой формулой, а
//! риски — только ориентиры. Поэтому
//! * значение от пальца **не округляется**: сдвинул на 1 pt — значение изменилось ровно на цену 1 pt;
//! * после отпускания шкала **не доезжает до риски** и не притягивается магнитом (у модели нет опорных рисок,
//!   а притяжение «к риске» включается только для них);
//! * программная установка показывает значение **точно** ([`ExactPosition`]).
//!
//! Модуль содержит общую часть непрерывных осей — [`grid_marks`] и [`ContinuousLinear`]; ось «майреды с
//! Кельвинами наружу» лежит рядом, в [`super::MiredKelvin`].

use super::{
    DEFAULT_MAX_MARKS, Emphasis, INDEX_EPSILON, LinearLimits, MAX_EXACT_INDEX, Mark, Quantization, ValueError,
    ValueModel, ensure_finite, ensure_mark_budget,
};

/// Модель, у которой положение на шкале связано со значением **без округления**: `position_of(value)` и
/// `value_at(position)` взаимно обратны (с точностью плавающей арифметики), а программа вправе показать любое
/// значение между рисками. Маркер нужен элементам, чтобы разрешать «точную» установку только там, где она
/// имеет смысл (у ступенчатой модели такая установка соврала бы о положении).
pub trait ExactPosition: ValueModel {}

/// Модель, которую умеет рисовать линейка ([`crate::ruler::RulerControl`]): знает шаг своих рисок.
///
/// Линейка считает «шагами» расстояние между соседними рисками: от него зависят затухание рисок к краям и
/// период крупной риски. У ступенчатой модели шаг риски — одна единица положения; у непрерывной — её собственный
/// шаг сетки рисок (5 майред, 5 единиц оттенка).
pub trait RulerAxis: ValueModel + Clone {
    /// Расстояние между соседними рисками в единицах положения модели. Конечное и > 0.
    fn tick_step_units(&self) -> f64;
}

/// Риски равномерной сетки в абсолютных положениях `k × spacing`, попавшие в `[lowest, highest]`.
///
/// Сетка привязана к нулю положения, а не к краю пределов: смена пределов (другая камера) не сдвигает риски,
/// они остаются на тех же местах. Все риски обычные ([`Emphasis::Tick`]): у непрерывной оси нет «опорных»
/// значений, к которым шкала могла бы притянуться.
pub(super) fn grid_marks(lowest: f64, highest: f64, spacing: f64) -> Vec<Mark> {
    let first_index = (lowest / spacing - INDEX_EPSILON).ceil() as i64;
    let last_index = (highest / spacing + INDEX_EPSILON).floor() as i64;
    (first_index..=last_index)
        .map(|index| Mark {
            position: index as f64 * spacing,
            emphasis: Emphasis::Tick,
        })
        .collect()
}

/// Проверка шага сетки рисок и числа рисок на отрезке положений `[lowest, highest]`.
pub(super) fn ensure_grid(spacing: f64, lowest: f64, highest: f64, max_marks: u32) -> Result<(), ValueError> {
    ensure_finite(spacing)?;
    if spacing <= 0.0 {
        return Err(ValueError::InvalidSpec {
            name: "tick_spacing",
            reason: "шаг рисок должен быть > 0",
        });
    }
    if lowest.abs() / spacing > MAX_EXACT_INDEX || highest.abs() / spacing > MAX_EXACT_INDEX {
        return Err(ValueError::InvalidLimits {
            reason: "пределы слишком велики для шага рисок",
        });
    }
    ensure_mark_budget((highest - lowest) / spacing + 1.0, max_marks)
}

/// Непрерывная линейная ось: положение = значение. Подходит для величин, которые тянутся равномерно
/// (оттенок −50…+50), где «единица положения» уже есть единица значения.
#[derive(Debug, Clone, PartialEq)]
pub struct ContinuousLinear {
    tick_spacing: f64,
    limits: LinearLimits,
    max_marks: u32,
}

impl ContinuousLinear {
    /// Ось с шагом сетки рисок `tick_spacing` (в единицах значения) и пределом числа рисок [`DEFAULT_MAX_MARKS`].
    /// Ошибки: шаг не конечен или <= 0; пределы не конечны или `min >= max`; рисок больше предела.
    pub fn new(tick_spacing: f64, limits: LinearLimits) -> Result<Self, ValueError> {
        Self::with_max_marks(tick_spacing, limits, DEFAULT_MAX_MARKS)
    }

    /// То же со своим пределом числа рисок.
    pub fn with_max_marks(tick_spacing: f64, limits: LinearLimits, max_marks: u32) -> Result<Self, ValueError> {
        Self::validate(tick_spacing, limits, max_marks)?;
        Ok(Self {
            tick_spacing,
            limits,
            max_marks,
        })
    }

    /// Шаг сетки рисок в единицах значения.
    pub fn tick_spacing(&self) -> f64 {
        self.tick_spacing
    }

    fn validate(tick_spacing: f64, limits: LinearLimits, max_marks: u32) -> Result<(), ValueError> {
        ensure_finite(limits.min)?;
        ensure_finite(limits.max)?;
        if limits.min >= limits.max {
            return Err(ValueError::InvalidLimits {
                reason: "нужно min < max",
            });
        }
        ensure_grid(tick_spacing, limits.min, limits.max, max_marks)
    }
}

impl ValueModel for ContinuousLinear {
    type Value = f64;
    type Limits = LinearLimits;

    fn limits(&self) -> &LinearLimits {
        &self.limits
    }

    fn set_limits(&mut self, limits: LinearLimits) -> Result<(), ValueError> {
        Self::validate(self.tick_spacing, limits, self.max_marks)?;
        self.limits = limits;
        Ok(())
    }

    fn quantization(&self) -> Quantization {
        Quantization::Continuous
    }

    fn position_range(&self) -> (f64, f64) {
        (self.limits.min, self.limits.max)
    }

    fn clamp_value(&self, value: f64) -> Result<f64, ValueError> {
        ensure_finite(value)?;
        Ok(value.clamp(self.limits.min, self.limits.max))
    }

    fn position_of(&self, value: f64) -> Result<f64, ValueError> {
        self.clamp_value(value)
    }

    fn value_at(&self, position: f64) -> f64 {
        if position.is_finite() {
            position.clamp(self.limits.min, self.limits.max)
        } else {
            self.limits.min
        }
    }

    fn marks(&self) -> Vec<Mark> {
        grid_marks(self.limits.min, self.limits.max, self.tick_spacing)
    }
}

impl ExactPosition for ContinuousLinear {}

impl RulerAxis for ContinuousLinear {
    fn tick_step_units(&self) -> f64 {
        self.tick_spacing
    }
}

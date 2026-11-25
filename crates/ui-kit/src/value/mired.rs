//! Ось «Кельвины снаружи, майреды на шкале»: равномерна по майредам (1 000 000 / K), больше Кельвинов — дальше
//! по шкале.
//!
//! Почему так. Глаз и плёнка различают цвет света не в Кельвинах, а в майредах: 100 К на 2700 К — заметный
//! сдвиг, 100 К на 6500 К — почти нет. Шкала, равномерная по Кельвинам, была бы «вялой» в тёплой части и
//! «дёрганой» в холодной; по майредам ход пальца даёт одинаково заметный сдвиг цвета на всём диапазоне.
//!
//! * Значение наружу — Кельвины (`f64`); пределы — тоже Кельвины.
//! * Положение — **отрицательные** майреды: `position = −1 000 000 / K`. Знак нужен, чтобы положение росло
//!   вместе с Кельвинами (майреды с ростом Кельвинов падают): тогда «палец влево — значение растёт» работает
//!   так же, как у остальных линеек, а Кельвины на шкале идут слева направо по возрастанию.
//! * Риски — через каждые `tick_spacing_mired` майред на абсолютной сетке (в майредах кратно шагу), обычные:
//!   магнитов у оси нет.
//!
//! Ось не знает слова «белое»: это просто «значение, обратное положению». Что за Кельвины и зачем — решает экран.

use super::continuous::{ExactPosition, RulerAxis, ensure_grid, grid_marks};
use super::{DEFAULT_MAX_MARKS, LinearLimits, Mark, Quantization, ValueError, ValueModel, ensure_finite};

/// Майредов в одном «мегаобратном Кельвине»: `mired = MIRED_NUMERATOR / K`.
const MIRED_NUMERATOR: f64 = 1_000_000.0;

/// Майреды (микрообратные кельвины) света `kelvin`: `1 000 000 / K`. Единственная формула перевода, общая для оси
/// и для экранов, которым нужно сравнивать белое в майредах. `kelvin` должен быть > 0.
pub fn mired_of_kelvin(kelvin: f64) -> f64 {
    MIRED_NUMERATOR / kelvin
}

/// Ось Кельвинов с положением в (отрицательных) майредах. Пределы — `LinearLimits` в Кельвинах, `0 < min < max`.
#[derive(Debug, Clone, PartialEq)]
pub struct MiredKelvin {
    tick_spacing_mired: f64,
    limits: LinearLimits,
    max_marks: u32,
}

impl MiredKelvin {
    /// Ось с шагом рисок `tick_spacing_mired` майред и пределом числа рисок [`DEFAULT_MAX_MARKS`].
    /// Ошибки: шаг не конечен или <= 0; пределы не `0 < min < max`; рисок больше предела.
    pub fn new(tick_spacing_mired: f64, limits_kelvin: LinearLimits) -> Result<Self, ValueError> {
        Self::with_max_marks(tick_spacing_mired, limits_kelvin, DEFAULT_MAX_MARKS)
    }

    /// То же со своим пределом числа рисок.
    pub fn with_max_marks(
        tick_spacing_mired: f64,
        limits_kelvin: LinearLimits,
        max_marks: u32,
    ) -> Result<Self, ValueError> {
        Self::validate(tick_spacing_mired, limits_kelvin, max_marks)?;
        Ok(Self {
            tick_spacing_mired,
            limits: limits_kelvin,
            max_marks,
        })
    }

    /// Шаг сетки рисок, майред.
    pub fn tick_spacing_mired(&self) -> f64 {
        self.tick_spacing_mired
    }

    /// Положение (отрицательные майреды) Кельвинов `kelvin`; `kelvin` > 0.
    fn position_of_kelvin(kelvin: f64) -> f64 {
        -mired_of_kelvin(kelvin)
    }

    fn validate(tick_spacing_mired: f64, limits: LinearLimits, max_marks: u32) -> Result<(), ValueError> {
        ensure_finite(limits.min)?;
        ensure_finite(limits.max)?;
        if limits.min <= 0.0 || limits.min >= limits.max {
            return Err(ValueError::InvalidLimits {
                reason: "нужно 0 < min < max (Кельвины)",
            });
        }
        ensure_grid(
            tick_spacing_mired,
            Self::position_of_kelvin(limits.min),
            Self::position_of_kelvin(limits.max),
            max_marks,
        )
    }
}

impl ValueModel for MiredKelvin {
    type Value = f64;
    type Limits = LinearLimits;

    fn limits(&self) -> &LinearLimits {
        &self.limits
    }

    fn set_limits(&mut self, limits: LinearLimits) -> Result<(), ValueError> {
        Self::validate(self.tick_spacing_mired, limits, self.max_marks)?;
        self.limits = limits;
        Ok(())
    }

    fn quantization(&self) -> Quantization {
        Quantization::Continuous
    }

    fn position_range(&self) -> (f64, f64) {
        (
            Self::position_of_kelvin(self.limits.min),
            Self::position_of_kelvin(self.limits.max),
        )
    }

    fn clamp_value(&self, value: f64) -> Result<f64, ValueError> {
        ensure_finite(value)?;
        if value <= 0.0 {
            // Нулевые и отрицательные Кельвины не имеют майред: это не «вне пределов», а ошибка.
            return Err(ValueError::InvalidSpec {
                name: "value",
                reason: "Кельвины должны быть > 0",
            });
        }
        Ok(value.clamp(self.limits.min, self.limits.max))
    }

    fn position_of(&self, value: f64) -> Result<f64, ValueError> {
        Ok(Self::position_of_kelvin(self.clamp_value(value)?))
    }

    fn value_at(&self, position: f64) -> f64 {
        let (lowest, highest) = self.position_range();
        if !position.is_finite() || position <= lowest {
            return self.limits.min;
        }
        if position >= highest {
            return self.limits.max;
        }
        // Внутри пределов `position` < 0 строго (highest < 0), деление безопасно.
        MIRED_NUMERATOR / -position
    }

    fn marks(&self) -> Vec<Mark> {
        let (lowest, highest) = self.position_range();
        grid_marks(lowest, highest, self.tick_spacing_mired)
    }
}

impl ExactPosition for MiredKelvin {}

impl RulerAxis for MiredKelvin {
    fn tick_step_units(&self) -> f64 {
        self.tick_spacing_mired
    }
}

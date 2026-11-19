//! «Резинка» за краем прокрутки (`motion.md` раздел 2).

use super::error::ScrollError;

/// Запас от асимптоты `dimension` при обратном расчёте: смещение палца не может достичь `d`.
const MAX_STRETCH_FRACTION: f64 = 1.0 - 1e-12;

/// Функция «резинки» `(1 − 1/(x·c/d + 1))·d` с коэффициентом `c` и размером области `d`.
///
/// `[reverse]` Grant Paul, реверс UIScrollView iOS 6: *"c = constant value, UIScrollView uses
/// 0.55"*; для iOS 26/27 не проверено — коэффициент это параметр конфига. Функция нечётна:
/// отрицательный выход за край (за минимальную границу) даёт отрицательное смещение.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RubberBand {
    coefficient: f64,
    dimension: f64,
}

impl RubberBand {
    pub fn new(coefficient: f64, dimension: f64) -> Result<Self, ScrollError> {
        if !coefficient.is_finite() || coefficient <= 0.0 {
            return Err(ScrollError::InvalidRubberCoefficient(coefficient));
        }
        if !dimension.is_finite() || dimension <= 0.0 {
            return Err(ScrollError::InvalidViewportExtent(dimension));
        }
        Ok(Self { coefficient, dimension })
    }

    pub fn coefficient(&self) -> f64 {
        self.coefficient
    }

    pub fn dimension(&self) -> f64 {
        self.dimension
    }

    /// Насколько сдвинется контент, если палец ушёл за край на `overshoot` (со знаком).
    /// Предел при бесконечном `overshoot` — `dimension`.
    pub fn stretch(&self, overshoot: f64) -> f64 {
        let magnitude = overshoot.abs();
        let stretched = (1.0 - 1.0 / (magnitude * self.coefficient / self.dimension + 1.0)) * self.dimension;
        stretched.copysign(overshoot)
    }

    /// Обратная функция: где должен быть «виртуальный» палец, чтобы контент стоял на
    /// смещении `displacement` за краем. Нужна, чтобы схватить контент во время возврата
    /// без скачка. `|displacement|` ограничивается асимптотой `dimension` (недостижимой).
    pub fn unstretch(&self, displacement: f64) -> f64 {
        let fraction = (displacement.abs() / self.dimension).min(MAX_STRETCH_FRACTION);
        let overshoot = self.dimension / self.coefficient * fraction / (1.0 - fraction);
        overshoot.copysign(displacement)
    }

    /// Производная `stretch` по `overshoot`: во сколько раз контент медленнее пальца.
    /// Нужна, чтобы перевести скорость пальца в скорость контента при отпускании за краем.
    pub fn slope(&self, overshoot: f64) -> f64 {
        let denominator = overshoot.abs() * self.coefficient / self.dimension + 1.0;
        self.coefficient / (denominator * denominator)
    }
}

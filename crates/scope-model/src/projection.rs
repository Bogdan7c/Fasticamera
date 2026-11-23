//! Целочисленная проекция кода в значение: единственная арифметика правил waveform, parade и
//! вектороскопа Y'CbCr.
//!
//! `value = (w0·c0 + w1·c1 + w2·c2 + offset) >> shift`, всё в `i32`, `>>` арифметический (у отрицательных
//! округляет вниз, как GLSL `>>` над `int`). Коэффициенты считаются ОДИН раз на CPU в f64 из формулы и
//! округляются до целых; именно эти целые отправляются на GPU. Поэтому CPU и GPU делают одно и то же
//! целочисленное вычисление и совпадают бит в бит.
//!
//! GLSL: `(w.x * c.x + w.y * c.y + w.z * c.z + offset) >> shift`.

/// Сдвиг по умолчанию: 16 двоичных разрядов дробной части коэффициентов.
pub const PROJECTION_SHIFT: u32 = 16;

/// Наибольший допустимый сдвиг: значения должны оставаться в `i32`.
const MAX_SHIFT: u32 = 30;

/// Наибольший код канала, на котором проверяется переполнение (10 бит).
const MAX_CODE: i64 = 1023;

/// Как согласовать сумму весов после округления.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WeightSum {
    /// Веса округляются независимо.
    Unconstrained,
    /// Сумма вещественных коэффициентов точно равна числу: вес зелёного канала поглощает ошибку
    /// округления. Для серого (`R = G = B`) это даёт точное значение (например, ровно 0 у цветоразности).
    Exactly(f64),
}

/// Ошибка построения проекции.
#[derive(Debug, Clone, Copy, PartialEq, thiserror::Error)]
pub enum ProjectionError {
    /// Коэффициент не конечен или не помещается в `i32` после масштабирования.
    #[error("коэффициент проекции не помещается в i32: {value}")]
    CoefficientOutOfRange {
        /// Значение коэффициента (в целых единицах).
        value: f64,
    },
    /// Сдвиг больше допустимого.
    #[error("сдвиг проекции {shift} больше {MAX_SHIFT}")]
    ShiftTooLarge {
        /// Недопустимый сдвиг.
        shift: u32,
    },
    /// Сумма на кодах 0…1023 выходит за `i32`.
    #[error("проекция переполняет i32 на кодах 0..=1023")]
    Overflow,
}

/// Целочисленная проекция кода `[u16; 3]` в значение `i32`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IntegerProjection {
    weights: [i32; 3],
    offset: i32,
    shift: u32,
}

impl IntegerProjection {
    /// Проекция из вещественных коэффициентов: `value = Σ coefficients[i]·code[i] + offset`
    /// (в единицах значения), с `shift` дробными разрядами.
    pub fn from_real(coefficients: [f64; 3], offset: f64, shift: u32, sum: WeightSum) -> Result<Self, ProjectionError> {
        if shift > MAX_SHIFT {
            return Err(ProjectionError::ShiftTooLarge { shift });
        }
        let scale = f64::from(1u32 << shift);
        let mut weights = [0i32; 3];
        for (weight, coefficient) in weights.iter_mut().zip(coefficients) {
            *weight = round_to_i32(coefficient * scale)?;
        }
        if let WeightSum::Exactly(target) = sum {
            let target_weight_sum = round_to_i32(target * scale)?;
            let others = i64::from(weights[0]) + i64::from(weights[2]);
            weights[1] = i32::try_from(i64::from(target_weight_sum) - others)
                .map_err(|_| ProjectionError::CoefficientOutOfRange { value: target * scale })?;
        }
        Self::from_integers(weights, round_to_i32(offset * scale)?, shift)
    }

    /// Проекция из готовых целых (проверка переполнения — та же).
    pub fn from_integers(weights: [i32; 3], offset: i32, shift: u32) -> Result<Self, ProjectionError> {
        if shift > MAX_SHIFT {
            return Err(ProjectionError::ShiftTooLarge { shift });
        }
        // Сумма линейна по кодам, поэтому экстремумы — в углах куба кодов; проверяем их в i64.
        let mut lowest = i64::from(offset);
        let mut highest = i64::from(offset);
        for weight in weights {
            let reach = i64::from(weight) * MAX_CODE;
            lowest += reach.min(0);
            highest += reach.max(0);
        }
        if i32::try_from(lowest).is_err() || i32::try_from(highest).is_err() {
            return Err(ProjectionError::Overflow);
        }
        Ok(Self { weights, offset, shift })
    }

    /// Значение для кода.
    pub fn apply(&self, code: [u16; 3]) -> i32 {
        let sum = self.weights[0] * i32::from(code[0])
            + self.weights[1] * i32::from(code[1])
            + self.weights[2] * i32::from(code[2])
            + self.offset;
        sum >> self.shift
    }

    /// Целые веса (для упаковки в uniform GPU).
    pub fn weights(&self) -> [i32; 3] {
        self.weights
    }

    /// Целое смещение (для GPU).
    pub fn offset(&self) -> i32 {
        self.offset
    }

    /// Сдвиг (для GPU).
    pub fn shift(&self) -> u32 {
        self.shift
    }
}

fn round_to_i32(value: f64) -> Result<i32, ProjectionError> {
    let rounded = value.round();
    if !rounded.is_finite() || rounded < f64::from(i32::MIN) || rounded > f64::from(i32::MAX) {
        return Err(ProjectionError::CoefficientOutOfRange { value });
    }
    // Диапазон проверен выше.
    Ok(rounded as i32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shift_is_arithmetic_for_negative_sums() {
        let projection = IntegerProjection::from_integers([-1, 0, 0], 0, 1).unwrap();
        // -3 >> 1 = -2 (вниз), а не -1 (к нулю).
        assert_eq!(projection.apply([3, 0, 0]), -2);
    }

    #[test]
    fn exact_sum_makes_gray_exact() {
        // Цветоразность: сумма коэффициентов 0 — любой серый даёт ровно 0 независимо от округления.
        let projection =
            IntegerProjection::from_real([-0.1146, -0.3854, 0.5], 0.0, PROJECTION_SHIFT, WeightSum::Exactly(0.0))
                .unwrap();
        assert_eq!(projection.weights().iter().sum::<i32>(), 0);
        for code in [0u16, 1, 511, 1023] {
            assert_eq!(projection.apply([code; 3]), 0);
        }
    }

    #[test]
    fn overflow_is_reported() {
        assert_eq!(
            IntegerProjection::from_integers([i32::MAX / 100, 0, 0], 0, 0),
            Err(ProjectionError::Overflow)
        );
        assert!(matches!(
            IntegerProjection::from_real([1e30, 0.0, 0.0], 0.0, 0, WeightSum::Unconstrained),
            Err(ProjectionError::CoefficientOutOfRange { .. })
        ));
        assert_eq!(
            IntegerProjection::from_integers([0; 3], 0, 31),
            Err(ProjectionError::ShiftTooLarge { shift: 31 })
        );
    }

    #[test]
    fn weights_are_rounded_once_from_the_formula() {
        let projection = IntegerProjection::from_real([0.25, 0.5, 0.25], 1.0, 4, WeightSum::Unconstrained).unwrap();
        assert_eq!(projection.weights(), [4, 8, 4]);
        assert_eq!(projection.offset(), 16);
        assert_eq!(projection.apply([16, 0, 0]), 5);
    }
}

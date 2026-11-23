//! Одна ручка вида: число в диапазоне [−1, 1], где 0 — нейтраль.

use serde::{Deserialize, Serialize};

/// Ошибка проверки значения ручки. Варианты различимы, чтобы экран мог отреагировать по-разному.
#[derive(Debug, Clone, Copy, PartialEq, thiserror::Error)]
pub enum LookKnobError {
    /// NaN или бесконечность: такое значение нельзя сравнивать и показывать на шкале.
    #[error("значение ручки не число (NaN или бесконечность)")]
    NotANumber,
    /// Конечное число, но за пределами [−1, 1].
    #[error("значение ручки {value} вне диапазона [-1, 1]")]
    OutOfRange {
        /// Отвергнутое значение.
        value: f32,
    },
}

/// Значение ручки: конечное `f32` в [−1, 1]; 0 — «ничего не менять».
///
/// Инвариант (конечное, в диапазоне) держится конструктором [`LookKnob::new`] и
/// `try_from` при чтении из файла. Поэтому NaN внутри быть не может и сравнение `==` честное.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "f32", into = "f32")]
pub struct LookKnob(f32);

impl LookKnob {
    /// Нейтраль: ручка стоит в нуле.
    pub const NEUTRAL: LookKnob = LookKnob(0.0);

    /// Проверяет и строит значение ручки.
    pub fn new(value: f32) -> Result<Self, LookKnobError> {
        if !value.is_finite() {
            return Err(LookKnobError::NotANumber);
        }
        if !(-1.0..=1.0).contains(&value) {
            return Err(LookKnobError::OutOfRange { value });
        }
        Ok(Self(value))
    }

    /// Значение в [−1, 1].
    pub fn get(self) -> f32 {
        self.0
    }

    /// Ручка стоит ровно в нуле (−0.0 тоже нейтраль).
    pub fn is_neutral(self) -> bool {
        self.0 == 0.0
    }
}

impl Default for LookKnob {
    fn default() -> Self {
        Self::NEUTRAL
    }
}

impl TryFrom<f32> for LookKnob {
    type Error = LookKnobError;

    fn try_from(value: f32) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<LookKnob> for f32 {
    fn from(knob: LookKnob) -> Self {
        knob.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn range_edges_are_accepted() {
        assert_eq!(LookKnob::new(-1.0).map(LookKnob::get), Ok(-1.0));
        assert_eq!(LookKnob::new(1.0).map(LookKnob::get), Ok(1.0));
        assert_eq!(LookKnob::new(0.0), Ok(LookKnob::NEUTRAL));
    }

    #[test]
    fn just_outside_range_is_rejected_with_value() {
        assert_eq!(LookKnob::new(1.0001), Err(LookKnobError::OutOfRange { value: 1.0001 }));
        assert_eq!(LookKnob::new(-1.5), Err(LookKnobError::OutOfRange { value: -1.5 }));
    }

    #[test]
    fn non_finite_is_not_a_number() {
        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            assert_eq!(LookKnob::new(bad), Err(LookKnobError::NotANumber));
        }
    }

    #[test]
    fn negative_zero_is_neutral() {
        assert!(LookKnob::new(-0.0).is_ok_and(LookKnob::is_neutral));
        assert!(!LookKnob::new(0.01).is_ok_and(LookKnob::is_neutral));
    }

    #[test]
    fn serde_roundtrip_keeps_value() {
        let knob = LookKnob::new(0.25).unwrap();
        let json = serde_json::to_string(&knob).unwrap();
        assert_eq!(json, "0.25");
        assert_eq!(serde_json::from_str::<LookKnob>(&json).unwrap(), knob);
    }

    #[test]
    fn serde_rejects_out_of_range_with_reason() {
        let error = serde_json::from_str::<LookKnob>("2.0").unwrap_err();
        assert!(error.to_string().contains("вне диапазона"), "{error}");
    }

    #[test]
    fn serde_rejects_non_number() {
        assert!(serde_json::from_str::<LookKnob>("\"0.5\"").is_err());
        assert!(serde_json::from_str::<LookKnob>("null").is_err());
        // JSON не умеет NaN, но 1e39 переполняет f32 в бесконечность — тоже отказ.
        assert!(serde_json::from_str::<LookKnob>("1e39").is_err());
    }
}

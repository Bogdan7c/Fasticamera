//! Плотность шкалы, заданная явным числом: сколько pt пальца приходится на одну единицу модели.

use std::hash::{Hash, Hasher};

use serde::{Deserialize, Serialize};

use super::error::TokenError;

/// pt пальца на единицу модели, заданные явно (вариант [`super::ScaleDensity::Explicit`]).
///
/// Инвариант: число конечное и строго больше нуля. Он проверяется один раз при создании, поэтому дальше шкала
/// делит и умножает на него без оглядки на `NaN`/ноль. Из-за этого же инварианта тип можно честно сравнивать
/// (`Eq`) и хешировать: значения `NaN`, ломающего эти свойства у `f64`, здесь не бывает.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "f64", into = "f64")]
pub struct PointsPerUnit(f64);

impl PointsPerUnit {
    /// Плотность из числа pt на единицу; `NaN`, бесконечность, ноль и отрицательное — ошибка.
    pub fn new(points_per_unit: f64) -> Result<Self, TokenError> {
        if points_per_unit.is_finite() && points_per_unit > 0.0 {
            Ok(Self(points_per_unit))
        } else {
            Err(TokenError::NonPositiveSize {
                name: "explicit_points_per_unit",
                value: points_per_unit,
            })
        }
    }

    /// pt на единицу модели (конечное, > 0).
    pub fn get(self) -> f64 {
        self.0
    }
}

impl TryFrom<f64> for PointsPerUnit {
    type Error = TokenError;

    fn try_from(points_per_unit: f64) -> Result<Self, Self::Error> {
        Self::new(points_per_unit)
    }
}

impl From<PointsPerUnit> for f64 {
    fn from(points_per_unit: PointsPerUnit) -> Self {
        points_per_unit.0
    }
}

// Безопасно: `NaN` исключён инвариантом конструктора.
impl Eq for PointsPerUnit {}

impl Hash for PointsPerUnit {
    fn hash<H: Hasher>(&self, state: &mut H) {
        // `+0.0` и `-0.0` невозможны (значение > 0), поэтому равные числа дают равные биты.
        self.0.to_bits().hash(state);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_only_finite_positive_numbers() {
        assert_eq!(PointsPerUnit::new(1.0).expect("1 pt допустим").get(), 1.0);
        for bad in [0.0, -1.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert!(PointsPerUnit::new(bad).is_err(), "{bad} должно отвергаться");
        }
    }

    #[test]
    fn deserialization_enforces_the_same_invariant() {
        let ok: PointsPerUnit = serde_json::from_str("2.5").expect("2.5 допустимо");
        assert_eq!(ok.get(), 2.5);
        assert!(serde_json::from_str::<PointsPerUnit>("0").is_err());
        assert!(serde_json::from_str::<PointsPerUnit>("-3").is_err());
    }
}

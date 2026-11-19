//! Критерий покоя пружины (`motion.md` 1.2).

use serde::{Deserialize, Serialize};

use super::model::SpringError;

/// Когда пружину считать успокоившейся. Учитываются И положение, И скорость.
///
/// Допуск положения `X = max(relative_epsilon · расстояние, absolute_position_floor)`.
/// Абсолютный пол нужен, когда расстояние 0: перенацеливание в ту же точку при ненулевой
/// скорости не должно быть «покоем» сразу, а относительный допуск был бы нулевым.
///
/// Допуск скорости привязан к допуску положения через частоту пружины: `V = X · ω0 · velocity_tolerance_factor`.
/// Так «эллипс покоя» в фазовой плоскости согласован с самой пружиной, а скорость не вводит
/// второй, независимый масштаб (абсолютный порог в pt/с на больших расстояниях задерживал бы
/// покой в разы дольше, чем положение).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct RestCriterion {
    /// Доля от начального расстояния. `[doc]` D `swiftui/spring/settlingduration`: *"an epsilon of 0.001"*.
    pub relative_epsilon: f64,
    /// Минимальный допуск положения, pt. `[estimate]` — подгонка, не Apple (~1/100 px при 3x).
    pub absolute_position_floor: f64,
    /// Множитель допуска скорости (см. выше). `[estimate]` — подгонка, не Apple: Apple скорость
    /// в `settlingDuration` не описывает, 1,0 означает «скорость не строже положения».
    pub velocity_tolerance_factor: f64,
}

impl Default for RestCriterion {
    fn default() -> Self {
        Self {
            relative_epsilon: 0.001,
            absolute_position_floor: 0.01,
            velocity_tolerance_factor: 1.0,
        }
    }
}

/// Допуски, вычисленные для конкретного расстояния.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RestTolerance {
    pub position: f64,
    pub velocity: f64,
}

impl RestTolerance {
    /// Состояние внутри допусков по обеим осям?
    pub fn contains(&self, displacement: f64, velocity: f64) -> bool {
        displacement.abs() <= self.position && velocity.abs() <= self.velocity
    }
}

impl RestCriterion {
    /// Проверка параметров: пол и порог скорости строго положительны (иначе покоя можно не дождаться).
    pub fn validate(&self) -> Result<(), SpringError> {
        let check = |field: &'static str, requirement: &'static str, value: f64, ok: bool| {
            if value.is_finite() && ok {
                Ok(())
            } else {
                Err(SpringError::InvalidRestCriterion {
                    field,
                    requirement,
                    value,
                })
            }
        };
        check(
            "relative_epsilon",
            ">= 0",
            self.relative_epsilon,
            self.relative_epsilon >= 0.0,
        )?;
        check(
            "absolute_position_floor",
            "> 0",
            self.absolute_position_floor,
            self.absolute_position_floor > 0.0,
        )?;
        check(
            "velocity_tolerance_factor",
            "> 0",
            self.velocity_tolerance_factor,
            self.velocity_tolerance_factor > 0.0,
        )
    }

    /// Допуски для анимации на расстояние `distance` (знак не важен) пружиной с собственной
    /// угловой частотой `angular_frequency` (рад/с, `√(k/m)`).
    pub fn tolerance_for_distance(&self, distance: f64, angular_frequency: f64) -> RestTolerance {
        let position = (self.relative_epsilon * distance.abs()).max(self.absolute_position_floor);
        RestTolerance {
            position,
            velocity: position * angular_frequency * self.velocity_tolerance_factor,
        }
    }
}

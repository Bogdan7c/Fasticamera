//! Сериализуемое описание пружины для конфигов.

use serde::{Deserialize, Serialize};

use super::model::{Spring, SpringError};

/// Описание пружины в конфиге; превращается в [`Spring`] через [`SpringSpec::build`].
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case", tag = "kind")]
pub enum SpringSpec {
    /// `Spring(duration:bounce:)`.
    DurationBounce { duration_s: f64, bounce: f64 },
    /// `Spring(response:dampingRatio:)`.
    ResponseDampingFraction { response_s: f64, damping_fraction: f64 },
    /// Масса, жёсткость, затухание.
    Physical { mass: f64, stiffness: f64, damping: f64 },
    /// «Бесконечно жёсткая» пружина.
    Immediate {},
}

impl SpringSpec {
    /// Проверить и построить пружину.
    pub fn build(&self) -> Result<Spring, SpringError> {
        match *self {
            Self::DurationBounce { duration_s, bounce } => Spring::from_duration_bounce(duration_s, bounce),
            Self::ResponseDampingFraction {
                response_s,
                damping_fraction,
            } => Spring::from_response_damping_fraction(response_s, damping_fraction),
            Self::Physical {
                mass,
                stiffness,
                damping,
            } => Spring::from_physical(mass, stiffness, damping),
            Self::Immediate {} => Ok(Spring::immediate()),
        }
    }
}

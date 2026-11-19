//! Масштаб «пикселей на pt» с проверкой.

use super::error::ViewError;

/// Сколько пикселей экрана в одном pt (1 pt = ширина / 440 px, решение 82). Конечное и > 0.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PxPerPt(f32);

impl PxPerPt {
    pub fn new(value: f32) -> Result<Self, ViewError> {
        if value.is_finite() && value > 0.0 {
            Ok(Self(value))
        } else {
            Err(ViewError::InvalidScale(value))
        }
    }

    pub fn get(self) -> f32 {
        self.0
    }

    /// pt → px.
    pub fn px(self, points: f64) -> f32 {
        points as f32 * self.0
    }
}

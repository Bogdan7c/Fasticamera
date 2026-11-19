//! Двумерный оценщик скорости одного пальца: две независимые оси на общей шкале времени.

use super::config::{VelocityConfig, VelocityConfigError};
use super::scalar::{SampleOutcome, ScalarVelocityTracker, VelocityEstimate};
use crate::geometry::{Point, Vector};
use crate::time::MonotonicNanos;

/// Оценщик скорости пальца в pt/с.
#[derive(Debug, Clone)]
pub struct VelocityTracker {
    horizontal: ScalarVelocityTracker,
    vertical: ScalarVelocityTracker,
}

impl VelocityTracker {
    pub fn new(config: VelocityConfig) -> Result<Self, VelocityConfigError> {
        Ok(Self {
            horizontal: ScalarVelocityTracker::new(config.clone())?,
            vertical: ScalarVelocityTracker::new(config)?,
        })
    }

    /// Стереть историю (новое касание).
    pub fn clear(&mut self) {
        self.horizontal.clear();
        self.vertical.clear();
    }

    pub fn sample_count(&self) -> usize {
        self.horizontal.sample_count()
    }

    /// Добавить положение пальца. Обе оси принимают или отбрасывают выборку вместе
    /// (одна метка времени, а координаты проверяются до записи).
    pub fn add_sample(&mut self, time: MonotonicNanos, position: Point) -> SampleOutcome {
        if !position.x.is_finite() || !position.y.is_finite() {
            return SampleOutcome::RejectedNonFinite;
        }
        let outcome = self.horizontal.add_sample(time, position.x);
        self.vertical.add_sample(time, position.y);
        outcome
    }

    /// Скорость пальца в момент `now`, pt/с. Оси делят метки времени, поэтому «нет оценки» и «стоп»
    /// у них совпадают.
    pub fn estimate_at(&self, now: MonotonicNanos) -> VelocityEstimate<Vector> {
        match (self.horizontal.estimate_at(now), self.vertical.estimate_at(now)) {
            (VelocityEstimate::Measured(dx), VelocityEstimate::Measured(dy)) => {
                VelocityEstimate::Measured(Vector::new(dx, dy))
            }
            (VelocityEstimate::Stopped, _) | (_, VelocityEstimate::Stopped) => VelocityEstimate::Stopped,
            _ => VelocityEstimate::NotEnoughSamples,
        }
    }

    /// Скорость вектором; «нет оценки» и «стоп» дают [`Vector::ZERO`], NaN/inf не бывает.
    pub fn velocity_at(&self, now: MonotonicNanos) -> Vector {
        self.estimate_at(now).or_zero()
    }
}

impl VelocityEstimate<Vector> {
    /// Вектор для потребителя, которому важно только значение: «нет оценки» и «стоп» дают нуль,
    /// каждая компонента проходит то же правило конечности, что у скаляра ([`VelocityEstimate::or_zero`]).
    pub fn or_zero(self) -> Vector {
        match self {
            Self::Measured(velocity) => Vector::new(
                VelocityEstimate::Measured(velocity.dx).or_zero(),
                VelocityEstimate::Measured(velocity.dy).or_zero(),
            ),
            Self::Stopped | Self::NotEnoughSamples => Vector::ZERO,
        }
    }
}

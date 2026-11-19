//! Оценка скорости пальца по истории касаний (`docs/ios-camera/behavior.md` раздел 1).
//!
//! Apple алгоритм не раскрывает; берём наименьшие квадраты по последним ~100 мс как в
//! `VelocityTracker` Android (`[estimate]` по спецификации, числа — `[reverse]` из AOSP). Все числа —
//! поля [`VelocityConfig`].
//!
//! * [`ScalarVelocityTracker`] — одна величина (координата, расстояние между пальцами);
//! * [`VelocityTracker`] — палец на плоскости (две оси).
//!
//! Вырожденные случаи (одна точка, одна метка времени, вырожденная матрица) дают явный вариант
//! [`VelocityEstimate::NotEnoughSamples`], а удобные методы `velocity_at`/`or_zero` — нулевую скорость.
//! NaN и бесконечностей на выходе не бывает.

mod config;
mod scalar;
mod tracker;

#[cfg(test)]
mod tests;

pub use config::{MAX_POLYNOMIAL_DEGREE, VelocityConfig, VelocityConfigError};
pub use scalar::{SampleOutcome, ScalarVelocityTracker, VelocityEstimate};
pub use tracker::VelocityTracker;

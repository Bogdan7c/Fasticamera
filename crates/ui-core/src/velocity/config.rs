//! Параметры оценки скорости пальца.
//!
//! Все числа — подгонка под поведение `VelocityTracker` Android, а не Apple: алгоритм Apple не
//! раскрыт (`docs/ios-camera/behavior.md` раздел 1, строка «Скорость пальца», метка `[estimate]`).
//! Значения по умолчанию сверены с исходником AOSP
//! `frameworks/native/libs/input/VelocityTracker.cpp` и `include/input/VelocityTracker.h`
//! (ветка `main`, читались 19.11.2026 через `android.googlesource.com`, метка `[reverse]`).

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::time::UiDuration;

/// Наибольшая поддерживаемая степень полинома (в AOSP `VelocityTracker::MAX_DEGREE` = 4,
/// нам для пальца хватает кубической подгонки).
pub const MAX_POLYNOMIAL_DEGREE: u8 = 3;

/// Настройки оценщика скорости.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct VelocityConfig {
    /// Степень полинома наименьших квадратов. `[reverse]` 2: в AOSP стратегия по умолчанию для
    /// осей X и Y — `LSQ2` (`DEFAULT_STRATEGY_BY_AXIS`). Подгонка, не Apple.
    pub polynomial_degree: u8,
    /// Горизонт: точки старше последней больше чем на это время не участвуют, мс. `[reverse]` 100:
    /// `LeastSquaresVelocityTrackerStrategy::HORIZON = 100 ms`; спецификация (`behavior.md` 1)
    /// тоже говорит «последние ~100 мс». Подгонка, не Apple.
    pub horizon_ms: u32,
    /// Сколько последних точек помнить. `[reverse]` 20: `AccumulatingVelocityTrackerStrategy::HISTORY_SIZE`.
    pub history_size: usize,
    /// Если между двумя точками (или между последней точкой и моментом запроса) прошло больше,
    /// палец считается остановившимся, мс. `[reverse]` 40: `ASSUME_POINTER_STOPPED_TIME`. Подгонка, не Apple.
    pub assume_stopped_after_ms: u32,
}

impl Default for VelocityConfig {
    fn default() -> Self {
        Self {
            polynomial_degree: 2,
            horizon_ms: 100,
            history_size: 20,
            assume_stopped_after_ms: 40,
        }
    }
}

/// Ошибка проверки [`VelocityConfig`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum VelocityConfigError {
    #[error("степень полинома должна быть от 1 до {MAX_POLYNOMIAL_DEGREE}, получено {0}")]
    InvalidDegree(u8),
    #[error("горизонт должен быть > 0 мс")]
    ZeroHorizon,
    #[error("в истории нужно хотя бы 2 точки, получено {0}")]
    HistoryTooSmall(usize),
    #[error("порог «палец остановился» должен быть > 0 мс")]
    ZeroStoppedThreshold,
}

impl VelocityConfig {
    /// Проверка значений; вызывается при создании трекера.
    pub fn validate(&self) -> Result<(), VelocityConfigError> {
        if self.polynomial_degree == 0 || self.polynomial_degree > MAX_POLYNOMIAL_DEGREE {
            return Err(VelocityConfigError::InvalidDegree(self.polynomial_degree));
        }
        if self.horizon_ms == 0 {
            return Err(VelocityConfigError::ZeroHorizon);
        }
        if self.history_size < 2 {
            return Err(VelocityConfigError::HistoryTooSmall(self.history_size));
        }
        if self.assume_stopped_after_ms == 0 {
            return Err(VelocityConfigError::ZeroStoppedThreshold);
        }
        Ok(())
    }

    pub(crate) fn horizon(&self) -> UiDuration {
        UiDuration::from_millis(i64::from(self.horizon_ms))
    }

    pub(crate) fn assume_stopped_after(&self) -> UiDuration {
        UiDuration::from_millis(i64::from(self.assume_stopped_after_ms))
    }
}

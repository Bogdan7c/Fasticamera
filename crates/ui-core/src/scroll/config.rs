//! Конфиг движка прокрутки.

use serde::{Deserialize, Serialize};

use super::deceleration::{DecelerationModel, DecelerationRate};
use super::error::ScrollError;
use super::snap::SnapSelection;
use crate::spring::{RestCriterion, Spring, SpringSpec};
use crate::time::MotionPreference;

/// Параметры движка одной оси. Все значения по умолчанию, кроме двух `[review]`-замедлений,
/// — `[estimate]`: «подгонка, не Apple». Измеренная пружина карусели режимов (duration ≈ 0,85,
/// bounce ≈ −0,13, `motion.md` 4.1) — токен `ui-kit`, а не умолчание движка.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ScrollConfig {
    /// Доля скорости за 1 мс. `[review]` `.normal` = 0,998 (`motion.md` раздел 2).
    pub deceleration_rate: DecelerationRate,
    /// Модуль скорости (pt/с), ниже которого бросок останавливается. `[estimate]` — подгонка, не Apple.
    pub stop_velocity: f64,
    /// Коэффициент «резинки». `[reverse]` 0,55 (Grant Paul, UIScrollView iOS 6; для iOS 26/27 не проверено).
    pub rubber_band_coefficient: f64,
    /// Пружина прилипания к точке и программной прокрутки. `[estimate]` — `.smooth`-подобная, подгонка, не Apple.
    pub snap_spring: SpringSpec,
    /// Пружина возврата из-за края. `[estimate]` — подгонка, не Apple.
    pub edge_return_spring: SpringSpec,
    /// Политика выбора точки прилипания. `[estimate]`: по умолчанию «ближайшая к проекции».
    pub snap_selection: SnapSelection,
    /// Критерий покоя пружин движка.
    pub rest: RestCriterion,
}

impl Default for ScrollConfig {
    fn default() -> Self {
        Self {
            deceleration_rate: DecelerationRate::normal(),
            stop_velocity: 0.5,
            rubber_band_coefficient: 0.55,
            snap_spring: SpringSpec::DurationBounce {
                duration_s: 0.5,
                bounce: 0.0,
            },
            edge_return_spring: SpringSpec::DurationBounce {
                duration_s: 0.4,
                bounce: 0.0,
            },
            snap_selection: SnapSelection::NearestToProjection,
            rest: RestCriterion::default(),
        }
    }
}

/// Конфиг после проверки: готовые пружины и модель замедления.
#[derive(Debug, Clone, Copy)]
pub(super) struct ResolvedScrollConfig {
    pub deceleration: DecelerationModel,
    pub rubber_band_coefficient: f64,
    pub snap_spring: Spring,
    pub edge_return_spring: Spring,
    pub snap_selection: SnapSelection,
    pub rest: RestCriterion,
}

impl ResolvedScrollConfig {
    /// Конфиг с учётом Reduce Motion: обе пружины проходят через единое правило
    /// [`Spring::for_motion`]; остальное не меняется.
    pub(super) fn for_motion(self, motion: MotionPreference) -> Self {
        Self {
            snap_spring: self.snap_spring.for_motion(motion),
            edge_return_spring: self.edge_return_spring.for_motion(motion),
            ..self
        }
    }
}

impl ScrollConfig {
    /// Проверить все поля.
    pub fn validate(&self) -> Result<(), ScrollError> {
        self.resolve().map(|_| ())
    }

    pub(super) fn resolve(&self) -> Result<ResolvedScrollConfig, ScrollError> {
        if !self.rubber_band_coefficient.is_finite() || self.rubber_band_coefficient <= 0.0 {
            return Err(ScrollError::InvalidRubberCoefficient(self.rubber_band_coefficient));
        }
        self.rest.validate()?;
        Ok(ResolvedScrollConfig {
            deceleration: DecelerationModel::new(self.deceleration_rate, self.stop_velocity)?,
            rubber_band_coefficient: self.rubber_band_coefficient,
            snap_spring: self.snap_spring.build()?,
            edge_return_spring: self.edge_return_spring.build()?,
            snap_selection: self.snap_selection,
            rest: self.rest,
        })
    }
}

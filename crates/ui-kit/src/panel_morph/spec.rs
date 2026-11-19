//! Описание панели-морфа: откуда растёт, куда, какой пружиной.

use ui_core::geometry::Rect;

use super::error::PanelError;
use crate::tokens::MotionToken;

/// Радиус вложенной формы по правилу концентричности (`glass.md` раздел 2, Apple 356, 3:49):
/// радиус родителя минус отступ; не меньше нуля.
///
/// Пример: панель с радиусом 40 pt и отступом пункта 8 pt → радиус пункта 32 pt.
pub fn concentric_radius_pt(parent_radius_pt: f64, inset_pt: f64) -> f64 {
    (parent_radius_pt - inset_pt).max(0.0)
}

/// Описание панели. Рамки — в pt, в одних координатах со сценой.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PanelMorphSpec {
    /// Рамка источника (кнопка, из которой растёт панель; у подменю — рамка пункта).
    /// Радиус источника — капсула/круг, то есть половина меньшей стороны.
    pub source: Rect,
    /// Рамка открытой панели.
    pub target: Rect,
    /// Радиус скругления открытой панели, pt. Для вложенной панели берётся из
    /// [`concentric_radius_pt`], для самостоятельной — из раскладки.
    pub target_corner_radius_pt: f64,
    /// Имя пружины движения. Умолчание — `camera.sheet` (меню и панель); для морфа пункта в
    /// подменю экран передаёт [`MotionToken::Smooth`].
    pub spring: MotionToken,
}

impl PanelMorphSpec {
    /// Панель с пружиной `camera.sheet`.
    pub fn new(source: Rect, target: Rect, target_corner_radius_pt: f64) -> Self {
        Self {
            source,
            target,
            target_corner_radius_pt,
            spring: MotionToken::CameraSheet,
        }
    }

    /// Та же панель с другой пружиной.
    pub fn with_spring(self, spring: MotionToken) -> Self {
        Self { spring, ..self }
    }

    pub(super) fn validate(&self) -> Result<(), PanelError> {
        for (which, frame) in [("source", self.source), ("target", self.target)] {
            let finite = [frame.origin.x, frame.origin.y, frame.size.width, frame.size.height]
                .iter()
                .all(|value| value.is_finite());
            if !finite || frame.size.width <= 0.0 || frame.size.height <= 0.0 {
                return Err(PanelError::InvalidFrame { which, frame });
            }
        }
        if !self.target_corner_radius_pt.is_finite() || self.target_corner_radius_pt < 0.0 {
            return Err(PanelError::InvalidRadius(self.target_corner_radius_pt));
        }
        Ok(())
    }
}

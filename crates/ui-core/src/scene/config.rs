//! Настройки и геометрия surface сцены.

use serde::{Deserialize, Serialize};

use crate::animator::AnimatorConfig;
use crate::geometry::{Insets, Size};
use crate::gesture::GestureConfig;
use crate::input::PointScaleRule;
use crate::time::MotionPreference;

/// Настройки сцены из конфига экрана: правило масштаба, жесты, анимации, Reduce Motion.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct SceneSettings {
    /// Правило «пиксели → pt» (`[review]` 440 pt по ширине, решение 82).
    pub point_scale: PointScaleRule,
    pub gestures: GestureConfig,
    pub animator: AnimatorConfig,
    /// Reduce Motion. В v1 источник — конфиг экрана (`motion.md` раздел 3).
    pub motion: MotionPreference,
}

/// Размер surface в пикселях и безопасная зона в pt: то, что сообщает оболочка при создании и
/// повороте окна.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SurfaceGeometry {
    pub width_px: u32,
    pub height_px: u32,
    /// Безопасная зона экрана в pt.
    pub safe_area: Insets,
}

impl SurfaceGeometry {
    /// Размер экрана в pt при пикселях на pt `pixels_per_point`.
    pub(super) fn screen_size_pt(&self, pixels_per_point: f64) -> Size {
        Size::new(
            f64::from(self.width_px) / pixels_per_point,
            f64::from(self.height_px) / pixels_per_point,
        )
    }
}

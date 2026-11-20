//! Размеры вида дуговой шкалы K3 (`view::arc_scale_items`): тёмный диск, кольцо рисок, подписи, маркер.
//!
//! Числа сняты с колеса зума iPhone (`docs/ios-camera/layout.md`, `[measured: R00 t=88.5–100]`), но
//! токен о зуме не знает: так выглядит любая шкала-«циферблат» (выдержка, ISO в ручном режиме, 15).

use serde::{Deserialize, Serialize};

use super::elements::positive;
use super::error::TokenError;

/// Вид дуговой шкалы. Радиусы — от центра диска, pt; углы — градусы.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ArcScaleTokens {
    /// Радиус тёмного диска-подложки. `[measured: R00 t=88.5–100]` 243.
    pub disc_radius_pt: f64,
    /// Прозрачность диска. `[measured: R00 t=88.5–100]` ≈ 0,45.
    pub disc_alpha: f32,
    /// Внутренний радиус рисок. `[measured: R00 t=88.5–100]` 222,5.
    pub tick_inner_radius_pt: f64,
    /// Внешний радиус рисок. `[measured: R00 t=88.5–100]` 238,5.
    pub tick_outer_radius_pt: f64,
    /// Толщина риски. `[estimate]` 1,0 — подгонка, не Apple (в замере не разобрана).
    pub tick_thickness_pt: f64,
    /// Прозрачность тусклой риски. `[measured: R00 t=88.5–100]` 0,2.
    pub dim_tick_alpha: f32,
    /// Градусов дуги на единицу модели (октаву зума). `[measured: R00 t=88.5–100]` 14,2.
    pub degrees_per_unit: f64,
    /// Угол от маркера, с которого риски начинают гаснуть. `[measured: R00 t=88.5–100]` 43.
    pub fade_start_degrees: f64,
    /// Угол, на котором риски погасли полностью. `[measured: R00 t=88.5–100]` 56.
    pub fade_end_degrees: f64,
    /// Ширина жёлтого маркера текущего значения. `[measured: R00 t=88.5–100]` 3,3.
    pub marker_width_pt: f64,
    /// Высота маркера. `[measured: R00 t=88.5–100]` 8,7.
    pub marker_height_pt: f64,
    /// Радиус центра маркера. `[estimate]` 230,5 — середина кольца рисок; положение маркера в замере не
    /// снято, подгонка, не Apple.
    pub marker_center_radius_pt: f64,
}

impl Default for ArcScaleTokens {
    fn default() -> Self {
        Self {
            disc_radius_pt: 243.0,
            disc_alpha: 0.45,
            tick_inner_radius_pt: 222.5,
            tick_outer_radius_pt: 238.5,
            tick_thickness_pt: 1.0,
            dim_tick_alpha: 0.2,
            degrees_per_unit: 14.2,
            fade_start_degrees: 43.0,
            fade_end_degrees: 56.0,
            marker_width_pt: 3.3,
            marker_height_pt: 8.7,
            marker_center_radius_pt: 230.5,
        }
    }
}

impl ArcScaleTokens {
    /// Числа конечны и согласованы: кольцо внутри диска, угол затухания растёт.
    pub fn validate(&self) -> Result<(), TokenError> {
        for (name, value) in [
            ("arc_scale.disc_radius_pt", self.disc_radius_pt),
            ("arc_scale.tick_inner_radius_pt", self.tick_inner_radius_pt),
            ("arc_scale.tick_outer_radius_pt", self.tick_outer_radius_pt),
            ("arc_scale.tick_thickness_pt", self.tick_thickness_pt),
            ("arc_scale.degrees_per_unit", self.degrees_per_unit),
            ("arc_scale.fade_start_degrees", self.fade_start_degrees),
            ("arc_scale.fade_end_degrees", self.fade_end_degrees),
            ("arc_scale.marker_width_pt", self.marker_width_pt),
            ("arc_scale.marker_height_pt", self.marker_height_pt),
            ("arc_scale.marker_center_radius_pt", self.marker_center_radius_pt),
        ] {
            positive(name, value)?;
        }
        for (name, value) in [
            ("arc_scale.disc_alpha", f64::from(self.disc_alpha)),
            ("arc_scale.dim_tick_alpha", f64::from(self.dim_tick_alpha)),
        ] {
            if !(0.0..=1.0).contains(&value) {
                return Err(TokenError::OutOfRange { name, value });
            }
        }
        if self.tick_inner_radius_pt >= self.tick_outer_radius_pt {
            return Err(TokenError::OutOfRange {
                name: "arc_scale.tick_inner_radius_pt (должен быть меньше внешнего)",
                value: self.tick_inner_radius_pt,
            });
        }
        if self.tick_outer_radius_pt > self.disc_radius_pt {
            return Err(TokenError::OutOfRange {
                name: "arc_scale.tick_outer_radius_pt (должен лежать внутри диска)",
                value: self.tick_outer_radius_pt,
            });
        }
        if self.fade_start_degrees >= self.fade_end_degrees {
            return Err(TokenError::OutOfRange {
                name: "arc_scale.fade_start_degrees (должен быть меньше конца затухания)",
                value: self.fade_start_degrees,
            });
        }
        Ok(())
    }
}

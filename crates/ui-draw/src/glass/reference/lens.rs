//! Линза: профиль смещения от края и преломление по каналам цвета.

use crate::geometry::PxVector;
use crate::glass::{EdgeProfile, GlassParamsPx};

/// Нормированное смещение выборки фона на глубине `depth_from_edge_px` от края формы внутрь.
///
/// Пусть `u = 1 − depth/zone`, `u ∈ [0, 1]` (1 — край, 0 — конец зоны), тогда
/// `offset = 1 − (1 − uⁿ)^(1/n)`, где `n = 2` для [`EdgeProfile::QuarterCircle`] (Kyant) и `n = 4`
/// для [`EdgeProfile::ConvexSuperellipse`] (kube.io). Результат: 1 у самого края, монотонно падает
/// до 0 на глубине `zone`; глубже зоны — 0. Закона Снелла нет — это эвристика, как у Kyant.
/// Глубина < 0 (снаружи формы) считается краем; `zone_px ≤ 0` — линзы нет.
pub fn lens_profile(depth_from_edge_px: f32, zone_px: f32, profile: EdgeProfile) -> f32 {
    if zone_px <= 0.0 || depth_from_edge_px >= zone_px {
        return 0.0;
    }
    let u = 1.0 - depth_from_edge_px.max(0.0) / zone_px;
    let exponent = match profile {
        EdgeProfile::QuarterCircle => 2.0,
        EdgeProfile::ConvexSuperellipse => 4.0,
    };
    1.0 - (1.0 - u.powf(exponent)).powf(1.0 / exponent)
}

/// Смещения выборки фона для трёх каналов цвета, px.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ChannelOffsets {
    pub r: PxVector,
    pub g: PxVector,
    pub b: PxVector,
}

impl ChannelOffsets {
    /// Все смещения, умноженные на `factor` (интенсивность материализации).
    pub fn scaled(self, factor: f32) -> Self {
        Self {
            r: self.r * factor,
            g: self.g * factor,
            b: self.b * factor,
        }
    }
}

/// На сколько пикселей и куда сместить выборку фона для пикселя на расстоянии `sd_px` от края.
///
/// Направление — НАРУЖУ по нормали: стекло «сэмплирует область больше себя» (Apple 323, 20:00), и
/// край показывает фон чуть снаружи формы. Модуль: `strength · (1 + boost·press) · lens_profile`,
/// где `strength` — `refraction_strength_px`, а `boost` — `press.refraction_boost`. Дисперсия
/// разводит каналы: R смещается в `(1 − δ)` раз, G в 1, B в `(1 + δ)`. Глубже зоны линзы смещение 0.
pub fn refraction_offsets(
    sd_px: f32,
    normal_out: PxVector,
    params: &GlassParamsPx,
    press_amount: f32,
) -> ChannelOffsets {
    let depth = -sd_px;
    let profile = lens_profile(depth, params.lens_zone_px, params.edge_profile);
    let press_gain = 1.0 + params.press.refraction_boost * press_amount.clamp(0.0, 1.0);
    let green = normal_out * (params.refraction_strength_px * press_gain * profile);
    ChannelOffsets {
        r: green * (1.0 - params.dispersion),
        g: green,
        b: green * (1.0 + params.dispersion),
    }
}

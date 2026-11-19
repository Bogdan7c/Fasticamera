//! Параметры стекла в пикселях для конкретной формы.

use crate::geometry::PxSize;
use crate::glass::style::{EdgeProfile, GlassStyle};

/// Параметры блика-ободка, px.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpecularParamsPx {
    pub alpha: f32,
    pub rim_width_px: f32,
    pub exponent: f32,
    pub rest_angle_deg: f32,
}

/// Тёмный край, px.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DarkEdgeParamsPx {
    pub strength: f32,
    pub width_px: f32,
}

/// Адаптивное затемнение (безразмерное, дублирует стиль, чтобы пакет параметров был самодостаточен).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DimmingParams {
    pub max_alpha: f32,
    pub luminance_low: f32,
    pub luminance_high: f32,
}

/// Тень, px.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShadowParamsPx {
    pub radius_px: f32,
    pub alpha: f32,
    /// Смещение вниз, px.
    pub offset_px: f32,
}

/// Нажатие, px.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PressParamsPx {
    pub scale_gain: f32,
    pub refraction_boost: f32,
    pub glow_radius_px: f32,
    pub glow_alpha: f32,
}

impl PressParamsPx {
    /// Масштаб формы при нажатии `amount` (0..1). Применяет его строитель списка рисования
    /// (в преобразование члена группы); рендер масштаб не добавляет.
    pub fn scale_for(&self, amount: f32) -> f32 {
        1.0 + self.scale_gain * amount.clamp(0.0, 1.0)
    }
}

/// Все параметры стекла одной формы (группы), в пикселях.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GlassParamsPx {
    pub edge_profile: EdgeProfile,
    /// Ширина зоны линзы; не больше половины меньшей стороны формы.
    pub lens_zone_px: f32,
    pub refraction_strength_px: f32,
    pub blur_radius_px: f32,
    pub saturation: f32,
    pub dispersion: f32,
    pub specular: SpecularParamsPx,
    pub dark_edge: DarkEdgeParamsPx,
    pub dimming: DimmingParams,
    pub tint_level: f32,
    pub tint_alpha_at_full: f32,
    pub shadow: ShadowParamsPx,
    pub press: PressParamsPx,
}

impl GlassStyle {
    /// Пересчитать стиль в пиксели для формы с размером `member_size`.
    ///
    /// Ожидает стиль, прошедший [`GlassStyle::validate`], и `px_per_pt > 0`; иначе в результате
    /// будут нечисловые значения, которые `DrawList::validate` поймает. «Доли высоты» считаются от
    /// меньшей стороны формы: большая форма получает более толстую линзу и сильнее преломление
    /// (Apple 219, 7:02), абсолютные `Extent::Points` от размера не зависят. Зона линзы и ободки
    /// ограничены половиной меньшей стороны — шире зоны у формы просто нет.
    pub fn resolve(&self, member_size: PxSize, px_per_pt: f32) -> GlassParamsPx {
        let side = member_size.min_side();
        let max_zone = side * 0.5;
        GlassParamsPx {
            edge_profile: self.edge_profile,
            lens_zone_px: self.lens_zone.resolve_px(side, px_per_pt).min(max_zone),
            refraction_strength_px: self.refraction_strength.resolve_px(side, px_per_pt),
            blur_radius_px: self.blur_radius_pt * px_per_pt,
            saturation: self.saturation,
            dispersion: self.dispersion,
            specular: SpecularParamsPx {
                alpha: self.specular.alpha,
                rim_width_px: self.specular.rim_width.resolve_px(side, px_per_pt).min(max_zone),
                exponent: self.specular.exponent,
                rest_angle_deg: self.specular.rest_angle_deg,
            },
            dark_edge: DarkEdgeParamsPx {
                strength: self.dark_edge.strength,
                width_px: self.dark_edge.width.resolve_px(side, px_per_pt).min(max_zone),
            },
            dimming: DimmingParams {
                max_alpha: self.dimming.max_alpha,
                luminance_low: self.dimming.luminance_low,
                luminance_high: self.dimming.luminance_high,
            },
            tint_level: self.tint_level,
            tint_alpha_at_full: self.tint_alpha_at_full,
            shadow: ShadowParamsPx {
                radius_px: self.shadow.radius_pt * px_per_pt,
                alpha: self.shadow.alpha,
                offset_px: self.shadow.radius_pt * px_per_pt * self.shadow.offset_ratio,
            },
            press: PressParamsPx {
                scale_gain: self.press.scale_gain,
                refraction_boost: self.press.refraction_boost,
                glow_radius_px: self.press.glow_radius_ratio * side,
                glow_alpha: self.press.glow_alpha,
            },
        }
    }
}

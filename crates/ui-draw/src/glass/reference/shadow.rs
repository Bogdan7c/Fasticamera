//! Тень под стеклом: слой, который `shade_glass_pixel` не рисует (он возвращает нуль вне формы).

use crate::color::DisplayColor;
use crate::color::{Opacity, PremultipliedColor};
use crate::geometry::PxPoint;
use crate::glass::ShadowParamsPx;
use crate::glass::reference::{GlassPixelInput, shade_glass_pixel};
use crate::shape::{over, shadow_falloff};

/// Тень группы стекла в пикселе, premultiplied (чёрная).
///
/// `shifted_group_sd_px` — расстояние от пикселя до СЛИТОЙ формы группы, сдвинутой вниз на
/// `shadow.offset_px` (то есть поле группы в точке [`crate::shape::shadow_probe_point`]); внутри < 0.
/// Альфа: `falloff · shadow.alpha · opacity · effect` — тень растёт вместе с материализацией и
/// гаснет с прозрачностью группы. Тень рисуется ПОД стеклом ([`shade_glass_with_shadow`]).
pub fn shade_glass_shadow(
    shifted_group_sd_px: f32,
    shadow: &ShadowParamsPx,
    effect: f32,
    opacity: Opacity,
    aa_width_px: f32,
) -> PremultipliedColor {
    if shadow.alpha <= 0.0 {
        return PremultipliedColor::TRANSPARENT;
    }
    let falloff = shadow_falloff(shifted_group_sd_px, shadow.radius_px, aa_width_px);
    PremultipliedColor {
        r: 0.0,
        g: 0.0,
        b: 0.0,
        a: falloff * shadow.alpha * opacity.get() * effect.clamp(0.0, 1.0),
    }
}

/// Полный пиксель группы: стекло ([`shade_glass_pixel`]) поверх своей тени. Именно это рисует
/// шейдер `ui_glass`; тень видна и вне формы (там стекло прозрачно).
pub fn shade_glass_with_shadow(
    backdrop: impl Fn(PxPoint) -> DisplayColor,
    input: &GlassPixelInput<'_>,
    shifted_group_sd_px: f32,
) -> PremultipliedColor {
    let shadow = shade_glass_shadow(
        shifted_group_sd_px,
        &input.params.shadow,
        input.effect,
        input.opacity,
        input.aa_width_px,
    );
    over(shade_glass_pixel(backdrop, input), shadow)
}

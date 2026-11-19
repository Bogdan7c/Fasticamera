//! Слои поверх преломлённого фона: тон, насыщенность, блик, тёмный край, свечение от касания.

use crate::color::{DisplayColor, luma_rec709};
use crate::geometry::{PxPoint, PxVector};
use crate::glass::reference::smoothstep;
use crate::glass::{DimmingParams, GlassParamsPx};
use crate::light::LightDirection;

/// Прозрачность чёрного затемняющего слоя для фона со средней яркостью `mean_luma` (0..1):
/// `max_alpha · smoothstep(low, high, luma)`. Монотонно: чем ярче фон, тем темнее тон; на тёмном
/// фоне затемнения нет. Потолок — 35 % из HIG Materials (`DimmingStyle::max_alpha`).
pub fn backdrop_dimming(mean_luma: f32, dimming: &DimmingParams) -> f32 {
    dimming.max_alpha * dimming_factor(mean_luma, dimming)
}

/// Доля затемнения 0..1 (то же, что [`backdrop_dimming`], но без потолка); ею же выбирается тон плёнки.
pub(crate) fn dimming_factor(mean_luma: f32, dimming: &DimmingParams) -> f32 {
    smoothstep(dimming.luminance_low, dimming.luminance_high, mean_luma)
}

/// Насыщенность: `luma + (c − luma)·saturation` по каждому каналу, результат зажат в 0..1.
/// `saturation = 1` — без изменений, 0 — серый, 1,5 — «живее» (как vibrancy у Kyant). Яркость по
/// закодированным значениям (см. [`luma_rec709`]); альфа сохраняется.
pub fn vibrancy(color: DisplayColor, saturation: f32) -> DisplayColor {
    let luma = luma_rec709(color);
    let adjust = |channel: f32| (luma + (channel - luma) * saturation).clamp(0.0, 1.0);
    DisplayColor {
        r: adjust(color.r),
        g: adjust(color.g),
        b: adjust(color.b),
        a: color.a,
    }
}

/// Вес «ободка»: 1 у края, плавно 0 на глубине `width_px` (`1 − smoothstep(0, width, depth)`).
fn rim_weight(depth_px: f32, width_px: f32) -> f32 {
    if width_px <= 0.0 {
        return 0.0;
    }
    1.0 - smoothstep(0.0, width_px, depth_px.max(0.0))
}

/// Яркость белого блика 0..1 (складывается с цветом): `alpha · rim · |dot(normal, light)|^exponent`.
/// `normal_out` — единичная нормаль наружу, `depth_px` — глубина от края внутрь, ободок шириной
/// `specular.rim_width_px`. Модуль скалярного произведения: свет подсвечивает обе стороны оси
/// (как у Kyant), поэтому знак направления света не важен.
pub fn specular_intensity(normal_out: PxVector, light: LightDirection, depth_px: f32, params: &GlassParamsPx) -> f32 {
    let alignment = normal_out.dot(light.as_vector()).abs();
    params.specular.alpha
        * rim_weight(depth_px, params.specular.rim_width_px)
        * alignment.powf(params.specular.exponent)
}

/// Прозрачность чёрного «тёмного края» iOS 27: `strength · rim` на ширине `dark_edge.width_px`.
pub fn dark_edge(depth_px: f32, params: &GlassParamsPx) -> f32 {
    params.dark_edge.strength * rim_weight(depth_px, params.dark_edge.width_px)
}

/// Прозрачность белого свечения от касания в пикселе `local`: радиальный спад
/// `glow_alpha · amount · (1 − dist/radius)²` до нуля на радиусе `glow_radius_px`. Точки пикселя и
/// касания — в ОДНОЙ системе координат (экранной), поэтому свечение «растекается» и на соседние
/// формы группы (Apple 219, 12:05). `amount ≤ 0` — свечения нет.
pub fn press_glow(local: PxPoint, touch_point: PxPoint, press_amount: f32, params: &GlassParamsPx) -> f32 {
    let amount = press_amount.clamp(0.0, 1.0);
    let radius = params.press.glow_radius_px;
    if amount <= 0.0 || radius <= 0.0 {
        return 0.0;
    }
    let falloff = (1.0 - (local - touch_point).length() / radius).clamp(0.0, 1.0);
    params.press.glow_alpha * amount * falloff * falloff
}

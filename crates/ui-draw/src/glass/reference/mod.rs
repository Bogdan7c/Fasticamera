//! Эталон формул стекла «в форме шейдера»: `f32`, без итераций, одно выражение — один оператор GLSL.
//!
//! GPU-рендер (`ui-render-vulkan`) сверяется с этими функциями тестами паритета, поэтому здесь нет
//! ничего, чего нельзя записать в шейдере. Системы координат: экранные пиксели, y вниз; расстояние
//! до формы `sd_px` — со знаком, внутри отрицательное (исправленное `d/|∇d|`, см. [`crate::shape`]).

mod compose;
mod effects;
mod lens;
mod shadow;

pub use compose::{GlassPixelInput, MATERIALIZE_LAYER_RAMP, PressInput, shade_glass_pixel};
pub use effects::{backdrop_dimming, dark_edge, press_glow, specular_intensity, vibrancy};
pub use lens::{ChannelOffsets, lens_profile, refraction_offsets};
pub use shadow::{shade_glass_shadow, shade_glass_with_shadow};

/// `smoothstep(0, 1, t)` для `t` в 0..1 (вне диапазона зажимается).
pub(crate) fn smoothstep01(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Как GLSL `smoothstep(edge0, edge1, x)`; при `edge0 == edge1` — ступенька.
pub(crate) fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    if (edge1 - edge0).abs() <= f32::EPSILON {
        return if x >= edge1 { 1.0 } else { 0.0 };
    }
    smoothstep01((x - edge0) / (edge1 - edge0))
}

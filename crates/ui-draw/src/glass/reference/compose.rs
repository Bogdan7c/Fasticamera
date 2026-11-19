//! Полная композиция пикселя стекла по эталону.

use crate::color::{DisplayColor, Opacity, PremultipliedColor};
use crate::geometry::{PxPoint, PxVector};
use crate::glass::GlassParamsPx;
use crate::glass::reference::effects::dimming_factor;
use crate::glass::reference::{
    backdrop_dimming, dark_edge, press_glow, refraction_offsets, smoothstep, specular_intensity, vibrancy,
};
use crate::light::LightDirection;
use crate::shape::coverage;

/// Нажатие, как его видит пиксель: сила и точка касания в ЭКРАННЫХ пикселях.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PressInput {
    /// 0 — не нажато, 1 — нажато полностью.
    pub amount: f32,
    pub touch_point_px: PxPoint,
}

impl PressInput {
    /// Нет нажатия.
    pub const NONE: PressInput = PressInput {
        amount: 0.0,
        touch_point_px: PxPoint::ORIGIN,
    };
}

/// Всё, что нужно знать о пикселе стекла.
#[derive(Debug, Clone, Copy)]
pub struct GlassPixelInput<'a> {
    /// Центр пикселя, экранные px.
    pub screen_pos: PxPoint,
    /// Исправленное расстояние до слитой формы группы, px (внутри < 0).
    pub group_sd_px: f32,
    /// Единичная нормаль слитой формы наружу.
    pub normal_out: PxVector,
    pub params: &'a GlassParamsPx,
    pub light: LightDirection,
    /// Средняя яркость фона под группой (Rec.709 по закодированным): адаптация общая для всей
    /// группы, «контейнер выравнивает адаптацию» (Apple 284, 24:46).
    pub backdrop_mean_luma: f32,
    pub press: PressInput,
    /// Материализация 0..1 (Apple 219, 2:55): 0 — стекла нет, 1 — полный эффект.
    pub effect: f32,
    pub opacity: Opacity,
    /// Ширина сглаживания края, px (в шейдере `fwidth`).
    pub aa_width_px: f32,
}

/// Диапазон `effect`, на котором стекло «включается» как слой. Ниже границы покрытие линейно растёт
/// от 0, чтобы при `effect = 0` фон не менялся совсем (иначе размытый фон создавал бы видимый
/// «шов» по контуру), а выше — материализацией управляют только сила линзы, блика и тона.
/// Инженерный выбор, подгонка, не Apple. Шейдер `ui_glass.frag` держит копию этого числа; паритет
/// закреплён тестами GPU (при `effect` внутри рампы расхождение с эталоном сразу видно).
pub const MATERIALIZE_LAYER_RAMP: f32 = 0.2;

/// Цвет пикселя стекла, premultiplied: его «over» кладут на кадр.
///
/// Порядок слоёв: (1) выборка размытого фона по трём каналам со смещениями линзы
/// ([`refraction_offsets`], умноженными на `effect`); (2) насыщенность ([`vibrancy`]) — к 1 с
/// ослаблением по `effect`; (3) плёнка тона: цвет от белого к чёрному по яркости фона, прозрачность
/// `tint_level · tint_alpha_at_full · effect`; (4) затемнение ([`backdrop_dimming`]); (5) тёмный
/// край ([`dark_edge`]); (6) свечение от касания; (7) блик ([`specular_intensity`], складывается).
/// Слои 3–7 умножаются на `effect`. Альфа результата: `coverage · opacity · ramp(effect)`.
/// Вне формы (`coverage = 0`) возвращается нуль.
pub fn shade_glass_pixel(
    backdrop: impl Fn(PxPoint) -> DisplayColor,
    input: &GlassPixelInput<'_>,
) -> PremultipliedColor {
    let params = input.params;
    let effect = input.effect.clamp(0.0, 1.0);
    let layer_alpha = coverage(input.group_sd_px, input.aa_width_px)
        * input.opacity.get()
        * smoothstep(0.0, MATERIALIZE_LAYER_RAMP, effect);
    if layer_alpha <= 0.0 {
        return PremultipliedColor::TRANSPARENT;
    }
    let depth_px = -input.group_sd_px;

    // (1) Линза: каждый канал берётся из своей точки фона.
    let offsets = refraction_offsets(input.group_sd_px, input.normal_out, params, input.press.amount).scaled(effect);
    let sample_at = |offset: PxVector| backdrop(input.screen_pos + offset);
    let mut color = DisplayColor::rgb(sample_at(offsets.r).r, sample_at(offsets.g).g, sample_at(offsets.b).b);

    // (2) Насыщенность, ослабленная до 1 при effect → 0.
    color = vibrancy(color, 1.0 + (params.saturation - 1.0) * effect);

    // (3) Плёнка тона: светлая над тёмным фоном, тёмная над светлым.
    let tone_value = 1.0 - dimming_factor(input.backdrop_mean_luma, &params.dimming);
    let tint_alpha = params.tint_level * params.tint_alpha_at_full * effect;
    color = color.lerp(DisplayColor::rgb(tone_value, tone_value, tone_value), tint_alpha);

    // (4) Затемнение: смесь с чёрным.
    let dimming = backdrop_dimming(input.backdrop_mean_luma, &params.dimming) * effect;
    color = color.lerp(DisplayColor::BLACK, dimming);

    // (5) Тёмный край.
    color = color.lerp(DisplayColor::BLACK, dark_edge(depth_px, params) * effect);

    // (6) Свечение от касания: смесь с белым.
    let glow = press_glow(input.screen_pos, input.press.touch_point_px, input.press.amount, params) * effect;
    color = color.lerp(DisplayColor::WHITE, glow);

    // (7) Блик: белый добавляется к цвету.
    let specular = specular_intensity(input.normal_out, input.light, depth_px, params) * effect;
    color = DisplayColor::rgb(
        (color.r + specular).min(1.0),
        (color.g + specular).min(1.0),
        (color.b + specular).min(1.0),
    );

    color.with_alpha(layer_alpha).premultiplied()
}

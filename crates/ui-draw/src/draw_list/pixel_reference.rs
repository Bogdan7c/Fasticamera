//! Эталонный цвет пикселя целых элементов списка рисования (фигура, группа стекла) «в форме шейдера».
//!
//! `ui-render-vulkan` сверяет свои шейдеры `ui_shape` и `ui_glass` с этими функциями. Они склеивают
//! поле расстояния (экранные координаты) с формулами слоёв; сами формулы — в `shape` и `glass::reference`.

use crate::color::{DisplayColor, PremultipliedColor};
use crate::draw_list::{GlassGroupItem, ShapeItem};
use crate::geometry::PxPoint;
use crate::glass::reference::{GlassPixelInput, shade_glass_with_shadow};
use crate::light::LightDirection;
use crate::shape::{corrected_distance, coverage, over, shadow_falloff, shadow_probe_point};
use crate::transform::{Affine2, AffineError};

/// Расстояния фигуры в точке экрана.
struct ShapeDistances {
    /// Исправленное расстояние в пикселях экрана (внутри < 0).
    screen_px: f32,
    /// То же в локальных единицах фигуры (нужно обводке: её ширина задана в локальных px).
    local: f32,
    /// Длина экранного градиента локальной нормали (1/масштаб): локальные единицы → экранные.
    gradient_length: f32,
}

fn shape_distances(item: &ShapeItem, inverse: &Affine2, screen_point: PxPoint) -> ShapeDistances {
    let sample = corrected_distance(&item.geometry, inverse.apply_point(screen_point));
    let gradient = inverse.apply_gradient(sample.normal_out);
    match gradient.normalized() {
        Some(_) => ShapeDistances {
            screen_px: sample.distance / gradient.length(),
            local: sample.distance,
            gradient_length: gradient.length(),
        },
        None => ShapeDistances {
            screen_px: sample.distance,
            local: sample.distance,
            gradient_length: 1.0,
        },
    }
}

fn premultiplied_scaled(color: DisplayColor, factor: f32) -> PremultipliedColor {
    PremultipliedColor {
        r: color.r * color.a * factor,
        g: color.g * color.a * factor,
        b: color.b * color.a * factor,
        a: color.a * factor,
    }
}

/// Цвет пикселя закрашенной фигуры (premultiplied) в центре пикселя `screen_point`.
///
/// Порядок слоёв: заливка · покрытие; обводка поверх (центрирована на границе, ширина в локальных px
/// переводится в экранные делением на масштаб); тень ПОД ними (та же форма, сдвинутая вниз на
/// `offset_px`, мягкий край ±radius); всё умножается на `opacity`. `aa_width_px` — ширина
/// сглаживания края (в шейдере 1 px). Ошибка — вырожденное преобразование фигуры.
pub fn shade_shape_pixel(
    item: &ShapeItem,
    screen_point: PxPoint,
    aa_width_px: f32,
) -> Result<PremultipliedColor, AffineError> {
    let inverse = item.transform.inverse()?;
    let here = shape_distances(item, &inverse, screen_point);
    let mut color = premultiplied_scaled(item.fill, coverage(here.screen_px, aa_width_px));
    if let Some(stroke) = item.stroke.filter(|stroke| stroke.width_px > 0.0) {
        let stroke_distance = (here.local.abs() - 0.5 * stroke.width_px) / here.gradient_length;
        let stroke_layer = premultiplied_scaled(stroke.color, coverage(stroke_distance, aa_width_px));
        color = over(stroke_layer, color);
    }
    if let Some(shadow) = item.shadow.filter(|shadow| shadow.alpha > 0.0) {
        let shifted = shape_distances(item, &inverse, shadow_probe_point(screen_point, shadow.offset_px));
        let falloff = shadow_falloff(shifted.screen_px, shadow.radius_px, aa_width_px);
        let shadow_layer = PremultipliedColor {
            r: 0.0,
            g: 0.0,
            b: 0.0,
            a: falloff * shadow.alpha,
        };
        color = over(color, shadow_layer);
    }
    let opacity = item.opacity.get();
    Ok(PremultipliedColor {
        r: color.r * opacity,
        g: color.g * opacity,
        b: color.b * opacity,
        a: color.a * opacity,
    })
}

/// Цвет пикселя группы стекла (premultiplied): стекло поверх своей тени, с нажатием группы
/// ([`GlassGroupItem::dominant_press`]). `backdrop` — цвет размытого фона в экранной точке,
/// `backdrop_mean_luma` — средняя яркость фона под группой. Группа без членов — прозрачность.
pub fn shade_glass_group_pixel(
    group: &GlassGroupItem,
    light: LightDirection,
    screen_point: PxPoint,
    backdrop_mean_luma: f32,
    backdrop: impl Fn(PxPoint) -> DisplayColor,
    aa_width_px: f32,
) -> Result<PremultipliedColor, AffineError> {
    let Some(field) = group.field_at(screen_point)? else {
        return Ok(PremultipliedColor::TRANSPARENT);
    };
    let shifted_probe = shadow_probe_point(screen_point, group.params.shadow.offset_px);
    // Члены есть (поле выше нашлось), значит и сдвинутое поле есть; ветка None — для полноты.
    let shifted_sd_px = group
        .field_at(shifted_probe)?
        .map_or(f32::INFINITY, |shifted| shifted.distance_px);
    let input = GlassPixelInput {
        screen_pos: screen_point,
        group_sd_px: field.distance_px,
        normal_out: field.normal_out,
        params: &group.params,
        light,
        backdrop_mean_luma,
        press: group.dominant_press(),
        effect: group.effect,
        opacity: group.opacity,
        aa_width_px,
    };
    Ok(shade_glass_with_shadow(backdrop, &input, shifted_sd_px))
}

//! K9: вид рамки фокуса — тонкий квадрат, четыре тика внутрь по серединам сторон, «солнце» справа
//! (диск и 8 лучей) и вертикальная направляющая, пока палец на «солнце».

use ui_draw::color::DisplayColor;
use ui_draw::color::Opacity;
use ui_draw::draw_list::{DrawItem, ShapeItem, StrokeStyle};
use ui_draw::geometry::PxSize;
use ui_draw::scene_bridge::NodePlacementPx;
use ui_draw::shape::{CornerSpec, ShapeGeometry};
use ui_draw::transform::Affine2;

use super::error::ViewError;
use super::scale::PxPerPt;
use crate::focus_frame::{FocusFrameSnapshot, SunInteraction};
use crate::tokens::{ColorTokens, FocusTokens};

/// Рамка фокуса и «солнце». `placement` — размещение узла рамки (центр — точка фокуса); масштаб
/// появления и прозрачность берутся из снимка. «Солнце» ставится в системе рамки: правее правой
/// стороны и выше центра на токены, плюс ход по значению шкалы (`sun_travel_up_pt`).
///
/// Предусловие: узел рамки **не повёрнут** (`placement.transform` — перенос и, возможно, масштаб).
/// Смещение «солнца» задано в системе рамки, поэтому у повёрнутого узла и сдвиг «вверх», и сам
/// значок повернулись бы вместе с ним. Так и задумано: по `behavior.md` 2.7 при повороте телефона
/// вращаются только значки на месте, а рамка фокуса — не значок, экран её узел не вращает
/// (ход «солнца» остаётся вертикальным на экране). Делать вид «верным для любого трансформа»
/// не стали: нужной семантики для повёрнутой рамки (куда «вверх»?) у iOS нет.
pub fn focus_frame_items<V>(
    placement: &NodePlacementPx,
    snapshot: &FocusFrameSnapshot<V>,
    scale: PxPerPt,
    tokens: &FocusTokens,
    colors: &ColorTokens,
) -> Result<Vec<DrawItem>, ViewError> {
    let combined = placement.opacity * snapshot.opacity;
    let opacity = Opacity::new(combined).map_err(|_| ViewError::InvalidOpacity {
        what: "focus_frame",
        value: combined,
    })?;
    let host = Affine2::uniform_scale(snapshot.scale as f32).then(placement.transform);
    let color = colors.focus_yellow;
    let stroke_px = scale.px(tokens.stroke_width_pt);
    let tick_px = scale.px(tokens.tick_length_pt);
    let (half_width, half_height) = (placement.size.width * 0.5, placement.size.height * 0.5);

    let shape = |size: PxSize, centered_at: Affine2, fill: DisplayColor, stroke: Option<StrokeStyle>| {
        DrawItem::Shape(ShapeItem {
            geometry: ShapeGeometry::RoundedRect {
                size,
                corner: CornerSpec::circular(0.0),
            },
            transform: centered_at.then(host),
            fill,
            stroke,
            shadow: None,
            opacity,
        })
    };

    // Контур: прозрачный квадрат с тонкой обводкой.
    let mut items = vec![shape(
        placement.size,
        Affine2::IDENTITY,
        DisplayColor::TRANSPARENT,
        Some(StrokeStyle {
            width_px: stroke_px,
            color,
        }),
    )];
    // Тики внутрь от середин сторон: центр тика отстоит от стороны на половину его длины.
    let vertical_tick = PxSize::new(stroke_px, tick_px);
    let horizontal_tick = PxSize::new(tick_px, stroke_px);
    for (size, dx, dy) in [
        (vertical_tick, 0.0, -half_height + tick_px * 0.5),
        (vertical_tick, 0.0, half_height - tick_px * 0.5),
        (horizontal_tick, -half_width + tick_px * 0.5, 0.0),
        (horizontal_tick, half_width - tick_px * 0.5, 0.0),
    ] {
        items.push(shape(size, Affine2::translate(dx, dy), color, None));
    }

    // Положение «солнца» в системе рамки: правее правой стороны; значение шкалы двигает его вверх/вниз.
    let sun_dx = half_width + scale.px(tokens.sun_offset_right_of_frame_pt);
    let sun_start_dy = -scale.px(tokens.sun_offset_above_center_pt);
    let sun_dy = sun_start_dy - scale.px(snapshot.sun_travel_up_pt);

    // Направляющая — только пока палец на «солнце»; лежит под значком.
    if snapshot.sun_interaction == SunInteraction::Dragged {
        for (size, centered_at) in guide_segments(tokens, scale, sun_dx, sun_start_dy, sun_dy) {
            items.push(shape(size, centered_at, color, None));
        }
    }

    // Диск и лучи «солнца» рисуются фигурами (значок-картинка не даёт нужных размеров лучей).
    let sun_host = Affine2::translate(sun_dx, sun_dy).then(host);
    items.push(DrawItem::Shape(ShapeItem {
        geometry: ShapeGeometry::Circle {
            diameter: scale.px(tokens.sun_disc_diameter_pt),
        },
        transform: sun_host,
        fill: color,
        stroke: None,
        shadow: None,
        opacity,
    }));
    let ray_length_px = scale.px(tokens.sun_ray_outer_radius_pt - tokens.sun_ray_inner_radius_pt);
    let ray_center_radius_px = scale.px((tokens.sun_ray_outer_radius_pt + tokens.sun_ray_inner_radius_pt) * 0.5);
    for ray_index in 0..tokens.sun_ray_count {
        let angle = std::f32::consts::TAU * ray_index as f32 / tokens.sun_ray_count as f32;
        // Луч — тонкий прямоугольник над центром; поворот вокруг центра «солнца» раскладывает лучи по кругу.
        let centered_at = Affine2::translate(0.0, -ray_center_radius_px)
            .then(Affine2::rotate(angle))
            .then(sun_host);
        items.push(DrawItem::Shape(ShapeItem {
            geometry: ShapeGeometry::RoundedRect {
                size: PxSize::new(scale.px(tokens.sun_ray_width_pt), ray_length_px),
                corner: CornerSpec::circular(0.0),
            },
            transform: centered_at,
            fill: color,
            stroke: None,
            shadow: None,
            opacity,
        }));
    }
    Ok(items)
}

/// Куски вертикальной направляющей: отрезок фиксированной длины, центрированный на исходной точке
/// «солнца», минус разрыв радиусом `guide_gap_radius_pt` вокруг текущего положения «солнца».
/// Возвращает `(размер, центр в системе рамки)` каждого непустого куска; координаты — px, y вниз.
fn guide_segments(
    tokens: &FocusTokens,
    scale: PxPerPt,
    sun_dx: f32,
    sun_start_dy: f32,
    sun_dy: f32,
) -> Vec<(PxSize, Affine2)> {
    let half_line = scale.px(tokens.guide_line_length_pt) * 0.5;
    let gap = scale.px(tokens.guide_gap_radius_pt);
    let (line_top, line_bottom) = (sun_start_dy - half_line, sun_start_dy + half_line);
    let (gap_top, gap_bottom) = (sun_dy - gap, sun_dy + gap);
    let width_px = scale.px(tokens.guide_line_width_pt);
    [
        (line_top, line_bottom.min(gap_top)),
        (line_top.max(gap_bottom), line_bottom),
    ]
    .into_iter()
    .filter(|(from, to)| to - from > f32::EPSILON)
    .map(|(from, to)| {
        (
            PxSize::new(width_px, to - from),
            Affine2::translate(sun_dx, (from + to) * 0.5),
        )
    })
    .collect()
}

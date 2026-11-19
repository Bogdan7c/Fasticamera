//! K9: вид рамки фокуса — тонкий квадрат, четыре тика внутрь по серединам сторон, «солнце» справа.

use ui_draw::color::DisplayColor;
use ui_draw::color::Opacity;
use ui_draw::draw_list::{DrawItem, ShapeItem, StrokeStyle};
use ui_draw::geometry::PxSize;
use ui_draw::icons::IconId;
use ui_draw::scene_bridge::NodePlacementPx;
use ui_draw::shape::{CornerSpec, ShapeGeometry};
use ui_draw::transform::Affine2;

use super::content::centered_icon;
use super::error::ViewError;
use super::scale::PxPerPt;
use crate::focus_frame::FocusFrameSnapshot;
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

    // «Солнце»: центр правее правой стороны и выше центра; значение шкалы двигает его вверх/вниз.
    let sun_dx = half_width + scale.px(tokens.sun_offset_right_of_frame_pt);
    let sun_dy = -scale.px(tokens.sun_offset_above_center_pt + snapshot.sun_travel_up_pt);
    items.push(centered_icon(
        IconId::Exposure,
        tokens.sun_width_pt.max(tokens.sun_height_pt),
        scale,
        Affine2::translate(sun_dx, sun_dy).then(host),
        color,
        opacity,
    ));
    Ok(items)
}

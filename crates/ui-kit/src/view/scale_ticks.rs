//! K3: вид шкалы с упорами — риски по `marks()`/`snapshot()` и неподвижная отметка текущего значения.

use ui_core::time::MonotonicNanos;
use ui_draw::color::{DisplayColor, Opacity};
use ui_draw::draw_list::{DrawItem, ShapeItem};
use ui_draw::geometry::PxSize;
use ui_draw::scene_bridge::NodePlacementPx;
use ui_draw::shape::{CornerSpec, ShapeGeometry};
use ui_draw::transform::Affine2;

use super::content::opacity_of_placement;
use super::error::ViewError;
use super::scale::PxPerPt;
use crate::drive::{GrowthDirection, ScreenAxis};
use crate::scale::{SnapScale, TickVisibility};
use crate::tokens::{ColorTokens, ScaleTickTokens};
use crate::value::{Emphasis, ValueModel};

/// Куда на экране смотрит ось роста значения: `+1` — большие значения лежат в сторону роста
/// экранной координаты (вправо или вниз), `-1` — в обратную.
///
/// Контент едет за пальцем: при `FingerLeft` / `FingerUp` палец идёт к началу экранной оси,
/// контент вместе с ним, значит большие значения подъезжают с конца оси (`+1`); при `FingerRight` /
/// `FingerDown` — наоборот (`-1`).
pub(super) fn growth_sign(direction: GrowthDirection) -> f32 {
    match direction {
        GrowthDirection::FingerLeft | GrowthDirection::FingerUp => 1.0,
        GrowthDirection::FingerRight | GrowthDirection::FingerDown => -1.0,
    }
}

/// Риски шкалы и отметка текущего значения. `placement` — размещение узла шкалы: центр рамки —
/// положение текущего значения, длина рамки вдоль оси ограничивает видимые риски (риски за краем
/// рамки не рисуются). Ось, направление роста и видимость берутся из описания самой шкалы:
/// горизонтальная шкала рисует вертикальные риски, вертикальная — горизонтальные.
///
/// Порядок элементов: риски в порядке `marks()` (обычные — приглушённым цветом, крупные —
/// основным), последним — жёлтая отметка текущего значения (она поверх рисок). Шкала с
/// [`TickVisibility::Hidden`] ничего не рисует (это «солнце» рамки фокуса: оно значок, а не линейка).
///
/// Поворот узла наследуется от `placement.transform`, как у остальных видов.
pub fn scale_items<M: ValueModel + Clone>(
    placement: &NodePlacementPx,
    scale: &SnapScale<M>,
    now: MonotonicNanos,
    px_per_pt: PxPerPt,
    tokens: &ScaleTickTokens,
    colors: &ColorTokens,
) -> Result<Vec<DrawItem>, ViewError> {
    let spec = scale.spec();
    if spec.ticks == TickVisibility::Hidden {
        return Ok(Vec::new());
    }
    let opacity = opacity_of_placement(placement, "scale")?;
    let axis = spec.direction.axis();
    let sign = growth_sign(spec.direction);
    let half_extent_px = match axis {
        ScreenAxis::Horizontal => placement.size.width,
        ScreenAxis::Vertical => placement.size.height,
    } * 0.5;
    let current_offset = scale.snapshot(now).offset_points;

    let mut items = Vec::new();
    for mark in scale.marks() {
        let along_px = sign * px_per_pt.px(mark.offset_points - current_offset);
        if along_px.abs() > half_extent_px {
            continue;
        }
        let (length_pt, color) = match mark.emphasis {
            Emphasis::Tick => (tokens.tick_length_pt, colors.text_muted),
            Emphasis::Reference => (tokens.reference_length_pt, colors.text_primary),
        };
        items.push(tick_item(
            placement, axis, along_px, length_pt, color, px_per_pt, tokens, opacity,
        ));
    }
    items.push(tick_item(
        placement,
        axis,
        0.0,
        tokens.current_mark_length_pt,
        colors.focus_yellow,
        px_per_pt,
        tokens,
        opacity,
    ));
    Ok(items)
}

/// Одна риска: прямоугольник, центр которого сдвинут на `along_px` вдоль оси шкалы от центра узла.
/// Толщина — вдоль оси, длина — поперёк; у вертикальной шкалы размеры меняются местами.
#[allow(clippy::too_many_arguments)]
fn tick_item(
    placement: &NodePlacementPx,
    axis: ScreenAxis,
    along_px: f32,
    length_pt: f64,
    color: DisplayColor,
    px_per_pt: PxPerPt,
    tokens: &ScaleTickTokens,
    opacity: Opacity,
) -> DrawItem {
    let thickness_px = px_per_pt.px(tokens.thickness_pt);
    let length_px = px_per_pt.px(length_pt);
    let (size, shift) = match axis {
        ScreenAxis::Horizontal => (PxSize::new(thickness_px, length_px), Affine2::translate(along_px, 0.0)),
        ScreenAxis::Vertical => (PxSize::new(length_px, thickness_px), Affine2::translate(0.0, along_px)),
    };
    DrawItem::Shape(ShapeItem {
        geometry: ShapeGeometry::RoundedRect {
            size,
            corner: CornerSpec::circular(0.0),
        },
        transform: shift.then(placement.transform),
        fill: color,
        stroke: None,
        shadow: None,
        opacity,
    })
}

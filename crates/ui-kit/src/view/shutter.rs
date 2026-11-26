//! K10: вид затвора — серое полупрозрачное кольцо со светлым ободком и белый диск, без стекла (14p, решение 300).

use ui_draw::color::Opacity;
use ui_draw::draw_list::{DrawItem, ShapeItem, StrokeStyle};
use ui_draw::glass::PressStyle;
use ui_draw::scene_bridge::NodePlacementPx;
use ui_draw::shape::ShapeGeometry;
use ui_draw::transform::Affine2;

use super::error::ViewError;
use super::scale::PxPerPt;
use crate::shutter::{ShutterLook, ShutterSnapshot};
use crate::tokens::{ColorTokens, ShutterTokens};

/// Затвор. `placement` — размещение узла затвора (круг внешнего диаметра). Порядок: серое полупрозрачное
/// кольцо со светлым ободком в 1 pt по внешнему краю (`[measured: R00 t=10.027]`), затем поверх белый диск
/// меньшего диаметра — между диском и краем остаётся кольцо ≈ 6,1 pt. Заливка кольца — круг внешнего
/// диаметра: диск его перекрывает, поэтому «видно» только кольцо.
///
/// Стекла под кольцом нет (14p, решение 300): затвор — своя отрисовка, от материала панелей не зависит, и
/// кадр с затвором не строит пирамиду размытия.
///
/// Нажатие сжимает диск и увеличивает кольцо: кольцо растёт на прирост `ring_press.scale_gain`, как раньше
/// росло стеклянное кольцо, — реакция на нажатие прежняя (решение 270). Вид (`Busy`, `Disabled`) притушает
/// диск и затвор по токенам.
pub fn shutter_items(
    placement: &NodePlacementPx,
    snapshot: &ShutterSnapshot,
    ring_press: &PressStyle,
    scale: PxPerPt,
    tokens: &ShutterTokens,
    colors: &ColorTokens,
) -> Result<Vec<DrawItem>, ViewError> {
    let look_opacity = match snapshot.look {
        ShutterLook::Disabled => tokens.disabled_opacity as f32,
        ShutterLook::Ready | ShutterLook::Pressed | ShutterLook::Busy => 1.0,
    };
    let combined = placement.opacity * look_opacity;
    let ring_opacity = Opacity::new(combined).map_err(|_| ViewError::InvalidOpacity {
        what: "shutter",
        value: combined,
    })?;
    let disc_opacity_value = match snapshot.look {
        ShutterLook::Busy => combined * tokens.busy_disc_opacity as f32,
        _ => combined,
    };
    let disc_opacity = Opacity::new(disc_opacity_value).map_err(|_| ViewError::InvalidOpacity {
        what: "shutter_disc",
        value: disc_opacity_value,
    })?;
    let disc_ratio = (tokens.disc_diameter_pt / tokens.outer_diameter_pt) as f32;
    let outer_px = placement.size.width.min(placement.size.height);
    // Масштаб нажатия кольца — та же формула, что у стеклянной кнопки: 1 + прирост · сила нажатия.
    let ring_press_scale = 1.0 + ring_press.scale_gain * snapshot.press_amount.clamp(0.0, 1.0);
    // Ободок: обводка центрирована на границе, поэтому круг уменьшен на толщину ободка — тогда
    // обводка занимает последний 1 pt внутри внешнего диаметра, а не выходит за него.
    let rim_px = scale.px(tokens.ring_rim_width_pt);
    let ring_fill = DrawItem::Shape(ShapeItem {
        geometry: ShapeGeometry::Circle {
            diameter: outer_px - rim_px,
        },
        transform: Affine2::uniform_scale(ring_press_scale).then(placement.transform),
        fill: tokens.ring_fill,
        stroke: Some(StrokeStyle {
            width_px: rim_px,
            color: tokens.ring_rim_color,
        }),
        shadow: None,
        opacity: ring_opacity,
    });
    Ok(vec![
        ring_fill,
        DrawItem::Shape(ShapeItem {
            geometry: ShapeGeometry::Circle {
                diameter: outer_px * disc_ratio,
            },
            transform: Affine2::uniform_scale(snapshot.disc_scale as f32).then(placement.transform),
            fill: colors.text_primary,
            stroke: None,
            shadow: None,
            opacity: disc_opacity,
        }),
    ])
}

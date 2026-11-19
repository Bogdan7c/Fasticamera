//! K10: вид затвора — тонкое стеклянное кольцо (`glass.md` раздел 7) и белый диск.

use ui_draw::color::Opacity;
use ui_draw::draw_list::{DrawItem, PressState, ShapeItem};
use ui_draw::glass::GlassStyle;
use ui_draw::scene_bridge::NodePlacementPx;
use ui_draw::shape::ShapeGeometry;
use ui_draw::transform::Affine2;

use super::error::ViewError;
use super::glass_surface::{GlassGroupBuilder, GlassShape};
use super::scale::PxPerPt;
use crate::shutter::{ShutterLook, ShutterSnapshot};
use crate::tokens::{ColorTokens, ShutterTokens};

/// Затвор. `placement` — размещение узла затвора (круг внешнего диаметра). Порядок: стеклянный
/// круг, затем поверх него белый диск меньшего диаметра — снаружи остаётся кольцо ≈ 6,1 pt.
/// Нажатие сжимает диск и даёт стеклу масштаб нажатия из пресета; вид (`Busy`, `Disabled`)
/// притушает диск и затвор по токенам.
pub fn shutter_items(
    placement: &NodePlacementPx,
    snapshot: &ShutterSnapshot,
    ring_style: &GlassStyle,
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
    let mut ring = GlassGroupBuilder::new(ring_style, scale);
    ring.add_member(
        GlassShape::Circle,
        *placement,
        PressState {
            amount: snapshot.press_amount,
            touch_point_local: ui_draw::geometry::PxPoint::ORIGIN,
        },
    )?;
    let group = ring.build(1.0, ring_opacity)?;

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
    Ok(vec![
        DrawItem::Glass(group),
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

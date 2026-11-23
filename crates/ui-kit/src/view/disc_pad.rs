//! K12: вид площадки «точка в круге» — перекрестие центра и точка-шайба.
//!
//! Сам круг (подложку) не рисует: фон кольца/диска — дело сборки экрана (растр [`super::ring_picture`] или
//! стекло), вид отдаёт только то, что зависит от значения площадки. Предметной области нет.

use ui_draw::color::DisplayColor;
use ui_draw::draw_list::{DrawItem, ShapeItem, StrokeStyle};
use ui_draw::geometry::PxSize;
use ui_draw::scene_bridge::NodePlacementPx;
use ui_draw::shape::{CornerSpec, ShapeGeometry};
use ui_draw::transform::Affine2;

use super::content::opacity_of_placement;
use super::error::ViewError;
use super::scale::PxPerPt;
use crate::disc_pad::DiscPad;
use crate::tokens::{ColorTokens, DiscPadTokens};

/// Элементы вида площадки: сначала перекрестие центра, поверх — шайба. Центр круга = центр узла-хоста.
///
/// Шайба стоит в `(x · радиус, −y · радиус)` от центра: значение площадки имеет ось y ВВЕРХ, экран — вниз,
/// поэтому знак y меняется здесь (парный переворот к `DiscPad::eye_delta_from_screen`).
pub fn disc_pad_items(
    placement: &NodePlacementPx,
    pad: &DiscPad,
    px_per_pt: PxPerPt,
    tokens: &DiscPadTokens,
    colors: &ColorTokens,
) -> Result<Vec<DrawItem>, ViewError> {
    let node_opacity = opacity_of_placement(placement, "disc_pad")?;
    let line = |size_pt: (f64, f64), alpha: f32| {
        DrawItem::Shape(ShapeItem {
            geometry: ShapeGeometry::RoundedRect {
                size: PxSize::new(px_per_pt.px(size_pt.0), px_per_pt.px(size_pt.1)),
                corner: CornerSpec::circular(0.0),
            },
            transform: placement.transform,
            fill: colors.text_primary.with_alpha(alpha),
            stroke: None,
            shadow: None,
            opacity: node_opacity,
        })
    };
    let mut items = vec![
        line(
            (tokens.crosshair_length_pt, tokens.crosshair_thickness_pt),
            tokens.crosshair_alpha,
        ),
        line(
            (tokens.crosshair_thickness_pt, tokens.crosshair_length_pt),
            tokens.crosshair_alpha,
        ),
    ];

    let point = pad.point();
    let radius_pt = pad.spec().radius_pt;
    let puck_center = Affine2::translate(
        px_per_pt.px(point.x() * radius_pt),
        px_per_pt.px(-point.y() * radius_pt),
    )
    .then(placement.transform);
    items.push(DrawItem::Shape(ShapeItem {
        geometry: ShapeGeometry::Circle {
            diameter: px_per_pt.px(tokens.puck_diameter_pt),
        },
        transform: puck_center,
        fill: colors.text_primary.with_alpha(tokens.puck_fill_alpha),
        stroke: Some(StrokeStyle {
            width_px: px_per_pt.px(tokens.puck_stroke_pt),
            color: DisplayColor {
                a: 1.0,
                ..colors.text_primary
            },
        }),
        shadow: None,
        opacity: node_opacity,
    }));
    Ok(items)
}

//! K8: вид плашки — заполненная капсула цвета токена и текст поверх.

use ui_core::geometry::Size;
use ui_draw::color::Opacity;
use ui_draw::draw_list::{DrawItem, ShapeItem};
use ui_draw::scene_bridge::NodePlacementPx;
use ui_draw::shape::ShapeGeometry;
use ui_draw::text::TextRun;
use ui_draw::transform::Affine2;

use super::content::centered_text;
use super::error::ViewError;
use super::scale::PxPerPt;
use crate::banner::BannerSnapshot;
use crate::tokens::{BannerTokens, ColorTokens};

/// Размер узла плашки в pt: ширина по тексту, но не меньше `min_width_pt`; высота — токен.
/// Нужен экрану, чтобы разметить узел до рисования.
pub fn measure_banner_pt(text_advance_px: f32, scale: PxPerPt, tokens: &BannerTokens) -> Size {
    let text_width_pt = f64::from(text_advance_px / scale.get());
    Size::new(
        (text_width_pt + 2.0 * tokens.horizontal_padding_pt).max(tokens.min_width_pt),
        tokens.height_pt,
    )
}

/// Плашка: капсула + текст. Масштаб и прозрачность появления берутся из снимка, поверх
/// размещения узла (положение и размер узла экран задаёт раскладкой).
pub fn banner_items(
    placement: &NodePlacementPx,
    snapshot: &BannerSnapshot,
    text: &TextRun,
    colors: &ColorTokens,
) -> Result<Vec<DrawItem>, ViewError> {
    let combined = placement.opacity * snapshot.opacity;
    let opacity = Opacity::new(combined).map_err(|_| ViewError::InvalidOpacity {
        what: "banner",
        value: combined,
    })?;
    let host = Affine2::uniform_scale(snapshot.scale as f32).then(placement.transform);
    Ok(vec![
        DrawItem::Shape(ShapeItem {
            geometry: ShapeGeometry::Capsule { size: placement.size },
            transform: host,
            fill: colors.focus_yellow,
            stroke: None,
            shadow: None,
            opacity,
        }),
        centered_text(text, host, colors.plate_text, opacity),
    ])
}

//! K7: вид панели — стеклянная панель из снимка `PanelMorph`.

use ui_draw::color::Opacity;
use ui_draw::draw_list::{GlassGroupItem, PressState};
use ui_draw::geometry::PxSize;
use ui_draw::glass::GlassStyle;
use ui_draw::scene_bridge::NodePlacementPx;
use ui_draw::transform::Affine2;

use super::error::ViewError;
use super::glass_surface::{GlassGroupBuilder, GlassShape};
use super::scale::PxPerPt;
use crate::panel_morph::PanelSnapshot;

/// Стеклянная панель: рамка и радиус берутся из снимка, материализация — из `effect`.
///
/// Рамка снимка лежит в абсолютных координатах сцены (pt), поэтому размещение строится по ней
/// напрямую, без преобразования узла: панель не поворачивается вместе с телефоном (панели iOS
/// остаются в портрете, поворачивается только содержимое — его ставит экран своими иконками).
/// Параметры стекла считаются по ТЕКУЩЕМУ размеру рамки, поэтому свёрнутая панель не получает
/// линзу большой формы. Прозрачность содержимого и источника экран берёт из снимка сам.
pub fn panel_glass_group(
    snapshot: &PanelSnapshot,
    style: &GlassStyle,
    scale: PxPerPt,
) -> Result<GlassGroupItem, ViewError> {
    panel_glass_group_hosted(snapshot, style, scale, Affine2::IDENTITY)
}

/// То же, но панель живёт внутри повёрнутого хоста (раздел калибровки в альбоме): `host` переводит пиксели
/// системы рамки снимка в пиксели экрана и целиком определяет поворот. Рамка и радиус по-прежнему берутся из
/// снимка в системе хоста, а центр панели едет вместе с хостом, поэтому морф идёт в осях хоста, а не экрана.
pub fn panel_glass_group_hosted(
    snapshot: &PanelSnapshot,
    style: &GlassStyle,
    scale: PxPerPt,
    host: Affine2,
) -> Result<GlassGroupItem, ViewError> {
    let center = snapshot.frame.center();
    let placement = NodePlacementPx {
        size: PxSize::new(
            scale.px(snapshot.frame.size.width),
            scale.px(snapshot.frame.size.height),
        ),
        transform: Affine2::translate(scale.px(center.x), scale.px(center.y)).then(host),
        opacity: 1.0,
    };
    let mut builder = GlassGroupBuilder::new(style, scale);
    builder.add_member(
        GlassShape::RoundedRect {
            corner_radius_pt: snapshot.corner_radius_pt as f32,
        },
        placement,
        PressState::RELEASED,
    )?;
    builder.build(snapshot.effect, Opacity::OPAQUE)
}

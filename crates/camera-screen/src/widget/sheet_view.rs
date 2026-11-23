//! Рисование шторки меню: круглый пункт с подписью и «ручка» (решения 84, 94).
//!
//! Одно рисование на два меню: Controls экрана PHOTO (`controls_drawer.rs`) и меню раздела калибровки
//! (`calibration/menu`). Модуль знает только вид: что нарисовать ([`ItemView`]) и где (размещения узлов); какие
//! пункты есть и что значит нажатие, решают владельцы шторок.

use ui_core::geometry::Rect;
use ui_draw::color::{DisplayColor, Opacity};
use ui_draw::draw_list::{DrawItem, ShapeItem};
use ui_draw::geometry::PxSize;
use ui_draw::icons::IconId;
use ui_draw::scene_bridge::NodePlacementPx;
use ui_draw::shape::ShapeGeometry;
use ui_draw::transform::Affine2;
use ui_kit::tokens::TextRoleKind;
use ui_kit::view::{centered_icon, centered_text};

use super::Ctx;
use super::error::WidgetError;
use super::layers::{DrawLayers, Layer};
use super::text_runs::TextRuns;

/// Что нарисовано внутри круга пункта: значок или короткая подпись (RAW у iOS остаётся подписью, не значком).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ItemFace<'a> {
    Icon(IconId),
    Text(&'a str),
}

/// Как выглядит пункт.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ItemView<'a> {
    pub face: ItemFace<'a>,
    /// Подпись под кругом (заглавные).
    pub caption: &'a str,
    /// Включён ли пункт: значок или текст в кругу — жёлтые.
    pub highlighted: bool,
}

/// Размещения узлов пункта: круг (узел кнопки) и содержимое (узел значка, у экрана PHOTO он поворачивается).
#[derive(Debug, Clone, Copy)]
pub(crate) struct ItemNodes {
    pub button: NodePlacementPx,
    pub content: NodePlacementPx,
}

/// Рисует пункт: чёрный полупрозрачный круг, значок или подпись в нём и белая приглушённая подпись под кругом.
/// `alpha` и `slide_px` — прозрачность и сдвиг содержимого шторки из её морфа.
pub(crate) fn draw_menu_item(
    ctx: &Ctx<'_>,
    layers: &mut DrawLayers,
    texts: &mut TextRuns,
    nodes: ItemNodes,
    view: ItemView<'_>,
    (alpha, slide_px): (f32, f32),
) -> Result<(), WidgetError> {
    let sheet_config = &ctx.geometry.config().sheet;
    let colors = &ctx.tokens.colors;
    let ItemNodes { button, content } = nodes;
    let item_alpha = (button.opacity * alpha).clamp(0.0, 1.0);
    let opacity =
        Opacity::new(item_alpha).map_err(|_| WidgetError::Inconsistent("прозрачность пункта вне 0..1".to_owned()))?;
    // Круг всегда чёрный полупрозрачный; «включённость» показывает только значок (`[measured: C t=30.0]`).
    let fill = DisplayColor {
        a: sheet_config.item_fill_alpha,
        ..DisplayColor::BLACK
    };
    let slide = Affine2::translate(0.0, slide_px);
    layers.push(
        Layer::Content,
        DrawItem::Shape(ShapeItem {
            geometry: ShapeGeometry::Circle {
                diameter: button.size.width.min(button.size.height),
            },
            transform: slide.then(button.transform),
            fill,
            stroke: None,
            shadow: None,
            opacity,
        }),
    );
    let face_color = if view.highlighted {
        colors.focus_yellow
    } else {
        colors.text_primary
    };
    let content_host = slide.then(content.transform);
    match view.face {
        ItemFace::Icon(icon) => layers.push(
            Layer::Content,
            centered_icon(
                icon,
                sheet_config.item_icon_size_pt,
                ctx.px,
                content_host,
                face_color,
                opacity,
            ),
        ),
        ItemFace::Text(text) => {
            let run = texts.run(ctx, text, TextRoleKind::Caption)?;
            layers.push(Layer::Content, centered_text(run, content_host, face_color, opacity));
        }
    }
    // Подпись под кругом не поворачивается: она ставится от узла пункта, а не узла значка.
    let caption = texts.run(ctx, view.caption, TextRoleKind::ToolCaption)?;
    let caption_shift = Affine2::translate(
        0.0,
        button.size.height * 0.5 + ctx.px.px(sheet_config.caption_center_below_circle_pt),
    );
    // Подписи ВСЕХ пунктов белые, чуть приглушённые (`[measured: R00 t=22.512]`), включённый — не исключение.
    let caption_opacity = Opacity::new((item_alpha * sheet_config.caption_alpha).clamp(0.0, 1.0))
        .map_err(|_| WidgetError::Inconsistent("прозрачность подписи вне 0..1".to_owned()))?;
    layers.push(
        Layer::Content,
        centered_text(
            caption,
            caption_shift.then(slide).then(button.transform),
            colors.text_primary,
            caption_opacity,
        ),
    );
    Ok(())
}

/// «Ручка» шторки: белая полоска-капсула `handle` (центр — в системе, которую переводит `host`: на экране PHOTO это
/// единичное преобразование, в разделе — преобразование его корня). Только рисунок: жест закрытия висит на шторке.
pub(crate) fn draw_sheet_handle(ctx: &Ctx<'_>, layers: &mut DrawLayers, handle: Rect, (host, alpha): (Affine2, f32)) {
    let Ok(opacity) = Opacity::new(alpha.clamp(0.0, 1.0)) else {
        return;
    };
    let sheet_config = &ctx.geometry.config().sheet;
    let center = handle.center();
    layers.push(
        Layer::Content,
        DrawItem::Shape(ShapeItem {
            geometry: ShapeGeometry::Capsule {
                size: PxSize::new(ctx.px.px(handle.size.width), ctx.px.px(handle.size.height)),
            },
            transform: Affine2::translate(ctx.px.px(center.x), ctx.px.px(center.y)).then(host),
            fill: DisplayColor {
                a: sheet_config.handle_alpha,
                ..ctx.tokens.colors.text_primary
            },
            stroke: None,
            shadow: None,
            opacity,
        }),
    );
}

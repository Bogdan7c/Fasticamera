//! Подложка «вне кадра» (`scope-v1.md`: в v1 чёрная). Это **слой** между видоискателем и интерфейсом, а не
//! обрезка видоискателя: видоискатель рисует рендер в прямоугольник из `FramingIntent`, а подложка закрывает
//! остальное (в будущем здесь же затемнённое «View Outside the Frame»).

use ui_core::geometry::Rect;
use ui_draw::color::Opacity;

use super::error::WidgetError;
use super::layers::{DrawLayers, Layer};
use super::{Ctx, rect_item};

pub(crate) struct FrameMaskWidget;

impl FrameMaskWidget {
    pub(crate) fn draw(ctx: &Ctx<'_>, layers: &mut DrawLayers) -> Result<(), WidgetError> {
        let screen = ctx.geometry.screen();
        let frame = ctx.geometry.frame(ctx.state.settings().aspect);
        let color = ctx.geometry.config().overlay.outside_frame_color;
        // Полосы над и под кадром; боковых нет (кадр на всю ширину, решение 82).
        let bands = [
            Rect::from_xywh(
                screen.min_x(),
                screen.min_y(),
                screen.size.width,
                frame.min_y() - screen.min_y(),
            ),
            Rect::from_xywh(
                screen.min_x(),
                frame.max_y(),
                screen.size.width,
                screen.max_y() - frame.max_y(),
            ),
        ];
        for band in bands.into_iter().filter(|band| band.size.height > 0.0) {
            layers.push(Layer::Backdrop, rect_item(band, color, Opacity::OPAQUE, ctx.px));
        }
        Ok(())
    }
}

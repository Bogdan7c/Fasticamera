//! Сетка 3 × 3 по кадру (две вертикальные и две горизонтальные линии), только когда включена в настройках.

use ui_core::geometry::Rect;
use ui_draw::color::{DisplayColor, Opacity};

use super::error::WidgetError;
use super::layers::{DrawLayers, Layer};
use super::{Ctx, rect_item};
use crate::settings::GridSetting;

pub(crate) struct GridOverlayWidget;

impl GridOverlayWidget {
    pub(crate) fn draw(ctx: &Ctx<'_>, layers: &mut DrawLayers) -> Result<(), WidgetError> {
        if ctx.state.settings().grid != GridSetting::On {
            return Ok(());
        }
        let overlay = &ctx.geometry.config().overlay;
        let frame = ctx.geometry.frame(ctx.state.settings().aspect);
        let thickness = overlay.grid_line_width_pt;
        let color = DisplayColor {
            a: overlay.grid_line_alpha,
            ..ctx.tokens.colors.text_primary
        };
        for third in 1..=2 {
            let fraction = f64::from(third) / 3.0;
            let x = frame.min_x() + frame.size.width * fraction;
            let y = frame.min_y() + frame.size.height * fraction;
            let vertical = Rect::from_xywh(x - thickness * 0.5, frame.min_y(), thickness, frame.size.height);
            let horizontal = Rect::from_xywh(frame.min_x(), y - thickness * 0.5, frame.size.width, thickness);
            for line in [vertical, horizontal] {
                layers.push(Layer::FrameOverlay, rect_item(line, color, Opacity::OPAQUE, ctx.px));
            }
        }
        Ok(())
    }
}

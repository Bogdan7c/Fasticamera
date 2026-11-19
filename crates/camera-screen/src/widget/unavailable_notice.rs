//! Текст «Camera unavailable» по центру кадра, когда камеры нет (`is_camera_unavailable()`).

use ui_draw::color::Opacity;
use ui_draw::transform::Affine2;
use ui_kit::tokens::TextRoleKind;
use ui_kit::view::centered_text;

use super::Ctx;
use super::error::WidgetError;
use super::layers::{DrawLayers, Layer};
use super::text_runs::TextRuns;

pub(crate) struct UnavailableNoticeWidget;

impl UnavailableNoticeWidget {
    pub(crate) fn draw(ctx: &Ctx<'_>, layers: &mut DrawLayers, texts: &mut TextRuns) -> Result<(), WidgetError> {
        if !ctx.state.is_camera_unavailable() {
            return Ok(());
        }
        let run = texts.run(
            ctx,
            &ctx.geometry.config().overlay.unavailable_text,
            TextRoleKind::MenuItem,
        )?;
        let center = ctx.geometry.frame(ctx.state.settings().aspect).center();
        let host = Affine2::translate(ctx.px.px(center.x), ctx.px.px(center.y));
        layers.push(
            Layer::Content,
            centered_text(run, host, ctx.tokens.colors.text_muted, Opacity::OPAQUE),
        );
        Ok(())
    }
}

//! Отсчёт таймера: крупные цифры по центру кадра, пока `ShutterState::Countdown` (`[estimate]` вида).

use ui_draw::color::Opacity;
use ui_draw::transform::Affine2;
use ui_kit::tokens::TextRoleKind;
use ui_kit::view::centered_text;

use super::Ctx;
use super::error::WidgetError;
use super::layers::{DrawLayers, Layer};
use super::text_runs::TextRuns;
use crate::shutter::ShutterState;

const NANOS_PER_SECOND: i64 = 1_000_000_000;

pub(crate) struct CountdownWidget;

/// Сколько целых секунд осталось (округление вверх: «3, 2, 1», нуля на экране нет).
pub(crate) fn seconds_left(ends_at_nanos: i64, now_nanos: i64) -> i64 {
    let left = ends_at_nanos.saturating_sub(now_nanos);
    if left <= 0 {
        0
    } else {
        (left + NANOS_PER_SECOND - 1) / NANOS_PER_SECOND
    }
}

impl CountdownWidget {
    pub(crate) fn draw(ctx: &Ctx<'_>, layers: &mut DrawLayers, texts: &mut TextRuns) -> Result<(), WidgetError> {
        let ShutterState::Countdown { ends_at } = ctx.state.shutter() else {
            return Ok(());
        };
        let seconds = seconds_left(ends_at.as_nanos(), ctx.now.as_nanos());
        if seconds == 0 {
            return Ok(());
        }
        // Крупные табличные цифры отсчёта — отдельная роль типографики `Countdown` (кегль задают токены).
        let run = texts.run(ctx, &seconds.to_string(), TextRoleKind::Countdown)?;
        let center = ctx.geometry.frame(ctx.state.settings().aspect).center();
        let host = Affine2::translate(ctx.px.px(center.x), ctx.px.px(center.y));
        layers.push(
            Layer::Content,
            centered_text(run, host, ctx.tokens.colors.text_primary, Opacity::OPAQUE),
        );
        Ok(())
    }
}

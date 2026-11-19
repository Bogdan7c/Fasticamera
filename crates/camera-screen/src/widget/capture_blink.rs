//! Затемнение кадра при снимке (`motion.md` 4.1, `[measured]`): за один кадр до ~15 % яркости (затемнение, не
//! белая вспышка), удержание ≈ 66 мс, возврат без перелёта за ≈ 8 кадров (λ ≈ 18 1/с).
//!
//! Кривая — функция времени от запуска, поэтому 60 и 120 Гц дают одинаковую темноту в общие моменты.
//! TODO(ui-kit): готового «элемента по времени» без пружины в `ui-kit` нет; кривая живёт здесь.

use shell_contract::clock::MonotonicNanos;
use ui_draw::color::{DisplayColor, Opacity};

use super::error::WidgetError;
use super::layers::{DrawLayers, Layer};
use super::{Ctx, rect_item};
use crate::widgets_config::OverlayLayout;

#[derive(Debug, Default)]
pub(crate) struct CaptureBlinkWidget {
    started_at: Option<MonotonicNanos>,
}

/// Глубина затемнения (0..1) через `elapsed_s` секунд после запуска.
pub(crate) fn darkness_after(overlay: &OverlayLayout, elapsed_s: f64) -> f64 {
    if elapsed_s < 0.0 {
        return 0.0;
    }
    let attack_s = overlay.blink_attack_ms / 1000.0;
    let hold_s = overlay.blink_hold_ms / 1000.0;
    if elapsed_s < attack_s {
        overlay.blink_peak_darkness * elapsed_s / attack_s
    } else if elapsed_s < attack_s + hold_s {
        overlay.blink_peak_darkness
    } else {
        overlay.blink_peak_darkness * (-overlay.blink_release_rate_per_s * (elapsed_s - attack_s - hold_s)).exp()
    }
}

impl CaptureBlinkWidget {
    /// Запрос снимка ушёл (`Effect::Capture`): запускаем затемнение.
    pub(crate) fn trigger(&mut self, now: MonotonicNanos) {
        self.started_at = Some(now);
    }

    fn darkness(&self, ctx: &Ctx<'_>) -> f64 {
        self.started_at.map_or(0.0, |start| {
            let elapsed = (ctx.now.as_nanos() - start.as_nanos()) as f64 / 1e9;
            darkness_after(&ctx.geometry.config().overlay, elapsed)
        })
    }

    /// Закончившееся затемнение забывается, чтобы не считать хвост экспоненты вечно.
    pub(crate) fn advance(&mut self, overlay: &OverlayLayout, now: MonotonicNanos) {
        if let Some(start) = self.started_at {
            let elapsed = (now.as_nanos() - start.as_nanos()) as f64 / 1e9;
            let finished = elapsed > (overlay.blink_attack_ms + overlay.blink_hold_ms) / 1000.0
                && darkness_after(overlay, elapsed) < overlay.blink_end_threshold;
            if finished {
                self.started_at = None;
            }
        }
    }

    pub(crate) fn draw(&self, ctx: &Ctx<'_>, layers: &mut DrawLayers) -> Result<(), WidgetError> {
        let darkness = self.darkness(ctx);
        if darkness <= 0.0 {
            return Ok(());
        }
        let opacity = Opacity::new(darkness.clamp(0.0, 1.0) as f32)
            .map_err(|_| WidgetError::Inconsistent("глубина затемнения вне 0..1".to_owned()))?;
        let frame = ctx.geometry.frame(ctx.state.settings().aspect);
        layers.push(
            Layer::FrameOverlay,
            rect_item(frame, DisplayColor::BLACK, opacity, ctx.px),
        );
        Ok(())
    }
}

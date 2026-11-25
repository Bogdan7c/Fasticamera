//! Линейка Exposure внутри нижней карточки (решение 103): горизонтальная шкала (K3 `SnapScale`, шаг 1/3 EV,
//! плотность `ScaleDensity::Ruler` — 20 pt на шаг) с жёлтой отметкой и риской поярче на каждом целом EV.
//!
//! Линейка — часть **одной** карточки инструментов (`tool_card.rs`), а не отдельная панель: карточка владеет
//! рамкой, стеклом, заголовком, значением и кнопками, линейка — только зоной пана и рисками. Значение
//! принадлежит `ScreenState`, шкала отвечает за физику (см. `exposure_link.rs`).

use std::num::NonZeroU32;

use ui_core::gesture::{GestureEvent, PanDirection, PanSpec, RecognizerSpec};
use ui_core::id::NodeId;
use ui_draw::scene_bridge::NodePlacementPx;
use ui_draw::transform::Affine2;
use ui_kit::pan::PanUpdate;
use ui_kit::ruler::{RulerControl, RulerFeel, RulerSpec};
use ui_kit::tokens::ScaleDensity;

use super::error::{WidgetError, element};
use super::exposure_link::{FingerDrive, exposure_model, linear_limits, state_value_ev};
use super::intent::ScreenGesture;
use super::kit::SceneKit;
use super::layers::{DrawLayers, Layer, VisualIndex};
use super::{Ctx, Outputs};
use ui_core::geometry::Rect;

pub(crate) struct ExposureRuler {
    /// Узел зоны пана; его размещение отдаёт виду центр и протяжённость.
    node: NodeId,
    /// Линейка-ручка `ui-kit` (общая с ручками раздела калибровки): физика, затвор пальца и вид рисок.
    ruler: RulerControl,
    /// Перевод значений линейки в действия EV и счёт значений, не ставших компенсацией.
    drive: FingerDrive,
}

impl ExposureRuler {
    /// Зона пана — ребёнок узла карточки; пока карточка Exposure не открыта, зона не ловит касаний.
    pub(crate) fn build(
        kit: &mut SceneKit,
        ctx: &Ctx<'_>,
        card_node: NodeId,
        card_rect: Rect,
    ) -> Result<Self, WidgetError> {
        let rect = ctx.geometry.card_ruler();
        let node = kit.add_node(card_node, "card_ruler", rect, card_rect)?;
        kit.add_recognizer(
            node,
            RecognizerSpec::Pan(PanSpec {
                direction: PanDirection::Horizontal,
                ..PanSpec::default()
            }),
            ScreenGesture::ExposureSliderPan,
        )?;
        kit.set_interactive(node, false)?;
        let ruler = RulerControl::new(
            RulerSpec {
                model: exposure_model(ctx)?,
                density: ScaleDensity::Ruler,
                major_every_steps: Self::major_tick_period(ctx)?,
                feel: RulerFeel::default(),
            },
            state_value_ev(ctx),
            &ctx.environment(rect.size.width),
            ctx.now,
        )
        .map_err(element("линейка EV"))?;
        Ok(Self {
            node,
            ruler,
            drive: FingerDrive::default(),
        })
    }

    /// Нужна ли линейке реакция на касания (открыта именно карточка Exposure).
    pub(crate) fn set_active(&mut self, kit: &mut SceneKit, active: bool) -> Result<(), WidgetError> {
        kit.set_interactive(self.node, active)
    }

    pub(crate) fn handle_gesture(&mut self, event: &GestureEvent<ScreenGesture>) -> Result<bool, WidgetError> {
        if event.intent != ScreenGesture::ExposureSliderPan {
            return Ok(false);
        }
        if let Some(update) = PanUpdate::from_gesture(event.state, event.data).map_err(element("пан EV"))? {
            self.ruler
                .apply_pan(event.time, update)
                .map_err(element("линейка EV"))?;
        }
        Ok(true)
    }

    /// Выравнивает линейку по состоянию: пределы камеры и значение (без эха и хаптики).
    pub(crate) fn sync(&mut self, ctx: &Ctx<'_>) -> Result<(), WidgetError> {
        // Пределы камеры изменились: шкала переезжает на новые (значение шкалы остаётся).
        self.ruler
            .retarget_limits(linear_limits(ctx), ctx.now)
            .map_err(element("линейка EV"))?;
        // Шкала разошлась с состоянием, а палец её не держит: едем к состоянию (допуск — доля шага из конфига).
        let tolerance = f64::from(ctx.config.exposure.step_ev) * ctx.geometry.config().card.state_match_tolerance_steps;
        self.ruler
            .follow_grid_value(state_value_ev(ctx), tolerance, ctx.now)
            .map_err(element("линейка EV"))?;
        Ok(())
    }

    pub(crate) fn advance(&mut self, ctx: &Ctx<'_>, out: &mut Outputs) {
        let routed = self.ruler.advance(ctx.now);
        self.drive.apply_routed(routed, ctx, out);
    }

    /// Риски и жёлтая отметка. `content_alpha` и `slide_px` — прозрачность и сдвиг содержимого карточки.
    pub(crate) fn draw(
        &self,
        ctx: &Ctx<'_>,
        visuals: &VisualIndex,
        layers: &mut DrawLayers,
        content_alpha: f32,
        slide_px: f32,
    ) -> Result<(), WidgetError> {
        let Some(node_placement) = visuals.placement(self.node) else {
            return Ok(());
        };
        let placement = NodePlacementPx {
            transform: Affine2::translate(0.0, slide_px).then(node_placement.transform),
            opacity: (node_placement.opacity * content_alpha).clamp(0.0, 1.0),
            ..node_placement
        };
        layers.extend(
            Layer::Content,
            self.ruler
                .items(&placement, ctx.now, ctx.px, &ctx.tokens.ruler, &ctx.tokens.colors)?,
        );
        Ok(())
    }

    /// Крупная риска — каждый целый EV: сколько шагов шкалы в одном EV, считается из шага конфига (1/3 → 3).
    fn major_tick_period(ctx: &Ctx<'_>) -> Result<NonZeroU32, WidgetError> {
        let steps_per_whole_ev = (1.0 / f64::from(ctx.config.exposure.step_ev)).round();
        let steps = u32::try_from(steps_per_whole_ev as i64)
            .ok()
            .and_then(NonZeroU32::new)
            .ok_or_else(|| {
                WidgetError::Inconsistent(format!(
                    "шаг EV {} не даёт целого числа шагов на 1 EV",
                    ctx.config.exposure.step_ev
                ))
            })?;
        Ok(steps)
    }

    #[cfg(test)]
    pub(crate) fn value_ev(&self, now: ui_core::time::MonotonicNanos) -> f64 {
        self.ruler.value(now)
    }

    pub(crate) fn invalid_bias_values(&self) -> u64 {
        self.drive.invalid_bias_values()
    }

    pub(crate) fn suppressed_echo_events(&self) -> u64 {
        self.ruler.suppressed_echo_events()
    }
}

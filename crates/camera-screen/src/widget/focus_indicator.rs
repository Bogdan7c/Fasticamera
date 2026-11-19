//! Рамка фокуса с «солнцем» экспозиции (K9 `FocusFrame`). Положение и срок жизни — из `ScreenState`
//! (`focus().indicator()`, один владелец срока — состояние экрана): рамка создаётся с `UntilReleased`, а
//! `dismiss` вызывается, когда индикатор стал `Hidden`. Стадию «потускнения» по таймеру `ui-kit` не используем.
//!
//! Вертикальный пан «солнца» двигает EV: `ValueChanged` пальца → `ExposureDragged` (квантует редьюсер),
//! хаптика тиков идёт из события шкалы. Узел-«солнце» — ребёнок видоискателя, поэтому выигрывает у пана вверх
//! (меню) и у тапа (`behavior.md` 1.2).

use ui_core::geometry::{Point, Rect, Size};
use ui_core::gesture::{GestureEvent, PanDirection, PanSpec, RecognizerSpec};
use ui_core::id::NodeId;
use ui_core::time::MonotonicNanos;
use ui_draw::geometry::PxSize;
use ui_draw::scene_bridge::NodePlacementPx;
use ui_draw::transform::Affine2;
use ui_kit::drive::{GrowthDirection, ScrollTransition};
use ui_kit::focus_frame::{FocusFrame, FocusFrameEvent, FocusFrameSpec, FocusHold, FocusPhase};
use ui_kit::pan::PanUpdate;
use ui_kit::scale::{ScaleSpec, TickVisibility};
use ui_kit::tokens::ScaleDensity;
use ui_kit::value::{QuantizedLinear, ValueModel};
use ui_kit::view::focus_frame_items;

use super::error::{WidgetError, element};
use super::exposure_link::{FingerDrive, differs_from_state, exposure_model, linear_limits, state_value_ev};
use super::intent::ScreenGesture;
use super::kit::SceneKit;
use super::layers::{DrawLayers, Layer};
use super::{Ctx, Outputs};
use crate::focus::FocusIndicator;
use crate::state::PanelState;

pub(crate) struct FocusIndicatorWidget {
    sun_node: NodeId,
    frame: FocusFrame<QuantizedLinear>,
    finger: FingerDrive,
    /// Где рамка стоит сейчас (`None` — скрыта).
    placed_at: Option<Point>,
    /// Метка показа индикатора (`since`), по которой рамка поставлена. Новый тап (в том числе в ту же точку)
    /// даёт новую метку, и рамка «пульсирует» заново, как в iOS.
    placed_since: Option<MonotonicNanos>,
    /// Шаги EV, замеченные прошлой синхронизацией. Метку показа двигает и продление срока при перетаскивании
    /// «солнца» (`keep_alive`), но оно всегда сопровождается сменой шагов: такая смена метки новым тапом не
    /// считается. Чистое решение — отдельный номер показа в `FocusIndicator` (ядро `reduce`, не эта зона).
    seen_steps: i32,
    /// Пан «солнца» пришёл, когда рамки уже нет (узел успел перестать быть доступным): считаем, не глотаем.
    ignored_sun_pans: u64,
}

impl FocusIndicatorWidget {
    fn new_frame(ctx: &Ctx<'_>) -> Result<FocusFrame<QuantizedLinear>, WidgetError> {
        let spec = FocusFrameSpec {
            sun: ScaleSpec {
                direction: GrowthDirection::FingerUp,
                density: ScaleDensity::Dense,
                ticks: TickVisibility::Hidden,
                ..ScaleSpec::default()
            },
            hold: FocusHold::UntilReleased,
        };
        FocusFrame::new(
            exposure_model(ctx)?,
            state_value_ev(ctx),
            spec,
            &ctx.environment(ctx.geometry.config().focus.sun_scale_extent_pt),
            ctx.now,
        )
        .map_err(element("рамка фокуса"))
    }

    pub(crate) fn build(kit: &mut SceneKit, ctx: &Ctx<'_>, viewfinder: NodeId) -> Result<Self, WidgetError> {
        let screen = ctx.geometry.screen();
        let sun_node = kit.add_node(
            viewfinder,
            "focus_sun",
            Self::sun_rect(ctx, Point::new(0.0, 0.0)),
            screen,
        )?;
        kit.add_recognizer(
            sun_node,
            RecognizerSpec::Pan(PanSpec {
                direction: PanDirection::Vertical,
                ..PanSpec::default()
            }),
            ScreenGesture::SunPan,
        )?;
        kit.set_interactive(sun_node, false)?;
        Ok(Self {
            sun_node,
            frame: Self::new_frame(ctx)?,
            finger: FingerDrive::default(),
            placed_at: None,
            placed_since: None,
            seen_steps: ctx.state.exposure().steps(),
            ignored_sun_pans: 0,
        })
    }

    /// Зона касания «солнца» при рамке с центром `center`: полоса справа от рамки, центр — на месте «солнца».
    fn sun_rect(ctx: &Ctx<'_>, center: Point) -> Rect {
        let focus = &ctx.tokens.focus;
        let layout = &ctx.geometry.config().focus;
        let sun_center = Point::new(
            center.x + focus.frame_width_pt * 0.5 + focus.sun_offset_right_of_frame_pt,
            center.y - focus.sun_offset_above_center_pt,
        );
        Rect::centered_at(
            sun_center,
            Size::new(layout.sun_touch_width_pt, layout.sun_touch_height_pt),
        )
    }

    pub(crate) fn handle_gesture(&mut self, event: &GestureEvent<ScreenGesture>) -> Result<bool, WidgetError> {
        if event.intent != ScreenGesture::SunPan {
            return Ok(false);
        }
        if self.frame.phase() == FocusPhase::Hidden {
            self.ignored_sun_pans += 1;
            return Ok(true);
        }
        if let Some(update) = PanUpdate::from_gesture(event.state, event.data).map_err(element("пан «солнца»"))?
        {
            if matches!(update, PanUpdate::Began { .. }) {
                self.finger.finger_took_over();
            }
            self.frame
                .apply_pan(event.time, update)
                .map_err(element("рамка фокуса"))?;
        }
        Ok(true)
    }

    pub(crate) fn sync(&mut self, kit: &mut SceneKit, ctx: &Ctx<'_>) -> Result<(), WidgetError> {
        // Пределы камеры изменились: «солнце» переезжает на новые пределы, рамка остаётся на месте
        // (положение, фаза и срок жизни не трогаются), значение вне пределов сжимается до крайнего.
        if self.frame.sun().model().limits() != &linear_limits(ctx) {
            self.frame
                .set_sun_limits(linear_limits(ctx), ScrollTransition::Animated, ctx.now)
                .map_err(element("рамка фокуса"))?;
            self.finger.program_took_over();
        }
        let steps = ctx.state.exposure().steps();
        let exposure_changed = steps != self.seen_steps;
        self.seen_steps = steps;
        match ctx.state.focus().indicator() {
            FocusIndicator::Shown { at, since, .. } => {
                let moved = self.placed_at != Some(at);
                // Метка сменилась без смены EV — это новый тап/долгий тап, пусть и в ту же точку.
                let shown_again = self.placed_since != Some(since) && !exposure_changed;
                if moved || shown_again {
                    self.frame.place(at, ctx.now).map_err(element("рамка фокуса"))?;
                }
                if moved {
                    kit.move_node(self.sun_node, Self::sun_rect(ctx, at), ctx.geometry.screen())?;
                    self.placed_at = Some(at);
                }
                self.placed_since = Some(since);
                kit.set_interactive(self.sun_node, ctx.state.panel() == PanelState::Closed)?;
            }
            FocusIndicator::Hidden => {
                self.placed_since = None;
                if self.placed_at.take().is_some() {
                    self.frame.dismiss(ctx.now).map_err(element("рамка фокуса"))?;
                }
                kit.set_interactive(self.sun_node, false)?;
            }
        }
        if self.placed_at.is_some()
            && !self.finger.is_finger_driven()
            && differs_from_state(ctx, self.frame.sun().value(ctx.now))
        {
            self.finger.program_took_over();
            self.frame
                .set_sun_value(state_value_ev(ctx), ScrollTransition::Animated, ctx.now)
                .map_err(element("рамка фокуса"))?;
        }
        Ok(())
    }

    pub(crate) fn advance(&mut self, ctx: &Ctx<'_>, out: &mut Outputs) -> Result<(), WidgetError> {
        self.frame.advance(ctx.now).map_err(element("рамка фокуса"))?;
        for event in self.frame.drain_events() {
            if let FocusFrameEvent::Sun(scale_event) = event {
                self.finger.translate(vec![scale_event], ctx, out);
            }
        }
        Ok(())
    }

    pub(crate) fn draw(&self, ctx: &Ctx<'_>, layers: &mut DrawLayers) -> Result<(), WidgetError> {
        let snapshot = self.frame.snapshot(ctx.now);
        if snapshot.phase == FocusPhase::Hidden {
            return Ok(());
        }
        let focus = &ctx.tokens.focus;
        let placement = NodePlacementPx {
            size: PxSize::new(ctx.px.px(focus.frame_width_pt), ctx.px.px(focus.frame_height_pt)),
            transform: Affine2::translate(ctx.px.px(snapshot.center.x), ctx.px.px(snapshot.center.y)),
            opacity: 1.0,
        };
        layers.extend(
            Layer::Plain,
            focus_frame_items(&placement, &snapshot, ctx.px, focus, &ctx.tokens.colors)?,
        );
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn sun_value_ev(&self, now: ui_core::time::MonotonicNanos) -> f64 {
        self.frame.sun().value(now)
    }

    pub(crate) fn suppressed_echo_events(&self) -> u64 {
        self.finger.suppressed_echo_events()
    }

    pub(crate) fn invalid_bias_values(&self) -> u64 {
        self.finger.invalid_bias_values()
    }

    /// Масштаб появления рамки сейчас (проба для тестов повторного тапа).
    #[cfg(test)]
    pub(crate) fn appearance_scale(&self, now: MonotonicNanos) -> f64 {
        self.frame.snapshot(now).scale
    }

    pub(crate) fn ignored_sun_pans(&self) -> u64 {
        self.ignored_sun_pans
    }
}

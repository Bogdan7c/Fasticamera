//! Затвор (K10): стеклянное кольцо и белый диск. Вид — по `ShutterState`; при открытом меню Controls затвор
//! растворяется под шторкой и **не нажимается** (решение 94).

use ui_core::geometry::Rect;
use ui_core::gesture::{GestureEvent, PressSpec, RecognizerSpec, TapSpec};
use ui_core::id::NodeId;
use ui_kit::button::PressOutcome;
use ui_kit::shutter::{ShutterButton, ShutterLook};
use ui_kit::tokens::{GlassRole, MotionToken};
use ui_kit::view::shutter_items;

use super::error::{WidgetError, element};
use super::intent::{ButtonId, ScreenGesture};
use super::kit::SceneKit;
use super::layers::{DrawLayers, Layer, VisualIndex};
use super::{Ctx, Outputs};
use crate::action::ScreenAction;
use crate::shutter::ShutterState;
use crate::state::ShutterVisibility;

/// Ниже этой прозрачности затвор в список не попадает (полностью ушёл под шторку).
const INVISIBLE_BELOW: f32 = 0.004;

pub(crate) struct ShutterWidget {
    node: NodeId,
    button: ShutterButton,
    /// К какой видимости едем сейчас (`None` — ещё не назначали).
    shown: Option<bool>,
}

impl ShutterWidget {
    pub(crate) fn build(kit: &mut SceneKit, ctx: &Ctx<'_>, parent: NodeId) -> Result<Self, WidgetError> {
        let frame: Rect = ctx.geometry.shutter();
        let node = kit.add_node(parent, "shutter", frame, ctx.geometry.screen())?;
        kit.add_recognizer(
            node,
            RecognizerSpec::Tap(TapSpec::default()),
            ScreenGesture::ButtonTapGuard(ButtonId::Shutter),
        )?;
        kit.add_recognizer(
            node,
            RecognizerSpec::Press(PressSpec::default()),
            ScreenGesture::Press(ButtonId::Shutter),
        )?;
        let environment = ctx.environment(frame.size.height);
        let button = ShutterButton::new(frame, ShutterLook::Ready, &environment, ctx.now).map_err(element("затвор"))?;
        Ok(Self {
            node,
            button,
            shown: None,
        })
    }

    pub(crate) fn handle_gesture(
        &mut self,
        event: &GestureEvent<ScreenGesture>,
        out: &mut Outputs,
    ) -> Result<bool, WidgetError> {
        if event.intent != ScreenGesture::Press(ButtonId::Shutter) {
            return Ok(false);
        }
        let outcome = self
            .button
            .handle_press(event.state, event.data, event.time)
            .map_err(element("затвор"))?;
        if outcome == PressOutcome::Activated {
            out.actions.push(ScreenAction::ShutterTapped { at: event.time });
        }
        Ok(true)
    }

    fn look_for(ctx: &Ctx<'_>) -> ShutterLook {
        if ctx.state.is_camera_unavailable() {
            return ShutterLook::Disabled;
        }
        match ctx.state.shutter() {
            ShutterState::Idle | ShutterState::Countdown { .. } => ShutterLook::Ready,
            _ => ShutterLook::Busy,
        }
    }

    pub(crate) fn sync(&mut self, kit: &mut SceneKit, ctx: &Ctx<'_>) -> Result<(), WidgetError> {
        self.button
            .set_look(Self::look_for(ctx), ctx.now)
            .map_err(element("затвор"))?;
        let visible = ctx.state.shutter_visibility() == ShutterVisibility::Visible;
        if self.shown != Some(visible) {
            self.shown = Some(visible);
            // Нажиматься перестаёт сразу, растворяется пружиной.
            kit.set_interactive(self.node, visible)?;
            kit.fade_to(
                self.node,
                if visible { 1.0 } else { 0.0 },
                ctx.spring(MotionToken::Snappy)?,
                ctx.now,
            )?;
        }
        Ok(())
    }

    pub(crate) fn draw(
        &self,
        ctx: &Ctx<'_>,
        visuals: &VisualIndex,
        layers: &mut DrawLayers,
    ) -> Result<(), WidgetError> {
        let Some(placement) = visuals.placement(self.node) else {
            return Ok(());
        };
        if placement.opacity < INVISIBLE_BELOW {
            return Ok(());
        }
        let snapshot = self.button.snapshot(ctx.now);
        let items = shutter_items(
            &placement,
            &snapshot,
            ctx.tokens.glass.style_for(GlassRole::Button),
            ctx.px,
            &ctx.tokens.shutter,
            &ctx.tokens.colors,
        )?;
        // Первый элемент — стеклянное кольцо, остальное (диск) — содержимое поверх.
        let mut items = items.into_iter();
        if let Some(ring) = items.next() {
            layers.push(Layer::Glass, ring);
        }
        layers.extend(Layer::Content, items);
        Ok(())
    }
}

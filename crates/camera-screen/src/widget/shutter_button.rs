//! Затвор (K10): стеклянное кольцо и белый диск. Вид — по `ShutterState`; при открытом меню Controls затвор
//! растворяется под шторкой и **не нажимается** (решение 94). Пока открыта нижняя карточка, ряд затвора поднят
//! над ней: затвор виден и снимает на поднятом месте (решение 103).
//!
//! # Подъём
//! Рамка узла не меняется — меняется визуальный сдвиг `TranslationY` (пружина `Smooth`, без перелёта).
//! `ui-core` учитывает сдвиг узла в хит-тесте, поэтому зона касания едет вместе с картинкой. Рамка трекера
//! нажатия (`PressTracker`) хранится в абсолютных pt отдельно от сцены, поэтому на каждом `sync` её надо
//! переставлять вслед за сдвигом, иначе нажатие на поднятом месте не дойдёт до `Activated`.

use ui_core::geometry::Rect;
use ui_core::gesture::{GestureEvent, PressSpec, RecognizerSpec, TapSpec};
use ui_core::id::NodeId;
use ui_core::time::MotionPreference;
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
    /// К какому положению едем сейчас (`None` — ещё не назначали).
    placement: Option<ShutterVisibility>,
    /// Рамка раскладки (без сдвига): от неё считается рамка трекера нажатия.
    base_frame: Rect,
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
            placement: None,
            base_frame: frame,
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
        let visibility = ctx.state.shutter_visibility();
        if self.placement != Some(visibility) {
            self.placement = Some(visibility);
            let visible = visibility != ShutterVisibility::HiddenUnderDrawer;
            // Нажиматься перестаёт сразу, растворяется пружиной.
            kit.set_interactive(self.node, visible)?;
            kit.fade_to(
                self.node,
                if visible { 1.0 } else { 0.0 },
                ctx.spring(MotionToken::Snappy)?,
                ctx.now,
            )?;
            let raised = visibility == ShutterVisibility::RaisedAboveCard;
            let offset_pt = if raised {
                -ctx.geometry.config().card.raised_shutter_offset_pt
            } else {
                0.0
            };
            if ctx.motion == MotionPreference::Reduced {
                // Reduce Motion: без перемещений, затвор сразу на новом месте (прозрачность по-прежнему плавная).
                kit.jump_vertically_to(self.node, offset_pt)?;
            } else {
                // Подъём и возврат затвора: без перелёта, ≈ 0,3 с, как морф карточки `[measured: C t=54.0]`.
                kit.slide_vertically_to(self.node, offset_pt, ctx.spring(MotionToken::CardMorph)?, ctx.now)?;
            }
        }
        // Зона нажатия следует за картинкой, пока та едет (см. заголовок модуля).
        let offset_pt = kit.vertical_offset_pt(self.node)?;
        self.button
            .set_frame(Rect::from_xywh(
                self.base_frame.min_x(),
                self.base_frame.min_y() + offset_pt,
                self.base_frame.size.width,
                self.base_frame.size.height,
            ))
            .map_err(element("затвор"))?;
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

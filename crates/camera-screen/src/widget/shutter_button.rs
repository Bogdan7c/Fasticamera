//! Затвор (K10): стеклянное кольцо и белый диск. Вид — по `ShutterState`; при открытом меню Controls затвор
//! растворяется под шторкой и **не нажимается** (решение 94). Пока открыта нижняя карточка, ряд затвора поднят
//! над ней: затвор виден и снимает на поднятом месте (решение 103).
//!
//! # Подъём
//! Видимость и подъём ведёт [`ShutterRowPose`] (`shutter_row.rs`): ту же позу, тем же кадром и той же пружиной
//! получает кнопка белого. Рамка трекера нажатия (`PressTracker`) хранится в абсолютных pt отдельно от сцены,
//! поэтому на каждом `sync` её надо переставлять вслед за сдвигом, иначе нажатие на поднятом месте не дойдёт до
//! `Activated`.

use ui_core::geometry::Rect;
use ui_core::gesture::{GestureEvent, PressSpec, RecognizerSpec, TapSpec};
use ui_core::id::NodeId;
use ui_kit::button::PressOutcome;
use ui_kit::shutter::{ShutterButton, ShutterLook};
use ui_kit::view::shutter_items;

use super::error::{WidgetError, element};
use super::intent::{ButtonId, ScreenGesture};
use super::kit::SceneKit;
use super::layers::{DrawLayers, Layer, VisualIndex};
use super::shutter_row::ShutterRowPose;
use super::{Ctx, Outputs};
use crate::action::ScreenAction;
use crate::shutter::ShutterState;

/// Ниже этой прозрачности затвор в список не попадает (полностью ушёл под шторку).
const INVISIBLE_BELOW: f32 = 0.004;

pub(crate) struct ShutterWidget {
    node: NodeId,
    button: ShutterButton,
    /// Видимость и подъём: общие с кнопкой белого (`shutter_row.rs`).
    pose: ShutterRowPose,
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
            pose: ShutterRowPose::default(),
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
        let offset_pt = self.pose.sync(kit, ctx, self.node)?;
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
        // Затвор без подложки (решение 300): кольцо и диск — содержимое, стекла под ними нет. Кольцо при
        // нажатии растёт на прирост кнопок — как раньше росло стеклянное кольцо.
        let items = shutter_items(
            &placement,
            &snapshot,
            &ctx.tokens.glass.button.press,
            ctx.px,
            &ctx.tokens.shutter,
            &ctx.tokens.colors,
        )?;
        layers.extend(Layer::Content, items);
        Ok(())
    }
}

//! Кнопка меню Controls (Ø48,3, иконка «точки»): стеклянный круг, из которого вырастает шторка меню.

use super::rotation_only;
use ui_core::gesture::GestureEvent;
use ui_core::id::NodeId;
use ui_draw::icons::IconId;
use ui_kit::tokens::GlassRole;
use ui_kit::view::{GlassShape, centered_icon, opacity_of_placement};

use super::button::{ButtonEvent, PressButton};
use super::error::WidgetError;
use super::intent::{ButtonId, ScreenGesture};
use super::kit::SceneKit;
use super::layers::{DrawLayers, Layer, VisualIndex};
use super::{Ctx, Outputs, single_member_group};
use crate::action::ScreenAction;
use crate::state::PanelState;
use ui_draw::draw_list::DrawItem;
use ui_draw::scene_bridge::NodePlacementPx;

pub(crate) struct MenuButtonWidget {
    button: PressButton,
    /// Узел значка: поворачивается по ориентации телефона, сама кнопка стоит.
    icon_node: NodeId,
}

impl MenuButtonWidget {
    pub(crate) fn build(kit: &mut SceneKit, ctx: &Ctx<'_>, parent: NodeId) -> Result<Self, WidgetError> {
        let frame = ctx.geometry.menu_button();
        let node = kit.add_node(parent, "menu_button", frame, ctx.geometry.screen())?;
        let button = PressButton::attach(kit, ButtonId::MenuButton, node, frame, ctx)?;
        let icon_node = kit.add_rotating_content(node, "menu_button_icon")?;
        Ok(Self { button, icon_node })
    }

    pub(crate) fn handle_gesture(
        &mut self,
        event: &GestureEvent<ScreenGesture>,
        out: &mut Outputs,
    ) -> Result<bool, WidgetError> {
        if event.intent != ScreenGesture::Press(ButtonId::MenuButton) {
            return Ok(false);
        }
        if self.button.handle(event.state, event.data, event.time)? == ButtonEvent::Activated {
            out.actions.push(ScreenAction::ControlsMenuOpenRequested);
        }
        Ok(true)
    }

    /// Кнопка ловит касания, только пока меню закрыто (открытое меню лежит поверх неё).
    pub(crate) fn sync(&mut self, kit: &mut SceneKit, ctx: &Ctx<'_>) -> Result<(), WidgetError> {
        let reachable = !matches!(ctx.state.panel(), PanelState::ControlsMenu | PanelState::Submenu(_));
        kit.set_interactive(self.button.node(), reachable)
    }

    /// `source_opacity` — прозрачность «источника» из морфа шторки: кнопка растворяется, пока из неё растёт панель.
    pub(crate) fn draw(
        &self,
        ctx: &Ctx<'_>,
        visuals: &VisualIndex,
        layers: &mut DrawLayers,
        source_opacity: f32,
    ) -> Result<(), WidgetError> {
        let Some(mut placement) = visuals.placement(self.button.node()) else {
            return Ok(());
        };
        placement.opacity *= source_opacity;
        if placement.opacity <= 0.0 {
            return Ok(());
        }
        let (group, host) = single_member_group(
            ctx,
            GlassRole::Button,
            GlassShape::Circle,
            &placement,
            self.button.press_state(ctx.now),
            "menu_button",
        )?;
        layers.push(Layer::Glass, DrawItem::Glass(group));
        if let Some(icon_placement) = visuals.placement(self.icon_node) {
            let icon_placement = NodePlacementPx {
                opacity: icon_placement.opacity * source_opacity,
                ..icon_placement
            };
            // Содержимое: хост — члену группы (масштаб нажатия) с поворотом узла значка поверх.
            let host = rotation_only(&icon_placement, &placement, host);
            layers.push(
                Layer::Content,
                centered_icon(
                    IconId::ControlsMenu,
                    ctx.geometry.config().bottom.button_icon_size_pt,
                    ctx.px,
                    host,
                    ctx.tokens.colors.text_primary,
                    opacity_of_placement(&icon_placement, "menu_button_icon")?,
                ),
            );
        }
        Ok(())
    }
}

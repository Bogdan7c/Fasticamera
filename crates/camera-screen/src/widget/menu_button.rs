//! Кнопка меню Controls (Ø44, иконка «точки»): стеклянный круг, из которого вырастает шторка меню.
//!
//! Кнопка живёт в двух местах ([`MenuButtonHome`]): на экране PHOTO и в нижнем ряду раздела калибровки (решение
//! 199: та же кнопка Ø44 справа внизу). В разделе она стоит в повёрнутом корне, значок не крутится сам, и кнопка
//! открывает меню раздела тем же действием `ControlsMenuOpenRequested` (какое меню — решает редьюсер по режиму).

use super::rotation_only;
use ui_core::geometry::Rect;
use ui_core::gesture::GestureEvent;
use ui_core::id::NodeId;
use ui_draw::icons::IconId;
use ui_kit::tokens::PanelMaterial;
use ui_kit::view::{GlassShape, centered_icon, opacity_of_placement};

use super::bottom_row::BottomRowFade;
use super::button::{ButtonEvent, PressButton};
use super::error::WidgetError;
use super::intent::{ButtonId, ScreenGesture};
use super::kit::SceneKit;
use super::layers::{DrawLayers, Layer, VisualIndex};
use super::{Ctx, Outputs, single_member_material};
use crate::action::ScreenAction;
use crate::state::PanelState;
use ui_draw::scene_bridge::NodePlacementPx;

/// Где стоит кнопка меню.
#[derive(Debug, Clone, Copy)]
pub(crate) enum MenuButtonHome {
    /// Нижний ряд экрана PHOTO.
    Screen,
    /// Нижний ряд раздела калибровки; рамка кнопки в системе корня раздела.
    Section { frame: Rect },
}

pub(crate) struct MenuButtonWidget {
    home: MenuButtonHome,
    button: PressButton,
    /// Узел значка: поворачивается по ориентации телефона, сама кнопка стоит.
    icon_node: NodeId,
    /// Растворение, пока открыта нижняя карточка.
    fade: BottomRowFade,
}

impl MenuButtonWidget {
    pub(crate) fn build(kit: &mut SceneKit, ctx: &Ctx<'_>, parent: NodeId) -> Result<Self, WidgetError> {
        Self::build_at(kit, ctx, (parent, ctx.geometry.screen()), MenuButtonHome::Screen)
    }

    /// Кнопка в `home`; `(parent, parent_rect)` — узел-родитель и его рамка.
    pub(crate) fn build_at(
        kit: &mut SceneKit,
        ctx: &Ctx<'_>,
        (parent, parent_rect): (NodeId, Rect),
        home: MenuButtonHome,
    ) -> Result<Self, WidgetError> {
        let (frame, id) = match home {
            MenuButtonHome::Screen => (ctx.geometry.menu_button(), ButtonId::MenuButton),
            MenuButtonHome::Section { frame } => (frame, ButtonId::CalibrationMenu),
        };
        let node = kit.add_node(parent, "menu_button", frame, parent_rect)?;
        let button = PressButton::attach(kit, id, node, frame, ctx)?;
        let icon_node = match home {
            MenuButtonHome::Screen => kit.add_rotating_content(node, "menu_button_icon")?,
            MenuButtonHome::Section { .. } => kit.add_fixed_content(node, "menu_button_icon")?,
        };
        Ok(Self {
            home,
            button,
            icon_node,
            fade: BottomRowFade::default(),
        })
    }

    fn button_id(&self) -> ButtonId {
        match self.home {
            MenuButtonHome::Screen => ButtonId::MenuButton,
            MenuButtonHome::Section { .. } => ButtonId::CalibrationMenu,
        }
    }

    /// `kit` нужен дому раздела: положение пальца он переводит в систему рамки кнопки (корень повёрнут).
    pub(crate) fn handle_gesture(
        &mut self,
        kit: &SceneKit,
        event: &GestureEvent<ScreenGesture>,
        out: &mut Outputs,
    ) -> Result<bool, WidgetError> {
        if event.intent != ScreenGesture::Press(self.button_id()) {
            return Ok(false);
        }
        let data = match self.home {
            MenuButtonHome::Screen => event.data,
            MenuButtonHome::Section { .. } => kit.gesture_data_in_node(self.button.node(), event.data),
        };
        if self.button.handle(event.state, data, event.time)? == ButtonEvent::Activated {
            out.actions.push(ScreenAction::ControlsMenuOpenRequested);
        }
        Ok(true)
    }

    /// Кнопка ловит касания, только пока меню закрыто (открытое меню лежит поверх неё) и нет нижней карточки
    /// (она растворяет кнопку, решение 103).
    pub(crate) fn sync(&mut self, kit: &mut SceneKit, ctx: &Ctx<'_>) -> Result<(), WidgetError> {
        let (row_shown, menu_closed) = match self.home {
            MenuButtonHome::Screen => (
                self.fade.sync(kit, ctx, self.button.node())?,
                ctx.state.panel() != PanelState::ControlsMenu,
            ),
            // Внутри раздела нижний ряд карточка не растворяет (у меню раздела своя шторка и карточка, и они лежат
            // поверх ряда целиком); кнопку прячет открытое меню раздела.
            MenuButtonHome::Section { .. } => (
                true,
                ctx.state.calibration().menu() == crate::calibration::CalibrationMenu::Closed,
            ),
        };
        kit.set_interactive(self.button.node(), row_shown && menu_closed)
    }

    /// `source_opacity` — прозрачность «источника» из морфа шторки: кнопка растворяется, пока из неё растёт панель.
    pub(crate) fn draw(
        &self,
        ctx: &Ctx<'_>,
        visuals: &VisualIndex,
        layers: &mut DrawLayers,
        source_opacity: f32,
        material: PanelMaterial,
    ) -> Result<(), WidgetError> {
        let Some(mut placement) = visuals.placement(self.button.node()) else {
            return Ok(());
        };
        placement.opacity *= source_opacity;
        if placement.opacity <= 0.0 {
            return Ok(());
        }
        let (item, host) = single_member_material(
            ctx,
            material,
            GlassShape::Circle,
            &placement,
            self.button.press_state(ctx.now),
            "menu_button",
        )?;
        layers.push(Layer::Glass, item);
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

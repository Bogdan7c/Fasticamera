//! Кнопка переворота камеры: круг Ø 48 в (382; 898), заливка #1C1C1C без кольца
//! (`[measured: R00 t=7.5–10.8]`, `layout.md` 2.2); значок — свой, на основе Phosphor (решение 81).
//!
//! Тап даёт `FlipCameraRequested`; что сделать с направлением и коэффициентом, решает редьюсер. Кнопка входит в
//! нижний ряд и вместе с ним растворяется, пока открыта карточка (решение 103).

use ui_core::gesture::GestureEvent;
use ui_core::id::NodeId;
use ui_draw::color::DisplayColor;
use ui_draw::draw_list::{DrawItem, ShapeItem};
use ui_draw::icons::IconId;
use ui_draw::shape::ShapeGeometry;
use ui_kit::view::{centered_icon, opacity_of_placement};

use super::bottom_row::BottomRowFade;
use super::button::{ButtonEvent, PressButton};
use super::error::WidgetError;
use super::intent::{ButtonId, ScreenGesture};
use super::kit::SceneKit;
use super::layers::{DrawLayers, Layer, VisualIndex};
use super::{Ctx, Outputs};
use crate::action::ScreenAction;

pub(crate) struct FlipButtonWidget {
    button: PressButton,
    fade: BottomRowFade,
}

impl FlipButtonWidget {
    pub(crate) fn build(kit: &mut SceneKit, ctx: &Ctx<'_>, parent: NodeId) -> Result<Self, WidgetError> {
        let frame = ctx.geometry.flip_button();
        let node = kit.add_node(parent, "flip_button", frame, ctx.geometry.screen())?;
        Ok(Self {
            button: PressButton::attach(kit, ButtonId::FlipCamera, node, frame, ctx)?,
            fade: BottomRowFade::default(),
        })
    }

    pub(crate) fn sync(&mut self, kit: &mut SceneKit, ctx: &Ctx<'_>) -> Result<(), WidgetError> {
        let row_shown = self.fade.sync(kit, ctx, self.button.node())?;
        // Без камеры другого направления кнопки нет совсем: ни пикселей, ни зоны касания.
        kit.set_interactive(self.button.node(), row_shown && ctx.state.can_flip_camera())
    }

    pub(crate) fn handle_gesture(
        &mut self,
        event: &GestureEvent<ScreenGesture>,
        out: &mut Outputs,
    ) -> Result<bool, WidgetError> {
        if event.intent != ScreenGesture::Press(ButtonId::FlipCamera) {
            return Ok(false);
        }
        if self.button.handle(event.state, event.data, event.time)? == ButtonEvent::Activated {
            out.actions.push(ScreenAction::FlipCameraRequested { at: event.time });
        }
        Ok(true)
    }

    pub(crate) fn draw(
        &self,
        ctx: &Ctx<'_>,
        visuals: &VisualIndex,
        layers: &mut DrawLayers,
    ) -> Result<(), WidgetError> {
        if !ctx.state.can_flip_camera() {
            return Ok(());
        }
        let Some(placement) = visuals.placement(self.button.node()) else {
            return Ok(());
        };
        if placement.opacity <= 0.0 {
            return Ok(());
        }
        let zoom = ctx.geometry.zoom_config();
        let opacity = opacity_of_placement(&placement, "flip_button")?;
        let [red, green, blue] = zoom.flip_button_fill_rgb;
        // Плоский круг без стекла: у iOS переворот — тёмный диск без кольца.
        layers.push(
            Layer::Plain,
            DrawItem::Shape(ShapeItem {
                geometry: ShapeGeometry::Circle {
                    diameter: placement.size.width.min(placement.size.height),
                },
                transform: placement.transform,
                fill: DisplayColor::rgb(red, green, blue),
                stroke: None,
                shadow: None,
                opacity,
            }),
        );
        layers.push(
            Layer::Content,
            centered_icon(
                IconId::FlipCamera,
                zoom.flip_icon_size_pt,
                ctx.px,
                placement.transform,
                ctx.tokens.colors.text_primary,
                opacity,
            ),
        );
        Ok(())
    }
}

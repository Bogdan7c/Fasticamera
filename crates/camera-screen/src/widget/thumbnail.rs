//! Миниатюра последнего снимка: стеклянный круг Ø48, тап открывает системную галерею (`ThumbnailTapped`).

use ui_core::gesture::GestureEvent;
use ui_core::id::NodeId;
use ui_draw::draw_list::{DrawItem, ShapeItem};
use ui_draw::shape::ShapeGeometry;
use ui_kit::tokens::GlassRole;
use ui_kit::view::{GlassShape, opacity_of_placement};

use super::button::{ButtonEvent, PressButton};
use super::error::WidgetError;
use super::intent::{ButtonId, ScreenGesture};
use super::kit::SceneKit;
use super::layers::{DrawLayers, Layer, VisualIndex};
use super::{Ctx, Outputs, single_member_group};
use crate::action::ScreenAction;
use crate::shutter::ThumbnailState;

pub(crate) struct ThumbnailWidget {
    button: PressButton,
}

impl ThumbnailWidget {
    pub(crate) fn build(kit: &mut SceneKit, ctx: &Ctx<'_>, parent: NodeId) -> Result<Self, WidgetError> {
        let frame = ctx.geometry.thumbnail();
        let node = kit.add_node(parent, "thumbnail", frame, ctx.geometry.screen())?;
        Ok(Self {
            button: PressButton::attach(kit, ButtonId::Thumbnail, node, frame, ctx)?,
        })
    }

    pub(crate) fn handle_gesture(
        &mut self,
        event: &GestureEvent<ScreenGesture>,
        out: &mut Outputs,
    ) -> Result<bool, WidgetError> {
        if event.intent != ScreenGesture::Press(ButtonId::Thumbnail) {
            return Ok(false);
        }
        if self.button.handle(event.state, event.data, event.time)? == ButtonEvent::Activated {
            out.actions.push(ScreenAction::ThumbnailTapped);
        }
        Ok(true)
    }

    pub(crate) fn draw(
        &self,
        ctx: &Ctx<'_>,
        visuals: &VisualIndex,
        layers: &mut DrawLayers,
    ) -> Result<(), WidgetError> {
        let Some(placement) = visuals.placement(self.button.node()) else {
            return Ok(());
        };
        let (group, host) = single_member_group(
            ctx,
            GlassRole::Button,
            GlassShape::Circle,
            &placement,
            self.button.press_state(ctx.now),
            "thumbnail",
        )?;
        layers.push(Layer::Glass, DrawItem::Glass(group));
        // TODO(миниатюра): в списке рисования нет элемента «картинка», поэтому после снимка внутри круга
        // рисуется однотонный диск (признак «снимок есть»); настоящая миниатюра — вместе с сервисом съёмки.
        if let ThumbnailState::Last { .. } = ctx.state.thumbnail() {
            let diameter =
                placement.size.width.min(placement.size.height) * ctx.geometry.config().bottom.thumbnail_dot_scale;
            layers.push(
                Layer::Content,
                DrawItem::Shape(ShapeItem {
                    geometry: ShapeGeometry::Circle { diameter },
                    transform: host,
                    fill: ctx.tokens.colors.text_muted,
                    stroke: None,
                    shadow: None,
                    opacity: opacity_of_placement(&placement, "thumbnail")?,
                }),
            );
        }
        Ok(())
    }
}

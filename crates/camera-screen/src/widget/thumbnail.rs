//! Миниатюра последнего снимка: стеклянный круг Ø48 с картинкой снимка внутри, тап открывает системную
//! галерею (`ThumbnailTapped`).
//!
//! Картинка (14g) лежит внутри стекла кругом диаметром `bottom.thumbnail_dot_scale` (86 % Ø, `[estimate]`,
//! решение 164) с заполнением «cover» и поворачивается вместе со значками (решение 165: узел картинки —
//! вращающееся содержимое, как у кнопки меню). Пиксели уже приходят «как покажет галерея» (поворот снимка
//! применён при уменьшении), поэтому в портрете поворот картинки нулевой. Смена картинки — перекрёстное
//! растворение (`thumbnail::ThumbnailTracker`): старая под новой полностью непрозрачной, не больше двух
//! элементов «картинка» за кадр. Без картинки («сохранено» пришло без неё) — серый диск, как до 14g.

use ui_core::gesture::GestureEvent;
use ui_core::id::NodeId;
use ui_draw::color::Opacity;
use ui_draw::draw_list::{DrawItem, ImageClip, ImageFit, ImageItem, ShapeItem};
use ui_draw::geometry::PxSize;
use ui_draw::picture::UiPicture;
use ui_draw::scene_bridge::NodePlacementPx;
use ui_draw::shape::ShapeGeometry;
use ui_draw::transform::Affine2;
use ui_kit::tokens::GlassRole;
use ui_kit::view::{GlassShape, ViewError, opacity_of_placement};

use super::bottom_row::BottomRowFade;
use super::button::{ButtonEvent, PressButton};
use super::error::WidgetError;
use super::intent::{ButtonId, ScreenGesture};
use super::kit::SceneKit;
use super::layers::{DrawLayers, Layer, VisualIndex};
use super::{Ctx, Outputs, rotation_only, single_member_group};
use crate::action::ScreenAction;
use crate::shutter::ThumbnailState;

pub(crate) struct ThumbnailWidget {
    button: PressButton,
    /// Узел картинки: поворачивается по ориентации телефона вместе со значками, сам круг стоит.
    picture_node: NodeId,
    /// Растворение, пока открыта нижняя карточка.
    fade: BottomRowFade,
}

impl ThumbnailWidget {
    pub(crate) fn build(kit: &mut SceneKit, ctx: &Ctx<'_>, parent: NodeId) -> Result<Self, WidgetError> {
        let frame = ctx.geometry.thumbnail();
        let node = kit.add_node(parent, "thumbnail", frame, ctx.geometry.screen())?;
        let button = PressButton::attach(kit, ButtonId::Thumbnail, node, frame, ctx)?;
        let picture_node = kit.add_rotating_content(node, "thumbnail_picture")?;
        Ok(Self {
            button,
            picture_node,
            fade: BottomRowFade::default(),
        })
    }

    /// Миниатюра растворяется и не ловит касаний, пока открыта карточка (решение 103).
    pub(crate) fn sync(&mut self, kit: &mut SceneKit, ctx: &Ctx<'_>) -> Result<(), WidgetError> {
        let shown = self.fade.sync(kit, ctx, self.button.node())?;
        kit.set_interactive(self.button.node(), shown)
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
        if placement.opacity <= 0.0 {
            return Ok(());
        }
        let (group, host) = single_member_group(
            ctx,
            GlassRole::Button,
            GlassShape::Circle,
            &placement,
            self.button.press_state(ctx.now),
            "thumbnail",
        )?;
        layers.push(Layer::Glass, DrawItem::Glass(group));
        let diameter =
            placement.size.width.min(placement.size.height) * ctx.geometry.config().bottom.thumbnail_dot_scale;
        match ctx.state.thumbnail_ref() {
            ThumbnailState::Empty => {}
            // «Сохранено» без картинки: однотонный диск — признак «снимок есть» (круг симметричен, поворот не нужен).
            ThumbnailState::Last { picture: None, .. } => layers.push(
                Layer::Content,
                DrawItem::Shape(ShapeItem {
                    geometry: ShapeGeometry::Circle { diameter },
                    transform: host,
                    fill: ctx.tokens.colors.text_muted,
                    stroke: None,
                    shadow: None,
                    opacity: opacity_of_placement(&placement, "thumbnail")?,
                }),
            ),
            ThumbnailState::Last {
                picture: Some(picture), ..
            } => self.draw_pictures(ctx, visuals, &placement, host, diameter, picture, layers)?,
        }
        Ok(())
    }

    /// Картинка снимка (и, пока идёт растворение, прежняя под ней) внутри круга.
    #[allow(
        clippy::too_many_arguments,
        reason = "рисование: контекст, размещения и слой — отдельные входы"
    )]
    fn draw_pictures(
        &self,
        ctx: &Ctx<'_>,
        visuals: &VisualIndex,
        button_placement: &NodePlacementPx,
        host: Affine2,
        diameter: f32,
        picture: &UiPicture,
        layers: &mut DrawLayers,
    ) -> Result<(), WidgetError> {
        let Some(picture_placement) = visuals.placement(self.picture_node) else {
            return Ok(());
        };
        // Хост — члену стеклянной группы (масштаб нажатия), поверх него поворот узла картинки.
        let transform = rotation_only(&picture_placement, button_placement, host);
        let base_opacity = opacity_of_placement(&picture_placement, "thumbnail_picture")?.get();
        let blend = ctx.state.thumbnail_blend(ctx.now);
        let image = |source: &UiPicture, opacity: f32| -> Result<DrawItem, WidgetError> {
            Ok(DrawItem::Image(ImageItem {
                picture: source.clone(),
                transform,
                size_px: PxSize::new(diameter, diameter),
                fit: ImageFit::Cover,
                clip: ImageClip::Circle,
                opacity: Opacity::new(opacity).map_err(|_| ViewError::InvalidOpacity {
                    what: "thumbnail_picture",
                    value: opacity,
                })?,
            }))
        };
        // Нижняя картинка — полностью непрозрачная (в пределах прозрачности самой миниатюры): стекло сквозь
        // неё посреди растворения не просвечивает.
        if let Some(underlay) = blend.underlay {
            layers.push(Layer::Content, image(underlay, base_opacity)?);
        }
        if blend.overlay_opacity > 0.0 {
            layers.push(Layer::Content, image(picture, base_opacity * blend.overlay_opacity)?);
        }
        Ok(())
    }
}

//! Круглая боковая кнопка карточки (чёрный диск и значок): «‹» назад, «×» закрыть, «↻» сброс.
//!
//! Одна кнопка на две карточки (решение 84): нижнюю карточку инструмента PHOTO (`tool_card.rs`) и карточки меню
//! раздела калибровки (`calibration/menu`). Диск и значок рисуются от размещений узлов, поэтому кнопка не знает,
//! стоит ли она на экране или внутри повёрнутого корня раздела; разница только в узле значка: на экране он
//! поворачивается вместе с телефоном, внутри раздела стоит неподвижно (поворачивает корень).

use ui_core::geometry::Rect;
use ui_core::id::NodeId;
use ui_draw::color::{DisplayColor, Opacity};
use ui_draw::draw_list::{DrawItem, ShapeItem};
use ui_draw::icons::IconId;
use ui_draw::scene_bridge::NodePlacementPx;
use ui_draw::shape::ShapeGeometry;
use ui_draw::transform::Affine2;
use ui_kit::view::centered_icon;

use super::Ctx;
use super::button::PressButton;
use super::error::WidgetError;
use super::intent::ButtonId;
use super::kit::SceneKit;
use super::layers::{DrawLayers, Layer, VisualIndex};

/// Поворачивается ли значок кнопки вместе с телефоном.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IconTurn {
    /// Экран PHOTO: значок следует за ориентацией телефона, кнопка стоит.
    FollowsPhone,
    /// Раздел калибровки: поворачивает весь корень, значок сам не крутится.
    Fixed,
}

/// Как рисуется боковая кнопка в этом кадре.
#[derive(Debug, Clone, Copy)]
pub(crate) struct SideButtonLook {
    pub icon: IconId,
    /// Поворот значка (стрелка «‹» смотрит влево; «×» и «↻» не поворачиваются).
    pub rotation: Affine2,
    /// Прозрачность содержимого карточки из морфа.
    pub content_alpha: f32,
    /// Сдвиг содержимого карточки из морфа, px.
    pub slide_px: f32,
    /// Дополнительный множитель прозрачности (затемнение «сброса» при EV = 0); 1,0 — без затемнения.
    pub extra_opacity: f32,
}

/// Круглая боковая кнопка: кнопка и узел значка (поворачивается по ориентации телефона).
pub(crate) struct SideButton {
    pub(crate) button: PressButton,
    content: NodeId,
}

impl SideButton {
    pub(crate) fn build(
        kit: &mut SceneKit,
        ctx: &Ctx<'_>,
        (card_node, card_rect): (NodeId, Rect),
        (id, rect): (ButtonId, Rect),
        turn: IconTurn,
    ) -> Result<Self, WidgetError> {
        let node = kit.add_node(card_node, "card_button", rect, card_rect)?;
        let button = PressButton::attach(kit, id, node, rect, ctx)?;
        let content = match turn {
            IconTurn::FollowsPhone => kit.add_rotating_content(node, "card_button_icon")?,
            IconTurn::Fixed => kit.add_fixed_content(node, "card_button_icon")?,
        };
        // Диск Ø 36 рисуется как есть, а попасть в него можно в квадрат ≥ 44 pt (HIG).
        kit.ensure_min_touch_side(node, rect, ctx.geometry.config().card.button_touch_side_pt)?;
        kit.set_interactive(node, false)?;
        Ok(Self { button, content })
    }

    /// Чёрный диск и значок поверх.
    pub(crate) fn draw(
        &self,
        ctx: &Ctx<'_>,
        visuals: &VisualIndex,
        layers: &mut DrawLayers,
        look: SideButtonLook,
    ) -> Result<(), WidgetError> {
        let SideButtonLook {
            icon,
            rotation,
            content_alpha,
            slide_px,
            extra_opacity,
        } = look;
        let (Some(button), Some(content)) = (visuals.placement(self.button.node()), visuals.placement(self.content))
        else {
            return Ok(());
        };
        let alpha = (button.opacity * content_alpha * extra_opacity).clamp(0.0, 1.0);
        if alpha <= 0.0 {
            return Ok(());
        }
        let opacity = Opacity::new(alpha)
            .map_err(|_| WidgetError::Inconsistent("прозрачность кнопки карточки вне 0..1".to_owned()))?;
        let card = &ctx.geometry.config().card;
        let slide = Affine2::translate(0.0, slide_px);
        layers.push(
            Layer::Content,
            DrawItem::Shape(ShapeItem {
                geometry: ShapeGeometry::Circle {
                    diameter: button.size.width.min(button.size.height),
                },
                transform: slide.then(button.transform),
                fill: DisplayColor {
                    a: card.button_disc_alpha,
                    ..DisplayColor::BLACK
                },
                stroke: None,
                shadow: None,
                opacity,
            }),
        );
        let content = NodePlacementPx {
            transform: rotation.then(content.transform),
            ..content
        };
        layers.push(
            Layer::Content,
            centered_icon(
                icon,
                card.button_icon_size_pt,
                ctx.px,
                slide.then(content.transform),
                ctx.tokens.colors.text_primary,
                opacity,
            ),
        );
        Ok(())
    }
}

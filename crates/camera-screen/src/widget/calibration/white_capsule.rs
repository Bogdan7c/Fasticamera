//! Капсула белого в правой части полоски прибора (решения 290, 294): «5600 K» на материале раздела, на каждой странице.
//!
//! Это единственная кнопка белого раздела. Тап открывает нижнюю карточку белого (`SectionCard::White`), а повторный тап
//! закрывает её: сама капсула лежит под модальной завесой карточки, поэтому закрывает тап по завесе, который
//! ничем не отличается для человека (решение 281).
//!
//! # Что показывает
//! Число — текущее белое камеры в Кельвинах (те же факты и то же округление, что число на кнопке белого у затвора в
//! PHOTO, `kelvin_text`), единица «K». Пометки «AUTO» нет (решение 294: в разделе слежение на паузе, а включают его в
//! PHOTO). Число жёлтое при ручном белом, иначе белое (как в PHOTO). Длинное число сжимается под капсулу.
//!
//! # Зона касания
//! Узел — ровно место `ScopeStripFrames::trailing` (вся высота полоски, 44 pt, цели соседей не перекрываются; тест
//! соседства). Видимая капсула ниже (`segment_height_pt`, как сегменты осей) и лежит в этой зоне; расширять зону
//! за полоску нельзя: она залезла бы на картинку прибора со свайпом.

use ui_core::geometry::{Rect, Size};
use ui_core::gesture::GestureEvent;
use ui_core::id::NodeId;
use ui_draw::transform::Affine2;
use ui_kit::tokens::TextRoleKind;
use ui_kit::view::{GlassShape, centered_text};

use super::paint::NodePaint;
use crate::action::ScreenAction;
use crate::white_display::{KELVIN_UNIT, kelvin_text, number_is_manual};
use crate::widget::button::{ButtonEvent, PressButton};
use crate::widget::error::WidgetError;
use crate::widget::intent::{ButtonId, ScreenGesture};
use crate::widget::kit::SceneKit;
use crate::widget::layers::{DrawLayers, Layer, VisualIndex};
use crate::widget::text_runs::TextRuns;
use crate::widget::white_button::fit_scale;
use crate::widget::{Ctx, Outputs, SECTION_PANEL_MATERIAL, single_member_material};

/// Капсула белого.
pub(super) struct WhiteCapsule {
    /// Зона касания (вся высота полоски).
    button: PressButton,
    /// Видимая капсула внутри зоны: рисуется как стекло материала раздела.
    face: NodeId,
    /// Ширина видимой капсулы, pt: число сжимается, чтобы уместиться в неё с запасом.
    width_pt: f64,
}

impl WhiteCapsule {
    /// Строит капсулу в месте `slot` (система полоски `parent_rect`).
    pub(super) fn build(
        kit: &mut SceneKit,
        ctx: &Ctx<'_>,
        (parent, parent_rect): (NodeId, Rect),
        slot: Rect,
    ) -> Result<Self, WidgetError> {
        let node = kit.add_node(parent, "calib_white_capsule", slot, parent_rect)?;
        let button = PressButton::attach(kit, ButtonId::SectionWhiteCapsule, node, slot, ctx)?;
        let config = &ctx.config.calibration.widgets;
        let face_frame = Rect::centered_at(slot.center(), Size::new(slot.size.width, config.segment_height_pt));
        let face = kit.add_node(node, "calib_white_capsule_face", face_frame, slot)?;
        Ok(Self {
            button,
            face,
            width_pt: face_frame.size.width,
        })
    }

    /// Тап по капсуле → `WhiteButtonTapped`. `false` — событие чужое.
    pub(super) fn handle_gesture(
        &mut self,
        kit: &SceneKit,
        event: &GestureEvent<ScreenGesture>,
        out: &mut Outputs,
    ) -> Result<bool, WidgetError> {
        if event.intent != ScreenGesture::Press(ButtonId::SectionWhiteCapsule) {
            return Ok(false);
        }
        let data = kit.gesture_data_in_node(self.button.node(), event.data);
        if self.button.handle(event.state, data, event.time)? == ButtonEvent::Activated {
            out.actions.push(ScreenAction::WhiteButtonTapped);
        }
        Ok(true)
    }

    pub(super) fn draw(
        &self,
        ctx: &Ctx<'_>,
        visuals: &VisualIndex,
        layers: &mut DrawLayers,
        texts: &mut TextRuns,
    ) -> Result<(), WidgetError> {
        let Some(face) = visuals.placement(self.face) else {
            return Ok(());
        };
        let (surface, _host) = single_member_material(
            ctx,
            SECTION_PANEL_MATERIAL,
            GlassShape::Capsule,
            &face,
            self.button.press_state(ctx.now),
            "calib_white_capsule",
        )?;
        layers.push(Layer::Glass, surface);
        let Some(paint) = NodePaint::new(ctx, face, 1.0)? else {
            return Ok(());
        };
        let white = ctx.state.camera_facts().white;
        let color = if number_is_manual(white.held, white.follow) {
            ctx.tokens.colors.focus_yellow
        } else {
            ctx.tokens.colors.text_primary
        };
        let text = format!(
            "{} {KELVIN_UNIT}",
            kelvin_text(white.held.setting.temperature, &ctx.config.white)
        );
        let run = texts.run(ctx, &text, TextRoleKind::Caption)?;
        let widgets = &ctx.config.calibration.widgets;
        let room_px = ctx.px.px(self.width_pt - 2.0 * widgets.scope_white_capsule_padding_pt);
        let fit = fit_scale(run.advance_px, room_px);
        layers.push(
            Layer::Content,
            centered_text(
                run,
                Affine2::uniform_scale(fit).then(paint.at(0.0, 0.0)),
                color,
                paint.opacity(),
            ),
        );
        Ok(())
    }
}

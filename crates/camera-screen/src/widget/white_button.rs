//! Кнопка белого в ряду затвора (WB35): стеклянный круг с числом Кельвинов над «K», как у EV.
//!
//! # Где стоит и как ездит
//! Круг того же диаметра и того же стекла, что кнопка меню Controls, на высоте центра затвора, между затвором и
//! кнопкой меню (или зеркально слева, `WhiteScreenConfig::button_side`). Видимость и подъём кнопки ведёт тот же
//! [`ShutterRowPose`], что и затвор: под шторкой Controls она растворяется и не нажимается, над открытой карточкой
//! едет вверх тем же кадром, той же пружиной и на ту же высоту (100 / 124 pt), поэтому расстояние до затвора
//! постоянно в каждом кадре анимации. Узел — ребёнок `photo_root`, так что в разделе калибровки он скрыт целиком.
//! Остальные состояния кнопка переживает как кнопка меню (отсчёт таймера, съёмка, камера недоступна): ни нажатие,
//! ни вид от них не меняются, а решение «камеры нет» принимает движок.
//!
//! # Жесты
//! Тап открывает, закрывает или переключает карточку; долгое нажатие берёт белое телефона. Рецепт — `Press`, `Tap`
//! и `LongPress` на одном узле (`ui-core/tests/button_recipe_arbitration.rs`): `Tap` гасит срабатывание видоискателя
//! под кнопкой, `LongPress` отменяет тап.
//!
//! # Вид
//! * Число — роль `StripValue`, «K» — `StripUnit`; жёлтое при ручном белом, иначе белое (WB16). Оба вращаются с
//!   устройством (узел `rotating_content`), стекло стоит.
//! * При слежении (WB49) число белое и живое, а под ним вместо «K» стоит «AUTO»: новой роли и нового цвета нет.
//!   «AUTO» сжимается под круг тем же правилом, что и число.
//! * Пять цифр («12000») шире круга: число сжимается равномерно до `number_max_width_pt`, размер цифр у остальных
//!   значков остаётся единым.
//! * Ожидание ответа — мерцание всей кнопки; отказ — тряска ВСЕЙ кнопки (стекло и число вместе: число, уехавшее
//!   по стеклу, читалось бы как сбой отрисовки). Reduce Motion учитывает [`WhiteAttention`].

use ui_core::geometry::Rect;
use ui_core::gesture::{GestureData, GestureEvent, GestureState, LongPressSpec, RecognizerSpec};
use ui_core::id::NodeId;
use ui_core::time::MonotonicNanos;
use ui_draw::color::Opacity;
use ui_draw::draw_list::DrawItem;
use ui_draw::scene_bridge::NodePlacementPx;
use ui_draw::transform::Affine2;
use ui_kit::tokens::{GlassRole, TextRoleKind};
use ui_kit::view::{GlassShape, centered_text, opacity_of_placement};

use super::button::{ButtonEvent, PressButton};
use super::error::WidgetError;
use super::intent::{ButtonId, ScreenGesture};
use super::kit::SceneKit;
use super::layers::{DrawLayers, Layer, VisualIndex};
use super::shutter_row::ShutterRowPose;
use super::text_runs::TextRuns;
use super::white_attention::WhiteAttention;
use super::{Ctx, Outputs, rotation_only, single_member_group};
use crate::action::ScreenAction;
use crate::white_display::{kelvin_text, number_is_manual, white_unit_label};

/// Во сколько раз сжать строку шириной `advance_px`, чтобы она не вышла за `max_width_px` (1, если и так помещается).
pub(crate) fn fit_scale(advance_px: f32, max_width_px: f32) -> f32 {
    if advance_px > max_width_px && advance_px > 0.0 {
        max_width_px / advance_px
    } else {
        1.0
    }
}

/// Кнопка белого у затвора.
pub(crate) struct WhiteButtonWidget {
    button: PressButton,
    /// Узел содержимого: поворачивается по ориентации телефона, стекло стоит.
    content: NodeId,
    /// Видимость и подъём: общие с затвором (`shutter_row.rs`).
    pose: ShutterRowPose,
    /// Рамка раскладки (без сдвига): от неё считается рамка трекера нажатия.
    base_frame: Rect,
    attention: WhiteAttention,
}

impl WhiteButtonWidget {
    pub(crate) fn build(kit: &mut SceneKit, ctx: &Ctx<'_>, parent: NodeId) -> Result<Self, WidgetError> {
        let frame = ctx.geometry.white_button(ctx.config.white.button_side);
        let node = kit.add_node(parent, "white_button", frame, ctx.geometry.screen())?;
        let button = PressButton::attach(kit, ButtonId::WhiteButton, node, frame, ctx)?;
        kit.add_recognizer(
            node,
            RecognizerSpec::LongPress(LongPressSpec::default()),
            ScreenGesture::WhiteButtonLongPress,
        )?;
        let content = kit.add_rotating_content(node, "white_button_content")?;
        Ok(Self {
            button,
            content,
            pose: ShutterRowPose::default(),
            base_frame: frame,
            attention: WhiteAttention::new(ctx),
        })
    }

    /// Тап открывает карточку, долгое нажатие берёт белое телефона. Событие чужого узла — `false`.
    pub(crate) fn handle_gesture(
        &mut self,
        event: &GestureEvent<ScreenGesture>,
        out: &mut Outputs,
    ) -> Result<bool, WidgetError> {
        match event.intent {
            ScreenGesture::Press(ButtonId::WhiteButton) => {
                if self.button.handle(event.state, event.data, event.time)? == ButtonEvent::Activated {
                    out.actions.push(ScreenAction::WhiteButtonTapped);
                }
                Ok(true)
            }
            ScreenGesture::WhiteButtonLongPress => {
                // Долгое нажатие начинается ровно один раз за касание (`Began`); продолжение и конец — тишина.
                if event.state == GestureState::Began && matches!(event.data, GestureData::LongPress { .. }) {
                    out.actions.push(ScreenAction::WhiteButtonLongPressed);
                }
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    /// Выравнивает позу (видимость, подъём) и внимание; рамку трекера нажатия ведёт вслед за сдвигом узла.
    pub(crate) fn sync(&mut self, kit: &mut SceneKit, ctx: &Ctx<'_>) -> Result<(), WidgetError> {
        self.attention.sync(ctx);
        let offset_pt = self.pose.sync(kit, ctx, self.button.node())?;
        self.button.set_frame(Rect::from_xywh(
            self.base_frame.min_x(),
            self.base_frame.min_y() + offset_pt,
            self.base_frame.size.width,
            self.base_frame.size.height,
        ))
    }

    /// Идёт ли мерцание или тряска (экрану нужны кадры, пока они не кончились).
    pub(crate) fn is_animating(&self, now: MonotonicNanos) -> bool {
        self.attention.is_animating(now)
    }

    pub(crate) fn draw(
        &self,
        ctx: &Ctx<'_>,
        visuals: &VisualIndex,
        layers: &mut DrawLayers,
        texts: &mut TextRuns,
    ) -> Result<(), WidgetError> {
        let Some(mut placement) = visuals.placement(self.button.node()) else {
            return Ok(());
        };
        if placement.opacity <= 0.0 {
            return Ok(());
        }
        // Тряска сдвигает всю кнопку в её осях (вместе с поворотом устройства): стекло и число вместе.
        // Поворот содержимого считается от неподвижной рамки, иначе обратное преобразование съело бы тряску.
        let resting_placement = placement;
        let shake = Affine2::translate(ctx.px.px(self.attention.offset_x_pt(ctx.now)), 0.0);
        placement.transform = shake.then(placement.transform);
        let (group, host) = single_member_group(
            ctx,
            GlassRole::Button,
            GlassShape::Circle,
            &placement,
            self.button.press_state(ctx.now),
            "white_button",
        )?;
        layers.push(Layer::Glass, DrawItem::Glass(group));
        let Some(content_placement) = visuals.placement(self.content) else {
            return Ok(());
        };
        // Мерцает только число (WB32): стекло неподвижно, кнопка не выглядит исчезающей.
        let content_placement = NodePlacementPx {
            opacity: content_placement.opacity * self.attention.opacity(ctx.now),
            ..content_placement
        };
        // Поворот содержимого относительно стекла, поверх члена группы (масштаб нажатия и тряска уже в нём).
        let host = rotation_only(&content_placement, &resting_placement, host);
        self.draw_number(
            ctx,
            host,
            opacity_of_placement(&content_placement, "white_button_content")?,
            layers,
            texts,
        )
    }

    /// Число Кельвинов над «K» по центру круга; длинное число сжато под круг.
    fn draw_number(
        &self,
        ctx: &Ctx<'_>,
        host: Affine2,
        opacity: Opacity,
        layers: &mut DrawLayers,
        texts: &mut TextRuns,
    ) -> Result<(), WidgetError> {
        let strip = &ctx.geometry.config().strip;
        let held = ctx.state.camera_facts().white.held;
        let follow = ctx.state.camera_facts().white.follow;
        let color = if number_is_manual(held, follow) {
            ctx.tokens.colors.focus_yellow
        } else {
            ctx.tokens.colors.text_primary
        };
        let number = kelvin_text(held.setting.temperature, &ctx.config.white);
        let value = texts.run(ctx, &number, TextRoleKind::StripValue)?;
        // Сжатие длинного числа: не шире `number_max_width_pt` (не сжимается, если и так помещается).
        let max_width_px = ctx.px.px(ctx.geometry.config().white_button.number_max_width_pt);
        let fit = fit_scale(value.advance_px, max_width_px);
        let line_at = |offset_pt: f64| Affine2::translate(0.0, ctx.px.px(offset_pt)).then(host);
        layers.push(
            Layer::Content,
            centered_text(
                value,
                Affine2::uniform_scale(fit).then(line_at(strip.ev_value_offset_pt)),
                color,
                opacity,
            ),
        );
        // Строка единицы: «K», при слежении «AUTO» (WB49). Сжимается под круг тем же правилом, что и число.
        let unit = texts.run(ctx, white_unit_label(follow), TextRoleKind::StripUnit)?;
        let unit_fit = fit_scale(unit.advance_px, max_width_px);
        layers.push(
            Layer::Content,
            centered_text(
                unit,
                Affine2::uniform_scale(unit_fit).then(line_at(strip.ev_unit_offset_pt)),
                color,
                opacity,
            ),
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::fit_scale;

    #[test]
    fn a_line_wider_than_the_circle_is_squeezed_uniformly_and_a_narrow_one_is_left_alone() {
        assert_eq!(fit_scale(90.0, 60.0), 60.0 / 90.0, "шире круга — сжать ровно до ширины");
        assert_eq!(fit_scale(60.0, 60.0), 1.0, "ровно по ширине — без сжатия");
        assert_eq!(fit_scale(20.0, 60.0), 1.0, "уже круга — без сжатия");
        assert_eq!(fit_scale(0.0, 60.0), 1.0, "пустая строка не делит на ноль");
    }
}

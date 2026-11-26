//! Кнопка CLEAN внизу слева (решения 259, 276): капсула с подписью на линии капсулы режимов, зеркально кнопке меню.
//! Пока палец держит кнопку, на превью и приборе основа без вида.
//!
//! Раньше она сидела в шапке колонки ручек; там освободилось место для соседних основ тона. Теперь её держат большим
//! пальцем у нижнего края, глядя на картинку. Идиома владельца (решение 189): значка «глаз» в наборе нет, поэтому
//! капсула с надписью, поверхность — материал раздела без стекла (решение 261).
//!
//! Нажатие даёт `CleanViewPressed`, отпускание или отмена — `CleanViewReleased`. Залипнуть кнопка не может: если
//! раздел собрали заново (поворот, пересборка сцены), пока состояние говорит «держится», палец прежней кнопки
//! потерян, и первый же `advance` новой кнопки один раз отдаёт `CleanViewReleased`. Проверка — ровно один раз после
//! сборки, а не на каждом кадре: удержание, начатое не пальцем этой кнопки (сценарий, тест, другой хост действием
//! `CleanViewPressed`), законно и не должно отпускаться само.

use ui_core::geometry::{Point, Rect, Size};
use ui_core::gesture::{GestureEvent, GestureState};
use ui_core::id::NodeId;
use ui_kit::tokens::TextRoleKind;
use ui_kit::view::GlassShape;

use super::paint::NodePaint;
use crate::action::ScreenAction;
use crate::calibration::CleanViewState;
use crate::widget::button::PressButton;
use crate::widget::error::WidgetError;
use crate::widget::intent::{ButtonId, ScreenGesture};
use crate::widget::kit::SceneKit;
use crate::widget::layers::{DrawLayers, Layer, VisualIndex};
use crate::widget::text_runs::TextRuns;
use crate::widget::{Ctx, Outputs, SECTION_PANEL_MATERIAL, single_member_material};

/// Кнопка «чистяк».
pub(super) struct CleanButton {
    button: PressButton,
    /// Размер капсулы (рисунок); зона касания шире — до `button_touch_side_pt`.
    size: Size,
    /// Палец сейчас держит кнопку.
    held: bool,
    /// Кнопку только что собрали и ещё не сверили с состоянием: удержание, пережившее пересборку, осиротело.
    orphan_hold_check_pending: bool,
}

impl CleanButton {
    /// Строит кнопку с центром в `center` (система корня раздела).
    pub(super) fn build(
        kit: &mut SceneKit,
        ctx: &Ctx<'_>,
        (parent, parent_rect): (NodeId, Rect),
        center: Point,
    ) -> Result<Self, WidgetError> {
        let config = &ctx.config.calibration.widgets;
        let size = Size::new(config.clean_button_width_pt, config.clean_button_height_pt);
        let frame = Rect::centered_at(center, size);
        let node = kit.add_node(parent, "calib_clean", frame, parent_rect)?;
        let button = PressButton::attach(kit, ButtonId::CleanView, node, frame, ctx)?;
        kit.ensure_min_touch_side(node, frame, ctx.geometry.config().card.button_touch_side_pt)?;
        Ok(Self {
            button,
            size,
            held: false,
            orphan_hold_check_pending: true,
        })
    }

    /// Удержание: палец лёг — показать основу, поднялся или жест отменён — вернуть вид с правками. `false` — событие
    /// чужое.
    pub(super) fn handle_gesture(
        &mut self,
        kit: &SceneKit,
        event: &GestureEvent<ScreenGesture>,
        out: &mut Outputs,
    ) -> Result<bool, WidgetError> {
        if event.intent != ScreenGesture::Press(ButtonId::CleanView) {
            return Ok(false);
        }
        let data = kit.gesture_data_in_node(self.button.node(), event.data);
        self.button.handle(event.state, data, event.time)?;
        match event.state {
            GestureState::Began if !self.held => {
                self.held = true;
                out.actions.push(ScreenAction::CleanViewPressed);
            }
            GestureState::Ended | GestureState::Cancelled => {
                self.held = false;
                out.actions.push(ScreenAction::CleanViewReleased);
            }
            _ => {}
        }
        Ok(true)
    }

    /// Первый кадр после сборки: «чистяк» держится по состоянию, а держал его палец прежней кнопки — отпускаем,
    /// чтобы вид не залип. Дальше удержание без пальца (действием) не трогаем.
    pub(super) fn advance(&mut self, ctx: &Ctx<'_>, out: &mut Outputs) {
        if std::mem::take(&mut self.orphan_hold_check_pending)
            && !self.held
            && ctx.state.calibration().clean_view() == CleanViewState::HoldingBase
        {
            out.actions.push(ScreenAction::CleanViewReleased);
        }
    }

    pub(super) fn draw(
        &self,
        ctx: &Ctx<'_>,
        visuals: &VisualIndex,
        layers: &mut DrawLayers,
        texts: &mut TextRuns,
    ) -> Result<(), WidgetError> {
        let Some(placement) = visuals.placement(self.button.node()) else {
            return Ok(());
        };
        let (surface, _host) = single_member_material(
            ctx,
            SECTION_PANEL_MATERIAL,
            GlassShape::Capsule,
            &placement,
            self.button.press_state(ctx.now),
            "calib_clean",
        )?;
        layers.push(Layer::Glass, surface);
        let Some(paint) = NodePaint::new(ctx, placement, 1.0)? else {
            return Ok(());
        };
        let config = &ctx.config.calibration.widgets;
        let holding = ctx.state.calibration().clean_view() == CleanViewState::HoldingBase;
        let alpha = if holding {
            config.clean_button_held_fill_alpha
        } else {
            config.clean_button_fill_alpha
        };
        layers.push(
            Layer::Content,
            paint.capsule(
                (0.0, 0.0),
                (self.size.width, self.size.height),
                ctx.tokens.colors.text_primary.with_alpha(alpha),
            ),
        );
        let label = texts.run(ctx, "CLEAN", TextRoleKind::Caption)?;
        layers.push(
            Layer::Content,
            paint.text_centered(label, 0.0, 0.0, ctx.tokens.colors.text_primary),
        );
        Ok(())
    }
}

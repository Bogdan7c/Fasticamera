//! Вид колёс цвета (решения 204–207): вкладки колёс, название с «↻», круг толчка и шкала кольца яркости.
//!
//! Раскладка сверху вниз (рамки считает `calibration::WheelsFrames` — одна функция и для виджета, и для запроса
//! фасада): ряд вкладок LIFT · GAMMA · GAIN · OFFSET; строка названия выбранного колеса с «↻» колеса целиком;
//! круг толчка с «↻» толчка в правом нижнем углу; шкала кольца яркости с «↻» кольца. Всё помещается в окно
//! колонки без прокрутки, поэтому содержимое вида ровно высоты окна.
//!
//! Экран значений колёс не хранит: точка, кольцо и метки «правлено» читаются из `LookFacts`, выбор колеса — из
//! `CalibrationState`. Каждая «↻» шлёт сброс своей части (`Whole` / `Push` / `Ring`) только если эта часть
//! отличается от пресета; тусклая «↻» (44 %) не делает ничего.

mod pad;

use look_contract::{LookControl, WheelPart, WheelZone};
use ui_core::geometry::Rect;
use ui_core::gesture::{GestureEvent, GestureState};
use ui_core::id::NodeId;
use ui_kit::tokens::TextRoleKind;

use self::pad::WheelPad;
use super::knob_row::{RowTarget, RulerRow};
use super::paint::NodePaint;
use super::reset_disc::draw_reset_disc;
use super::segments::{SegmentRow, SegmentSpec, push_if_activated};
use super::space::SectionSpace;
use crate::action::ScreenAction;
use crate::calibration::WheelsFrames;
use crate::widget::button::{ButtonEvent, PressButton};
use crate::widget::error::WidgetError;
use crate::widget::intent::{ButtonId, ScreenGesture};
use crate::widget::kit::SceneKit;
use crate::widget::layers::{DrawLayers, Layer, VisualIndex};
use crate::widget::text_runs::TextRuns;
use crate::widget::{Ctx, Outputs};

/// Подпись вкладки и названия колеса заглавными.
fn zone_title(zone: WheelZone) -> &'static str {
    match zone {
        WheelZone::Lift => "LIFT",
        WheelZone::Gamma => "GAMMA",
        WheelZone::Gain => "GAIN",
        WheelZone::Offset => "OFFSET",
    }
}

/// Вкладки в порядке показа: `(колесо, подпись)`.
fn tab_entries() -> Vec<(WheelZone, &'static str)> {
    WheelZone::ALL.iter().map(|zone| (*zone, zone_title(*zone))).collect()
}

/// Вид колёс.
pub(in crate::widget::calibration) struct WheelsView {
    frames: WheelsFrames,
    tabs: SegmentRow<WheelZone>,
    /// Узел строки названия: на нём рисуется название, под ним стоит «↻» колеса целиком.
    title_node: NodeId,
    whole_reset: PressButton,
    /// Колесо, выбранное в момент нажатия «↻» колеса целиком.
    whole_pressed_zone: Option<WheelZone>,
    pad: WheelPad,
    push_reset: PressButton,
    /// Колесо, выбранное в момент нажатия «↻» толчка.
    push_pressed_zone: Option<WheelZone>,
    ring_row: RulerRow,
    /// Доступны ли «↻» касаниям (факты есть).
    enabled: bool,
    /// Высота содержимого вида: ровно высота окна колонки (вид не прокручивается).
    content_height_pt: f64,
}

impl WheelsView {
    /// Строит вид внутри узла содержимого `content_node` (рамка `content`: внутренняя ширина колонки × высота окна).
    pub(in crate::widget::calibration) fn build(
        kit: &mut SceneKit,
        ctx: &Ctx<'_>,
        space: &SectionSpace,
        (content_node, content): (NodeId, Rect),
        frames: WheelsFrames,
    ) -> Result<Self, WidgetError> {
        let touch_side = ctx.geometry.config().card.button_touch_side_pt;
        let tabs = SegmentRow::build(
            kit,
            ctx,
            (content_node, content),
            frames.tabs,
            SegmentSpec {
                entries: &tab_entries(),
                node_label: "calib_wheel_tab",
                id_of: ButtonId::CalibrationWheelTab,
                // Четыре вкладки в узкой портретной колонке: GAMMA / OFFSET в обычной подписи (12 pt) шире капсулы.
                text_role: TextRoleKind::SegmentLabel,
            },
        )?;
        let title_node = kit.add_node(content_node, "calib_wheel_title", frames.title, content)?;
        let whole_node = kit.add_node(title_node, "calib_wheel_reset_whole", frames.whole_reset, frames.title)?;
        let whole_reset = PressButton::attach(
            kit,
            ButtonId::CalibrationWheelReset(WheelPart::Whole),
            whole_node,
            frames.whole_reset,
            ctx,
        )?;
        kit.ensure_min_touch_side(whole_node, frames.whole_reset, touch_side)?;
        let pad = WheelPad::build(kit, ctx, (content_node, content), &frames)?;
        // «↻» толчка — ребёнок узла круга: касание угла достаётся кнопке, остальной круг — пану точки.
        let push_node = kit.add_node(pad.node(), "calib_wheel_reset_push", frames.push_reset, frames.circle)?;
        let push_reset = PressButton::attach(
            kit,
            ButtonId::CalibrationWheelReset(WheelPart::Push),
            push_node,
            frames.push_reset,
            ctx,
        )?;
        let ring_row = RulerRow::build(
            kit,
            ctx,
            space,
            (content_node, content),
            (RowTarget::WheelRing, frames.ring_row),
        )?;
        Ok(Self {
            frames,
            tabs,
            title_node,
            whole_reset,
            whole_pressed_zone: None,
            pad,
            push_reset,
            push_pressed_zone: None,
            ring_row,
            enabled: false,
            content_height_pt: content.size.height,
        })
    }

    pub(in crate::widget::calibration) fn handle_gesture(
        &mut self,
        kit: &SceneKit,
        space: &SectionSpace,
        ctx: &Ctx<'_>,
        event: &GestureEvent<ScreenGesture>,
        out: &mut Outputs,
    ) -> Result<bool, WidgetError> {
        let tab_event = self.tabs.handle_gesture(kit, event)?;
        if push_if_activated(tab_event, out, ScreenAction::WheelSelected) {
            return Ok(true);
        }
        for (part, button, pressed_zone) in [
            (WheelPart::Whole, &mut self.whole_reset, &mut self.whole_pressed_zone),
            (WheelPart::Push, &mut self.push_reset, &mut self.push_pressed_zone),
        ] {
            if event.intent == ScreenGesture::Press(ButtonId::CalibrationWheelReset(part)) {
                if event.state == GestureState::Began {
                    // Зона берётся на момент НАЖАТИЯ (как зона пана): вкладка, сменённая до отпускания, сброс не
                    // перенаправляет.
                    *pressed_zone = Some(ctx.state.calibration().selected_wheel());
                }
                let data = kit.gesture_data_in_node(button.node(), event.data);
                if button.handle(event.state, data, event.time)? == ButtonEvent::Activated {
                    let zone = pressed_zone.unwrap_or_else(|| ctx.state.calibration().selected_wheel());
                    reset_part_if_differs(ctx, zone, part, out);
                }
                return Ok(true);
            }
        }
        Ok(self.pad.handle_gesture(space, ctx, event, out)?
            || self.ring_row.handle_gesture(kit, space, ctx, event, out)?)
    }

    pub(in crate::widget::calibration) fn sync(
        &mut self,
        kit: &mut SceneKit,
        ctx: &Ctx<'_>,
    ) -> Result<(), WidgetError> {
        self.enabled = ctx.state.look_facts().is_some();
        self.tabs.set_touchable(kit, self.enabled)?;
        kit.set_interactive(self.whole_reset.node(), self.enabled)?;
        kit.set_interactive(self.push_reset.node(), self.enabled)?;
        self.pad.sync(kit, ctx)?;
        self.ring_row.sync(kit, ctx)
    }

    pub(in crate::widget::calibration) fn advance(&mut self, ctx: &Ctx<'_>, out: &mut Outputs) {
        self.ring_row.advance(ctx, out);
    }

    /// Высота содержимого, pt: окно целиком, прокручивать нечего.
    pub(in crate::widget::calibration) fn content_height(&self) -> f64 {
        self.content_height_pt
    }

    pub(in crate::widget::calibration) fn invalid_values(&self) -> u64 {
        self.pad.invalid_values() + self.ring_row.invalid_values()
    }

    pub(in crate::widget::calibration) fn suppressed_echo_events(&self) -> u64 {
        self.ring_row.suppressed_echo_events()
    }

    /// Положение линейки кольца в шагах, если `slot` — её номер (проба для тестов).
    #[cfg(test)]
    pub(in crate::widget::calibration) fn row_position_steps(
        &self,
        slot: u8,
        now: ui_core::time::MonotonicNanos,
    ) -> Option<f64> {
        (self.ring_row.slot() == slot).then(|| self.ring_row.position_steps(now))
    }

    pub(in crate::widget::calibration) fn draw(
        &self,
        ctx: &Ctx<'_>,
        visuals: &VisualIndex,
        layers: &mut DrawLayers,
        texts: &mut TextRuns,
    ) -> Result<(), WidgetError> {
        let config = &ctx.config.calibration.widgets;
        let facts = ctx.state.look_facts();
        let zone = ctx.state.calibration().selected_wheel();
        let dim = if self.enabled { 1.0 } else { config.disabled_row_opacity };
        let marked = |candidate: WheelZone| {
            facts.is_some_and(|facts| {
                facts.control_differs(&LookControl::Wheel {
                    zone: candidate,
                    part: WheelPart::Whole,
                })
            })
        };
        self.tabs.draw(ctx, (visuals, texts), layers, (zone, marked, dim))?;
        self.draw_title(ctx, visuals, layers, texts, dim)?;
        self.pad.draw(ctx, visuals, layers)?;
        self.draw_push_reset(ctx, visuals, layers, dim)?;
        self.ring_row.draw(ctx, visuals, layers, texts)
    }

    /// Название выбранного колеса слева и «↻» колеса целиком справа.
    fn draw_title(
        &self,
        ctx: &Ctx<'_>,
        visuals: &VisualIndex,
        layers: &mut DrawLayers,
        texts: &mut TextRuns,
        dim: f32,
    ) -> Result<(), WidgetError> {
        let Some(placement) = visuals.placement(self.title_node) else {
            return Ok(());
        };
        let Some(paint) = NodePaint::new(ctx, placement, dim)? else {
            return Ok(());
        };
        let zone = ctx.state.calibration().selected_wheel();
        let run = texts.run(ctx, zone_title(zone), TextRoleKind::CardTitle)?;
        let half_width = self.frames.title.size.width / 2.0;
        layers.push(
            Layer::Content,
            paint.text_left(run, -half_width, 0.0, ctx.tokens.colors.panel_label),
        );
        let center = (
            self.frames.whole_reset.center().x - self.frames.title.center().x,
            self.frames.whole_reset.center().y - self.frames.title.center().y,
        );
        let differs = ctx.state.look_facts().is_some_and(|facts| {
            facts.control_differs(&LookControl::Wheel {
                zone,
                part: WheelPart::Whole,
            })
        });
        draw_reset_disc(ctx, &paint, layers, center, differs);
        Ok(())
    }

    /// «↻» толчка в углу круга: рисуется в системе узла круга поверх всего круга.
    fn draw_push_reset(
        &self,
        ctx: &Ctx<'_>,
        visuals: &VisualIndex,
        layers: &mut DrawLayers,
        dim: f32,
    ) -> Result<(), WidgetError> {
        let Some(placement) = visuals.placement(self.pad.node()) else {
            return Ok(());
        };
        let Some(paint) = NodePaint::new(ctx, placement, dim)? else {
            return Ok(());
        };
        let center = (
            self.frames.push_reset.center().x - self.frames.circle.center().x,
            self.frames.push_reset.center().y - self.frames.circle.center().y,
        );
        let differs = ctx.state.look_facts().is_some_and(|facts| {
            facts.control_differs(&LookControl::Wheel {
                zone: ctx.state.calibration().selected_wheel(),
                part: WheelPart::Push,
            })
        });
        draw_reset_disc(ctx, &paint, layers, center, differs);
        Ok(())
    }
}

/// «↻» части выбранного колеса: сброс только если часть отличается от пресета (тусклая «↻» ничего не делает).
fn reset_part_if_differs(ctx: &Ctx<'_>, zone: WheelZone, part: WheelPart, out: &mut Outputs) {
    let Some(facts) = ctx.state.look_facts() else {
        return;
    };
    let control = LookControl::Wheel { zone, part };
    if facts.control_differs(&control) {
        out.actions.push(ScreenAction::ResetControlTapped(control));
    }
}

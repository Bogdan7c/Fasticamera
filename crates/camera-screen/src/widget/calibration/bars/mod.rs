//! Вид BARS (решение 343): вкладки зон LIFT · GAMMA · GAIN · OFFSET и под ними три шкалы R, G, B.
//!
//! Раскладка сверху вниз (рамки считает `calibration::BarsFrames` — одна функция и для виджета, и для запроса
//! фасада): ряд вкладок и три строки-шкалы. Всё помещается в окно колонки без прокрутки. Одна раскладка для обеих
//! ориентаций: колонка повёрнута вместе с разделом. Строки названия зоны и «↻ всей зоны» нет: в альбоме они не
//! помещаются (решение 343), у каждой полоски своя «↻».
//!
//! Вкладка ОБЩАЯ с видом WHEELS: выбор живёт в `CalibrationState::selected_wheel` (имя осталось «колесо»), вид своего
//! выбора не хранит. Шкалы — те же `RulerRow`, что у остальных ручек (−200…+200, ход 400 pt, «↻», метка пресета);
//! значения и метки «правлено» читаются из `LookFacts`, экран их не хранит. Вкладка помечена кольцом, если хоть одна
//! из трёх полосок этой зоны отличается от пресета.

use look_contract::{LookControl, PrimaryChannel, WheelZone};
use ui_core::geometry::Rect;
use ui_core::gesture::GestureEvent;
use ui_core::id::NodeId;
use ui_kit::tokens::TextRoleKind;

use super::knob_row::{RowTarget, RulerRow};
use super::segments::{SegmentEvent, SegmentRow, SegmentSpec, push_if_activated};
use super::selection_haptic::play_selection_haptic;
use super::space::SectionSpace;
use super::wheels::tab_entries;
use crate::action::ScreenAction;
use crate::calibration::BarsFrames;
use crate::widget::error::WidgetError;
use crate::widget::intent::{ButtonId, ScreenGesture};
use crate::widget::kit::SceneKit;
use crate::widget::layers::{DrawLayers, VisualIndex};
use crate::widget::text_runs::TextRuns;
use crate::widget::{Ctx, Outputs};

/// Вид BARS.
pub(in crate::widget::calibration) struct BarsView {
    tabs: SegmentRow<WheelZone>,
    /// Строки R, G, B по порядку `PrimaryChannel::ALL`.
    rows: Vec<RulerRow>,
    /// Доступны ли вкладки касаниям (факты есть).
    enabled: bool,
    /// Высота содержимого вида: ровно высота окна колонки (вид не прокручивается).
    content_height_pt: f64,
}

impl BarsView {
    /// Строит вид внутри узла содержимого `content_node` (рамка `content`: внутренняя ширина колонки × высота окна).
    pub(in crate::widget::calibration) fn build(
        kit: &mut SceneKit,
        ctx: &Ctx<'_>,
        space: &SectionSpace,
        (content_node, content): (NodeId, Rect),
        frames: BarsFrames,
    ) -> Result<Self, WidgetError> {
        let tabs = SegmentRow::build(
            kit,
            ctx,
            (content_node, content),
            frames.tabs,
            SegmentSpec {
                entries: &tab_entries(),
                node_label: "calib_bars_tab",
                // Те же нажатия, что у вкладок колёс: вкладка одна на оба вида.
                id_of: ButtonId::CalibrationWheelTab,
                text_role: TextRoleKind::SegmentLabel,
            },
        )?;
        let mut rows = Vec::with_capacity(PrimaryChannel::ALL.len());
        for (channel, row_frame) in PrimaryChannel::ALL.into_iter().zip(frames.rows) {
            rows.push(RulerRow::build(
                kit,
                ctx,
                space,
                (content_node, content),
                (RowTarget::Primary(channel), row_frame),
            )?);
        }
        Ok(Self {
            tabs,
            rows,
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
        // Щелчок выбора — только когда вкладка СМЕНИЛАСЬ (как в виде колёс).
        if let SegmentEvent::Activated(zone) = &tab_event
            && *zone != ctx.state.calibration().selected_wheel()
        {
            play_selection_haptic(ctx, out);
        }
        // Выбор вкладки — то же действие, что у колёс: состояние зоны общее.
        if push_if_activated(tab_event, out, ScreenAction::WheelSelected) {
            return Ok(true);
        }
        for row in &mut self.rows {
            if row.handle_gesture(kit, space, ctx, event, out)? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub(in crate::widget::calibration) fn sync(
        &mut self,
        kit: &mut SceneKit,
        ctx: &Ctx<'_>,
    ) -> Result<(), WidgetError> {
        self.enabled = ctx.state.look_facts().is_some();
        self.tabs.set_touchable(kit, self.enabled)?;
        for row in &mut self.rows {
            row.sync(kit, ctx)?;
        }
        Ok(())
    }

    pub(in crate::widget::calibration) fn advance(&mut self, ctx: &Ctx<'_>, out: &mut Outputs) {
        for row in &mut self.rows {
            row.advance(ctx, out);
        }
    }

    /// Узел строки со слотом `slot`, если она из этого вида: остаётся видимой под пальцем (решение 273).
    pub(in crate::widget::calibration) fn row_node(&self, slot: u8) -> Option<NodeId> {
        self.rows.iter().find(|row| row.slot() == slot).map(RulerRow::node)
    }

    /// Высота содержимого, pt: окно целиком, прокручивать нечего.
    pub(in crate::widget::calibration) fn content_height(&self) -> f64 {
        self.content_height_pt
    }

    pub(in crate::widget::calibration) fn invalid_values(&self) -> u64 {
        self.rows.iter().map(RulerRow::invalid_values).sum()
    }

    pub(in crate::widget::calibration) fn orphan_pan_continuations(&self) -> u64 {
        self.rows.iter().map(RulerRow::orphan_pan_continuations).sum()
    }

    pub(in crate::widget::calibration) fn suppressed_echo_events(&self) -> u64 {
        self.rows.iter().map(RulerRow::suppressed_echo_events).sum()
    }

    /// Положение линейки строки `slot` в шагах (проба для тестов).
    #[cfg(test)]
    pub(in crate::widget::calibration) fn row_position_steps(
        &self,
        slot: u8,
        now: ui_core::time::MonotonicNanos,
    ) -> Option<f64> {
        self.rows
            .iter()
            .find(|row| row.slot() == slot)
            .map(|row| row.position_steps(now))
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
        let selected = ctx.state.calibration().selected_wheel();
        let dim = if self.enabled { 1.0 } else { config.disabled_row_opacity };
        // Кольцо на вкладке: хоть одна из трёх полосок зоны отличается от пресета.
        let marked = |zone: WheelZone| {
            facts.is_some_and(|facts| {
                PrimaryChannel::ALL
                    .into_iter()
                    .any(|channel| facts.control_differs(&LookControl::Primary { zone, channel }))
            })
        };
        self.tabs.draw(ctx, (visuals, texts), layers, (selected, marked, dim))?;
        for row in &self.rows {
            row.draw(ctx, visuals, layers, texts)?;
        }
        Ok(())
    }
}

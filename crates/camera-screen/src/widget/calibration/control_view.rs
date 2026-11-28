//! Вид органов управления колонки ручек — шов «вид можно заменить» (сессия 14b2: цветовые колёса).
//!
//! Колонка ручек не знает, КАК показаны ручки: она даёт виду область под шапкой, прокрутку и жесты, а вид строит
//! свои узлы и рисует. Видов три: строки-линейки ([`RowsView`]), колёса цвета ([`WheelsView`]) и полоски каналов ([`BarsView`]); какой из них — решает
//! состояние раздела (`ControlViewKind`), смена вида пересобирает раздел по `SectionKey`.

use look_contract::{BandAxis, BandFacts, GlobalKnob};
use ui_core::geometry::Rect;
use ui_core::gesture::GestureEvent;
use ui_core::id::NodeId;
use ui_kit::tokens::TextRoleKind;

use super::band_dots::BandDots;
use super::bars::BarsView;
use super::finger_focus::DrivenControl;
use super::knob_row::{RowTarget, RulerRow};
use super::paint::NodePaint;
use super::space::SectionSpace;
use super::wheels::WheelsView;
use crate::calibration::CalibrationWidgetsConfig;
use crate::widget::error::WidgetError;
use crate::widget::intent::ScreenGesture;
use crate::widget::kit::SceneKit;
use crate::widget::layers::{DrawLayers, Layer, VisualIndex};
use crate::widget::text_runs::TextRuns;
use crate::widget::{Ctx, Outputs};

/// Какая часть содержимого видна: окно прокрутки в системе содержимого.
#[derive(Debug, Clone, Copy)]
pub(super) struct VisibleWindow {
    /// Смещение прокрутки, pt.
    pub offset_pt: f64,
    /// Высота окна, pt.
    pub height_pt: f64,
    /// Прозрачность «остального» под пальцем (решение 273): 1 в покое. Заголовки групп рисуются в системе узла
    /// содержимого — предка ведомой строки, поэтому сцена их не растворяет, и множитель берут они сами.
    pub others_opacity: f32,
}

impl VisibleWindow {
    /// Виден ли хоть краешек блока `[top, top + height]` (в системе содержимого). Блок у края рисуется целиком и
    /// режется окном (`DrawLayers::clipped_to`, решение 227); блок целиком снаружи не рисуется вовсе.
    pub(super) fn shows(&self, top_pt: f64, height_pt: f64) -> bool {
        let rel_top = top_pt - self.offset_pt;
        rel_top < self.height_pt && rel_top + height_pt > 0.0
    }
}

/// Вид органов управления.
pub(super) enum ControlView {
    /// Группы строк-линеек: TONE, COLOR, BANDS.
    Rows(RowsView),
    /// Колесо цвета с вкладками (решения 204–207): без прокрутки.
    /// В коробке: вид колёс (линейка кольца) заметно больше строк, а вариант живёт один на колонку.
    Wheels(Box<WheelsView>),
    /// Три полоски каналов под вкладками зон (решение 343): без прокрутки.
    Bars(Box<BarsView>),
    /// Вид колёс или полосок не поместился в колонку (низкая поверхность): зона пуста, без круга и жестов колёс. Шапка и
    /// переключатель живут, поэтому из этого вида можно выйти обратно в SCALES.
    Unavailable { window_height_pt: f64 },
}

impl ControlView {
    /// Высота содержимого, pt.
    pub(super) fn content_height(&self) -> f64 {
        match self {
            ControlView::Rows(rows) => rows.content_height,
            ControlView::Wheels(wheels) => wheels.content_height(),
            ControlView::Bars(bars) => bars.content_height(),
            ControlView::Unavailable { window_height_pt } => *window_height_pt,
        }
    }

    /// Узлы органа управления `driven`, которые остаются видимыми под пальцем (решение 273); пусто — орган не
    /// из этого вида.
    pub(super) fn focus_nodes(&self, driven: DrivenControl) -> Vec<NodeId> {
        match (self, driven) {
            (ControlView::Rows(rows), DrivenControl::Row(slot)) => rows
                .rows
                .iter()
                .find(|row| row.slot() == slot)
                .map(|row| vec![row.node()])
                .unwrap_or_default(),
            (ControlView::Wheels(wheels), DrivenControl::Wheels) => wheels.focus_nodes(),
            (ControlView::Bars(bars), DrivenControl::Row(slot)) => bars.row_node(slot).into_iter().collect(),
            _ => Vec::new(),
        }
    }

    pub(super) fn handle_gesture(
        &mut self,
        kit: &SceneKit,
        space: &SectionSpace,
        ctx: &Ctx<'_>,
        event: &GestureEvent<ScreenGesture>,
        out: &mut Outputs,
    ) -> Result<bool, WidgetError> {
        match self {
            ControlView::Rows(rows) => rows.handle_gesture(kit, space, ctx, event, out),
            ControlView::Wheels(wheels) => wheels.handle_gesture(kit, space, ctx, event, out),
            ControlView::Bars(bars) => bars.handle_gesture(kit, space, ctx, event, out),
            ControlView::Unavailable { .. } => Ok(false),
        }
    }

    pub(super) fn sync(&mut self, kit: &mut SceneKit, ctx: &Ctx<'_>) -> Result<(), WidgetError> {
        match self {
            ControlView::Rows(rows) => rows.sync(kit, ctx),
            ControlView::Wheels(wheels) => wheels.sync(kit, ctx),
            ControlView::Bars(bars) => bars.sync(kit, ctx),
            ControlView::Unavailable { .. } => Ok(()),
        }
    }

    pub(super) fn advance(&mut self, ctx: &Ctx<'_>, out: &mut Outputs) {
        match self {
            ControlView::Rows(rows) => rows.advance(ctx, out),
            ControlView::Wheels(wheels) => wheels.advance(ctx, out),
            ControlView::Bars(bars) => bars.advance(ctx, out),
            ControlView::Unavailable { .. } => {}
        }
    }

    pub(super) fn draw(
        &self,
        ctx: &Ctx<'_>,
        visuals: &VisualIndex,
        layers: &mut DrawLayers,
        texts: &mut TextRuns,
        window: VisibleWindow,
    ) -> Result<(), WidgetError> {
        match self {
            ControlView::Rows(rows) => rows.draw(ctx, visuals, layers, texts, window),
            // Вид колёс не прокручивается: окно списка ему не нужно.
            ControlView::Wheels(wheels) => wheels.draw(ctx, visuals, layers, texts),
            ControlView::Bars(bars) => bars.draw(ctx, visuals, layers, texts),
            ControlView::Unavailable { .. } => Ok(()),
        }
    }

    /// Продолжения паноров без начала во всех органах вида (диагностика, на нормальном пути 0).
    pub(super) fn orphan_pan_continuations(&self) -> u64 {
        match self {
            ControlView::Rows(rows) => rows.rows.iter().map(RulerRow::orphan_pan_continuations).sum(),
            ControlView::Wheels(wheels) => wheels.orphan_pan_continuations(),
            ControlView::Bars(bars) => bars.orphan_pan_continuations(),
            ControlView::Unavailable { .. } => 0,
        }
    }

    /// Сколько значений ручек не стали значениями вида (диагностика).
    pub(super) fn invalid_values(&self) -> u64 {
        match self {
            ControlView::Rows(rows) => rows.rows.iter().map(RulerRow::invalid_values).sum(),
            ControlView::Wheels(wheels) => wheels.invalid_values(),
            ControlView::Bars(bars) => bars.invalid_values(),
            ControlView::Unavailable { .. } => 0,
        }
    }

    pub(super) fn suppressed_echo_events(&self) -> u64 {
        match self {
            ControlView::Rows(rows) => rows.rows.iter().map(RulerRow::suppressed_echo_events).sum(),
            ControlView::Wheels(wheels) => wheels.suppressed_echo_events(),
            ControlView::Bars(bars) => bars.suppressed_echo_events(),
            ControlView::Unavailable { .. } => 0,
        }
    }

    /// Положение линейки строки `slot` в шагах (проба для тестов).
    #[cfg(test)]
    pub(super) fn row_position_steps(&self, slot: u8, now: ui_core::time::MonotonicNanos) -> Option<f64> {
        match self {
            ControlView::Rows(rows) => rows
                .rows
                .iter()
                .find(|row| row.slot() == slot)
                .map(|row| row.position_steps(now)),
            ControlView::Wheels(wheels) => wheels.row_position_steps(slot, now),
            ControlView::Bars(bars) => bars.row_position_steps(slot, now),
            ControlView::Unavailable { .. } => None,
        }
    }
}

/// Заголовок группы: подпись и верх блока в системе содержимого.
struct GroupHeader {
    title: &'static str,
    top_pt: f64,
}

/// Строки-линейки: группы TONE, COLOR и BANDS.
pub(super) struct RowsView {
    headers: Vec<GroupHeader>,
    rows: Vec<RulerRow>,
    bands: BandDots,
    /// Рамка содержимого (система корня): от неё считаются смещения заголовков.
    content: Rect,
    /// Узел содержимого: заголовки групп рисуются в его системе.
    content_node: NodeId,
    content_height: f64,
}

/// Пары «заголовок группы — ручки» в порядке показа.
const KNOB_GROUPS: [(&str, [GlobalKnob; 4]); 2] = [
    (
        "TONE",
        [
            GlobalKnob::Contrast,
            GlobalKnob::Midtones,
            GlobalKnob::Shadows,
            GlobalKnob::Highlights,
        ],
    ),
    (
        "COLOR",
        [
            GlobalKnob::Saturation,
            GlobalKnob::Vibrance,
            GlobalKnob::Warmth,
            GlobalKnob::Tint,
        ],
    ),
];

impl RowsView {
    /// Высота содержимого, pt: заранее известна по конфигу (колонка создаёт узел содержимого до вида).
    pub(super) fn planned_height(config: &CalibrationWidgetsConfig) -> f64 {
        let knob_group = config.group_header_height_pt + 4.0 * config.row_height_pt;
        let bands_group = config.group_header_height_pt + config.band_dots_height_pt + 3.0 * config.row_height_pt;
        2.0 * knob_group + bands_group + config.panel_padding_pt
    }

    /// Строит вид внутри узла содержимого `content_node`; `content` — его рамка (система корня) высотой
    /// [`Self::planned_height`], ширина — внутренняя ширина колонки.
    pub(super) fn build(
        kit: &mut SceneKit,
        ctx: &Ctx<'_>,
        space: &SectionSpace,
        (content_node, content): (NodeId, Rect),
        bands: &[BandFacts],
    ) -> Result<Self, WidgetError> {
        let config = &ctx.config.calibration.widgets;
        let mut headers = Vec::new();
        let mut rows = Vec::new();
        let mut top = 0.0;
        let row_rect =
            |top: f64, height: f64| Rect::from_xywh(content.min_x(), content.min_y() + top, content.size.width, height);
        for (title, knobs) in KNOB_GROUPS {
            headers.push(GroupHeader { title, top_pt: top });
            top += config.group_header_height_pt;
            for knob in knobs {
                rows.push(RulerRow::build(
                    kit,
                    ctx,
                    space,
                    (content_node, content),
                    (RowTarget::Knob(knob), row_rect(top, config.row_height_pt)),
                )?);
                top += config.row_height_pt;
            }
        }
        headers.push(GroupHeader {
            title: "BANDS",
            top_pt: top,
        });
        top += config.group_header_height_pt;
        let dots = BandDots::build(
            kit,
            ctx,
            (content_node, content),
            row_rect(top, config.band_dots_height_pt),
            bands,
        )?;
        top += config.band_dots_height_pt;
        for axis in BandAxis::ALL {
            rows.push(RulerRow::build(
                kit,
                ctx,
                space,
                (content_node, content),
                (RowTarget::BandAxis(axis), row_rect(top, config.row_height_pt)),
            )?);
            top += config.row_height_pt;
        }
        let content_height = top + config.panel_padding_pt;
        debug_assert!(
            (content_height - Self::planned_height(config)).abs() < 1e-9,
            "высота содержимого разошлась с планом: {content_height} против {}",
            Self::planned_height(config)
        );
        Ok(Self {
            headers,
            rows,
            bands: dots,
            content,
            content_node,
            content_height,
        })
    }

    fn handle_gesture(
        &mut self,
        kit: &SceneKit,
        space: &SectionSpace,
        ctx: &Ctx<'_>,
        event: &GestureEvent<ScreenGesture>,
        out: &mut Outputs,
    ) -> Result<bool, WidgetError> {
        for row in &mut self.rows {
            if row.handle_gesture(kit, space, ctx, event, out)? {
                return Ok(true);
            }
        }
        self.bands.handle_gesture(kit, ctx, event, out)
    }

    fn sync(&mut self, kit: &mut SceneKit, ctx: &Ctx<'_>) -> Result<(), WidgetError> {
        for row in &mut self.rows {
            row.sync(kit, ctx)?;
        }
        self.bands.sync(kit, ctx)
    }

    fn advance(&mut self, ctx: &Ctx<'_>, out: &mut Outputs) {
        for row in &mut self.rows {
            row.advance(ctx, out);
        }
    }

    fn draw(
        &self,
        ctx: &Ctx<'_>,
        visuals: &VisualIndex,
        layers: &mut DrawLayers,
        texts: &mut TextRuns,
        window: VisibleWindow,
    ) -> Result<(), WidgetError> {
        let config = &ctx.config.calibration.widgets;
        let content_top = self.content.min_y();
        if let Some(placement) = visuals.placement(self.content_node) {
            for header in &self.headers {
                if !window.shows(header.top_pt, config.group_header_height_pt) {
                    continue;
                }
                let Some(paint) = NodePaint::new(ctx, placement, window.others_opacity)? else {
                    continue;
                };
                let run = texts.run(ctx, header.title, TextRoleKind::Caption)?;
                // Центр заголовка от центра рамки содержимого.
                let y_pt = header.top_pt + config.group_header_height_pt / 2.0 - self.content.size.height / 2.0;
                layers.push(
                    Layer::Content,
                    paint.text_left(run, -self.content.size.width / 2.0, y_pt, ctx.tokens.colors.panel_label),
                );
            }
        }
        for row in &self.rows {
            if window.shows(row.frame().min_y() - content_top, config.row_height_pt) {
                row.draw(ctx, visuals, layers, texts)?;
            }
        }
        if window.shows(self.bands.frame().min_y() - content_top, config.band_dots_height_pt) {
            self.bands.draw(ctx, visuals, layers)?;
        }
        Ok(())
    }
}

//! Колонка ручек слева (решения 185, 199, 203): стеклянная панель, под ней видоискатель; сверху шапка (основа тона
//! и «чистяк»), ниже свободно прокручиваемый список органов управления.
//!
//! Прокрутка списка — `ui_kit::scroll_list::ScrollList`: палец ведёт содержимое, узел содержимого сдвигается по
//! вертикали на смещение, а хит-тест `ui-core` сам учитывает сдвиг. Узел окна списка ограничивает касания его
//! рамкой (дети за рамкой родителя не ловят), поэтому строки, уехавшие под шапку или за низ панели, нажать нельзя.
//! Рисование рамками не ограничено, поэтому строки у края гаснут по доле видимой части (`VisibleWindow::fade_of`).

use look_contract::BandFacts;
use ui_core::geometry::Rect;
use ui_core::gesture::{GestureEvent, PanSpec, RecognizerSpec};
use ui_core::id::NodeId;
use ui_draw::draw_list::{DrawItem, PressState};
use ui_kit::pan::PanUpdate;
use ui_kit::scroll_list::ScrollList;
use ui_kit::tokens::GlassRole;
use ui_kit::view::GlassShape;

use super::control_view::{ControlView, RowsView, VisibleWindow};
use super::space::{EyeAxis, SectionSpace};
use super::tone_header::ToneHeader;
use crate::widget::error::{WidgetError, element};
use crate::widget::intent::ScreenGesture;
use crate::widget::kit::SceneKit;
use crate::widget::layers::{DrawLayers, Layer, VisualIndex};
use crate::widget::text_runs::TextRuns;
use crate::widget::{Ctx, Outputs, single_member_group};

/// Колонка ручек.
pub(super) struct KnobsColumn {
    panel: NodeId,
    header: ToneHeader,
    content_node: NodeId,
    list: ScrollList,
    view: ControlView,
    /// Высота окна списка, pt.
    window_height_pt: f64,
}

impl KnobsColumn {
    /// Строит колонку в `parent`; `panel_rect` — рамка стеклянной панели в системе корня раздела.
    pub(super) fn build(
        kit: &mut SceneKit,
        ctx: &Ctx<'_>,
        space: &SectionSpace,
        (parent, parent_rect): (NodeId, Rect),
        (panel_rect, bands): (Rect, &[BandFacts]),
    ) -> Result<Self, WidgetError> {
        let config = &ctx.config.calibration.widgets;
        let panel = kit.add_node(parent, "calib_knobs_panel", panel_rect, parent_rect)?;
        let padding = config.panel_padding_pt;
        let inner_width = (panel_rect.size.width - 2.0 * padding).max(0.0);
        let header_rect = Rect::from_xywh(
            panel_rect.min_x() + padding,
            panel_rect.min_y() + padding,
            inner_width,
            config.header_height_pt,
        );
        let header = ToneHeader::build(kit, ctx, space, (panel, panel_rect), header_rect)?;
        // Окно списка — вся ширина панели под шапкой; содержимое уже на отступы и едет внутри окна.
        let window_rect = Rect::from_xywh(
            panel_rect.min_x(),
            header_rect.max_y(),
            panel_rect.size.width,
            (panel_rect.max_y() - header_rect.max_y()).max(0.0),
        );
        let list_node = kit.add_node(panel, "calib_knobs_window", window_rect, panel_rect)?;
        kit.add_recognizer(
            list_node,
            RecognizerSpec::Pan(PanSpec {
                direction: space.pan_direction(EyeAxis::Vertical),
                ..PanSpec::default()
            }),
            ScreenGesture::CalibrationListPan,
        )?;
        let content_rect = Rect::from_xywh(
            panel_rect.min_x() + padding,
            window_rect.min_y(),
            inner_width,
            RowsView::planned_height(config),
        );
        let content_node = kit.add_node(list_node, "calib_knobs_content", content_rect, window_rect)?;
        let rows = RowsView::build(kit, ctx, space, (content_node, content_rect), bands)?;
        let view = ControlView::Rows(rows);
        let list = ScrollList::new(
            view.content_height(),
            &ctx.environment(window_rect.size.height),
            ctx.now,
        )
        .map_err(element("список ручек"))?;
        Ok(Self {
            panel,
            header,
            content_node,
            list,
            view,
            window_height_pt: window_rect.size.height,
        })
    }

    /// Смещение прокрутки списка, pt (проба для тестов).
    #[cfg(test)]
    pub(super) fn scroll_offset(&self, now: ui_core::time::MonotonicNanos) -> f64 {
        self.list.offset(now)
    }

    /// Положение линейки строки `slot` в шагах (проба для тестов).
    #[cfg(test)]
    pub(super) fn row_position_steps(&self, slot: u8, now: ui_core::time::MonotonicNanos) -> Option<f64> {
        self.view.row_position_steps(slot, now)
    }

    pub(super) fn invalid_values(&self) -> u64 {
        self.view.invalid_values()
    }

    pub(super) fn suppressed_echo_events(&self) -> u64 {
        self.view.suppressed_echo_events()
    }

    pub(super) fn handle_gesture(
        &mut self,
        kit: &SceneKit,
        space: &SectionSpace,
        ctx: &Ctx<'_>,
        event: &GestureEvent<ScreenGesture>,
        out: &mut Outputs,
    ) -> Result<bool, WidgetError> {
        if event.intent == ScreenGesture::CalibrationListPan {
            if let Some(update) = PanUpdate::from_gesture(event.state, event.data).map_err(element("пан списка"))?
            {
                self.list
                    .apply_pan(event.time, space.eye_pan(update))
                    .map_err(element("список ручек"))?;
            }
            return Ok(true);
        }
        Ok(self.header.handle_gesture(kit, space, ctx, event, out)?
            || self.view.handle_gesture(kit, space, ctx, event, out)?)
    }

    pub(super) fn sync(&mut self, kit: &mut SceneKit, ctx: &Ctx<'_>) -> Result<(), WidgetError> {
        // Содержимое едет за списком: смещение вверх — узел вниз по системе корня с минусом.
        kit.jump_vertically_to(self.content_node, -self.list.offset(ctx.now))?;
        self.header.sync(kit, ctx)?;
        self.view.sync(kit, ctx)
    }

    pub(super) fn advance(&mut self, ctx: &Ctx<'_>, out: &mut Outputs) {
        self.header.advance(ctx, out);
        self.view.advance(ctx, out);
    }

    pub(super) fn draw(
        &self,
        ctx: &Ctx<'_>,
        visuals: &VisualIndex,
        layers: &mut DrawLayers,
        texts: &mut TextRuns,
    ) -> Result<(), WidgetError> {
        let Some(placement) = visuals.placement(self.panel) else {
            return Ok(());
        };
        let (group, _host) = single_member_group(
            ctx,
            GlassRole::Panel,
            GlassShape::RoundedRect {
                corner_radius_pt: ctx.config.calibration.widgets.panel_corner_radius_pt,
            },
            &placement,
            PressState::RELEASED,
            "calib_knobs_panel",
        )?;
        layers.push(Layer::Glass, DrawItem::Glass(group));
        self.header.draw(ctx, visuals, layers, texts)?;
        self.view.draw(
            ctx,
            visuals,
            layers,
            texts,
            VisibleWindow {
                offset_pt: self.list.offset(ctx.now),
                height_pt: self.window_height_pt,
            },
        )
    }
}

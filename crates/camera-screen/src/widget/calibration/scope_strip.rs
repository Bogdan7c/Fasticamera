//! Полоска над картинкой прибора (решения 258, 290, 291): имя прибора слева, точки страниц по центру (тап выбирает
//! прибор), на странице вектороскопа справа переключатель осей Y'CbCr | OKLab, а на самом правом краю каждой
//! страницы — капсула белого (`WhiteCapsule`, место `ScopeStripFrames::trailing`).
//!
//! Раньше имя и точки лежали поверх следа и закрывали света и тени; теперь у них своя полоска, а картинка прибора
//! начинается под ней. Все цели — на всю высоту полоски (44 pt, решение 291), соседние по ширине не перекрываются
//! (границы дают `ScopeStripFrames`, их проверяет тест соседства).
//!
//! Владение: какой прибор, какие оси и какое увеличение выбраны, решает `CalibrationState`; полоска только показывает и
//! шлёт намерения `ScopeKindSelected` / `VectorscopeAxesSelected` / `VectorscopeZoomSelected`. Переключатель осей и
//! кнопка «2×» (решение 325) живы только на странице вектороскопа (`sync`): на остальных страницах они не рисуются и
//! не ловят касаний.

use scope_contract::{ScopeKind, VectorscopeAxes, VectorscopeZoom};
use ui_core::geometry::Rect;
use ui_core::gesture::GestureEvent;
use ui_core::id::NodeId;
use ui_kit::tokens::TextRoleKind;

use super::paint::NodePaint;
use super::segments::{SegmentRow, SegmentSpec, push_if_activated};
use super::white_capsule::WhiteCapsule;
use crate::action::ScreenAction;
use crate::calibration::ScopeStripFrames;
use crate::widget::button::{ButtonEvent, PressButton};
use crate::widget::error::WidgetError;
use crate::widget::intent::{ButtonId, ScreenGesture};
use crate::widget::kit::SceneKit;
use crate::widget::layers::{DrawLayers, Layer, VisualIndex};
use crate::widget::text_runs::TextRuns;
use crate::widget::{Ctx, Outputs};

/// Подпись прибора заглавными.
fn scope_title(kind: ScopeKind) -> &'static str {
    match kind {
        ScopeKind::Waveform => "RGB WAVEFORM",
        ScopeKind::LumaWaveform => "LUMA WAVEFORM",
        ScopeKind::RgbParade => "RGB PARADE",
        ScopeKind::Vectorscope => "VECTORSCOPE",
    }
}

/// Сегменты переключателя осей в порядке показа (тексты UI английские).
const AXES_SEGMENTS: [(VectorscopeAxes, &str); 2] = [
    (VectorscopeAxes::YCbCrBt709, "Y'CbCr"),
    (VectorscopeAxes::OklabAb, "OKLab"),
];

/// Кнопка «2×»: один сегмент; горит, когда вектороскоп увеличен, тап переключает увеличение туда и обратно.
const ZOOM_SEGMENTS: [(VectorscopeZoom, &str); 1] = [(VectorscopeZoom::Double, "2×")];

/// Точка страницы: её прибор и кнопка.
struct PageDot {
    kind: ScopeKind,
    button: PressButton,
}

/// Полоска прибора.
pub(super) struct ScopeStrip {
    node: NodeId,
    frame: Rect,
    frames: ScopeStripFrames,
    dots: Vec<PageDot>,
    axes: SegmentRow<VectorscopeAxes>,
    /// Кнопка «2×» вектороскопа.
    zoom: SegmentRow<VectorscopeZoom>,
    /// Капсула белого справа на каждой странице.
    white: WhiteCapsule,
}

impl ScopeStrip {
    /// Строит полоску внутри `parent`; `strip` — её рамка в системе корня раздела.
    pub(super) fn build(
        kit: &mut SceneKit,
        ctx: &Ctx<'_>,
        (parent, parent_rect): (NodeId, Rect),
        strip: Rect,
    ) -> Result<Self, WidgetError> {
        let frames = ScopeStripFrames::of(strip, &ctx.config.calibration.widgets);
        let node = kit.add_node(parent, "calib_scope_strip", strip, parent_rect)?;
        let mut dots = Vec::with_capacity(ScopeKind::ALL.len());
        for (kind, cell) in ScopeKind::ALL.into_iter().zip(frames.page_dots) {
            let dot_node = kit.add_node(node, "calib_scope_page_dot", cell, strip)?;
            // Зона касания точки — её ячейка: расширять до 44 pt по ширине нельзя, соседи встык.
            let button = PressButton::attach(kit, ButtonId::ScopePageDot(kind), dot_node, cell, ctx)?;
            dots.push(PageDot { kind, button });
        }
        let axes = SegmentRow::build(
            kit,
            ctx,
            (node, strip),
            frames.axes_toggle,
            SegmentSpec {
                entries: &AXES_SEGMENTS,
                node_label: "calib_scope_axes",
                id_of: ButtonId::ScopeAxes,
                text_role: TextRoleKind::SegmentLabel,
            },
        )?;
        let zoom = SegmentRow::build(
            kit,
            ctx,
            (node, strip),
            frames.zoom_toggle,
            SegmentSpec {
                entries: &ZOOM_SEGMENTS,
                node_label: "calib_scope_zoom",
                id_of: ButtonId::ScopeZoom,
                text_role: TextRoleKind::SegmentLabel,
            },
        )?;
        let white = WhiteCapsule::build(kit, ctx, (node, strip), frames.trailing)?;
        let built = Self {
            node,
            frame: strip,
            frames,
            dots,
            axes,
            zoom,
            white,
        };
        // До первой сверки переключатель осей и «2×» не ловят касаний (страница по умолчанию — не вектороскоп).
        built.axes.set_touchable(kit, false)?;
        built.zoom.set_touchable(kit, false)?;
        Ok(built)
    }

    /// Тап по точке страницы и по сегменту осей. `false` — событие чужое.
    pub(super) fn handle_gesture(
        &mut self,
        kit: &SceneKit,
        ctx: &Ctx<'_>,
        event: &GestureEvent<ScreenGesture>,
        out: &mut Outputs,
    ) -> Result<bool, WidgetError> {
        if let Some(dot) = self
            .dots
            .iter_mut()
            .find(|dot| event.intent == ScreenGesture::Press(ButtonId::ScopePageDot(dot.kind)))
        {
            let data = kit.gesture_data_in_node(dot.button.node(), event.data);
            if dot.button.handle(event.state, data, event.time)? == ButtonEvent::Activated
                && ctx.state.calibration().scope_kind() != dot.kind
            {
                out.actions.push(ScreenAction::ScopeKindSelected(dot.kind));
            }
            return Ok(true);
        }
        if self.white.handle_gesture(kit, event, out)? {
            return Ok(true);
        }
        let zoom_event = self.zoom.handle_gesture(kit, event)?;
        // Один сегмент «2×»: тап по нему переключает увеличение (включено — выключить, и наоборот).
        let current = ctx.state.calibration().vectorscope_zoom();
        if push_if_activated(zoom_event, out, |_| {
            ScreenAction::VectorscopeZoomSelected(current.toggled())
        }) {
            return Ok(true);
        }
        let event = self.axes.handle_gesture(kit, event)?;
        Ok(push_if_activated(event, out, ScreenAction::VectorscopeAxesSelected))
    }

    /// Переключатель осей ловит касания только на странице вектороскопа.
    pub(super) fn sync(&mut self, kit: &mut SceneKit, ctx: &Ctx<'_>) -> Result<(), WidgetError> {
        let on_vectorscope = ctx.state.calibration().scope_kind() == ScopeKind::Vectorscope;
        self.zoom.set_touchable(kit, on_vectorscope)?;
        self.axes.set_touchable(kit, on_vectorscope)
    }

    pub(super) fn draw(
        &self,
        ctx: &Ctx<'_>,
        visuals: &VisualIndex,
        layers: &mut DrawLayers,
        texts: &mut TextRuns,
    ) -> Result<(), WidgetError> {
        let kind = ctx.state.calibration().scope_kind();
        self.draw_title_and_hairline(ctx, visuals, layers, texts, kind)?;
        self.draw_page_dots(ctx, visuals, layers, kind)?;
        if kind == ScopeKind::Vectorscope {
            let selected = ctx.state.calibration().vectorscope_axes();
            self.axes
                .draw(ctx, (visuals, texts), layers, (selected, |_| false, 1.0))?;
            let zoom = ctx.state.calibration().vectorscope_zoom();
            self.zoom.draw(ctx, (visuals, texts), layers, (zoom, |_| false, 1.0))?;
        }
        self.white.draw(ctx, visuals, layers, texts)?;
        Ok(())
    }

    /// Имя прибора слева и волосяная линия под полоской (граница с картинкой).
    fn draw_title_and_hairline(
        &self,
        ctx: &Ctx<'_>,
        visuals: &VisualIndex,
        layers: &mut DrawLayers,
        texts: &mut TextRuns,
        kind: ScopeKind,
    ) -> Result<(), WidgetError> {
        let Some(placement) = visuals.placement(self.node) else {
            return Ok(());
        };
        let Some(paint) = NodePaint::new(ctx, placement, 1.0)? else {
            return Ok(());
        };
        let colors = &ctx.tokens.colors;
        let title = texts.run(ctx, scope_title(kind), TextRoleKind::Caption)?;
        let centre = self.frame.center();
        layers.push(
            Layer::Content,
            paint.text_left(title, self.frames.title.min_x() - centre.x, 0.0, colors.panel_label),
        );
        // Линия толщиной в один физический пиксель вдоль нижнего края полоски: картинка прибора начинается ровно под ней.
        let hairline_pt = 1.0 / f64::from(ctx.px.get());
        layers.push(
            Layer::Content,
            paint.capsule(
                (0.0, (self.frame.size.height - hairline_pt) / 2.0),
                (self.frame.size.width, hairline_pt),
                colors
                    .text_primary
                    .with_alpha(ctx.config.calibration.widgets.scope_strip_hairline_alpha),
            ),
        );
        Ok(())
    }

    /// Точки страниц: текущая яркая, остальные приглушены.
    fn draw_page_dots(
        &self,
        ctx: &Ctx<'_>,
        visuals: &VisualIndex,
        layers: &mut DrawLayers,
        selected: ScopeKind,
    ) -> Result<(), WidgetError> {
        let config = &ctx.config.calibration.widgets;
        let colors = &ctx.tokens.colors;
        for dot in &self.dots {
            let Some(placement) = visuals.placement(dot.button.node()) else {
                continue;
            };
            let Some(paint) = NodePaint::new(ctx, placement, 1.0)? else {
                continue;
            };
            let color = if dot.kind == selected {
                colors.text_primary
            } else {
                colors.text_muted.with_alpha(config.scope_page_dot_dim_alpha)
            };
            layers.push(
                Layer::Content,
                paint.disc(0.0, 0.0, config.scope_page_dot_diameter_pt, color),
            );
        }
        Ok(())
    }
}

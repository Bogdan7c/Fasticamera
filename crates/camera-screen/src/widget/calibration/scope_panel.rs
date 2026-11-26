//! Зона прибора справа снизу (решение 203): стеклянная панель, поверх неё картинка прибора с GPU, подпись
//! прибора и точки-страницы. Горизонтальный свайп (в глазных осях) листает приборы.
//!
//! Картинку просят размером зоны в пикселях (`CalibrationLayout::scope_image_size_for`: вся зона или вписанный
//! квадрат вектороскопа), поэтому она кладётся 1:1 по центру зоны без фильтрации: `ImageFit::Stretch`, без обрезки, в системе повёрнутого корня раздела.
//! Листание не зацикливается: за первым и последним прибором свайп ничего не делает (точки-страницы читаются
//! как линейный ряд) — `[invented]`.

use scope_contract::ScopeKind;
use ui_core::geometry::Rect;
use ui_core::gesture::{GestureData, GestureEvent, GestureState, PanSpec, RecognizerSpec};
use ui_core::id::NodeId;
use ui_draw::draw_list::{DrawItem, ImageClip, ImageFit, ImageItem, ImageSource, LiveImage, PressState};
use ui_draw::geometry::PxSize;
use ui_kit::tokens::{GlassRole, TextRoleKind};
use ui_kit::view::GlassShape;

use super::paint::NodePaint;
use super::space::{EyeAxis, SectionSpace};
use crate::action::ScreenAction;
use crate::widget::error::WidgetError;
use crate::widget::intent::ScreenGesture;
use crate::widget::kit::SceneKit;
use crate::widget::layers::{DrawLayers, Layer, VisualIndex};
use crate::widget::text_runs::TextRuns;
use crate::widget::{Ctx, Outputs, single_member_group};

/// Подпись прибора заглавными.
fn scope_title(kind: ScopeKind) -> &'static str {
    match kind {
        ScopeKind::Waveform => "RGB WAVEFORM",
        ScopeKind::RgbParade => "RGB PARADE",
        ScopeKind::Vectorscope => "VECTORSCOPE",
    }
}

/// Соседний прибор в линейном ряду: `+1` — следующий, `-1` — предыдущий; за краем ряда — `None`.
pub(super) fn neighbour_scope(current: ScopeKind, step: i32) -> Option<ScopeKind> {
    let index = ScopeKind::ALL.iter().position(|kind| *kind == current)?;
    let target = i32::try_from(index).ok()?.checked_add(step)?;
    usize::try_from(target)
        .ok()
        .and_then(|i| ScopeKind::ALL.get(i))
        .copied()
}

/// Зона прибора.
pub(super) struct ScopePanel {
    node: NodeId,
    frame: Rect,
}

impl ScopePanel {
    pub(super) fn build(
        kit: &mut SceneKit,
        space: &SectionSpace,
        (parent, parent_rect): (NodeId, Rect),
        frame: Rect,
    ) -> Result<Self, WidgetError> {
        let node = kit.add_node(parent, "calib_scope", frame, parent_rect)?;
        kit.add_recognizer(
            node,
            RecognizerSpec::Pan(PanSpec {
                direction: space.pan_direction(EyeAxis::Horizontal),
                ..PanSpec::default()
            }),
            ScreenGesture::CalibrationScopePan,
        )?;
        Ok(Self { node, frame })
    }

    /// Свайп: решается по концу жеста — достаточный сдвиг глазами влево листает к следующему прибору, вправо — к
    /// предыдущему. Отменённый жест ничего не меняет.
    pub(super) fn handle_gesture(
        &mut self,
        space: &SectionSpace,
        ctx: &Ctx<'_>,
        event: &GestureEvent<ScreenGesture>,
        out: &mut Outputs,
    ) -> bool {
        if event.intent != ScreenGesture::CalibrationScopePan {
            return false;
        }
        if let (GestureState::Ended, GestureData::Pan { translation, .. }) = (event.state, event.data) {
            let eye_dx = space.eye_vector(translation.from_touch_down).dx;
            let step = if eye_dx <= -ctx.config.calibration.widgets.scope_swipe_min_pt {
                1
            } else if eye_dx >= ctx.config.calibration.widgets.scope_swipe_min_pt {
                -1
            } else {
                0
            };
            if step != 0
                && let Some(next) = neighbour_scope(ctx.state.calibration().scope_kind(), step)
            {
                out.actions.push(ScreenAction::ScopeKindSelected(next));
            }
        }
        true
    }

    pub(super) fn draw(
        &self,
        ctx: &Ctx<'_>,
        visuals: &VisualIndex,
        layers: &mut DrawLayers,
        texts: &mut TextRuns,
    ) -> Result<(), WidgetError> {
        let Some(placement) = visuals.placement(self.node) else {
            return Ok(());
        };
        let config = &ctx.config.calibration.widgets;
        let (group, _host) = single_member_group(
            ctx,
            GlassRole::Panel,
            GlassShape::RoundedRect {
                corner_radius_pt: config.panel_corner_radius_pt,
            },
            &placement,
            PressState::RELEASED,
            "calib_scope",
        )?;
        layers.push(Layer::Glass, DrawItem::Glass(group));
        let Some(paint) = NodePaint::new(ctx, placement, 1.0)? else {
            return Ok(());
        };
        if let Some(layout) = ctx.state.calibration().layout() {
            // Картинка нужного вида (waveform — вся зона, вектороскоп — квадрат) стоит по центру зоны: узел зоны —
            // её центр, `paint.at(0, 0)`. Размер тот же, что в запросе прибора, поэтому она кладётся 1:1.
            let size = layout.scope_image_size_for(ctx.state.calibration().scope_kind());
            layers.push(
                Layer::Content,
                DrawItem::Image(ImageItem {
                    source: ImageSource::Live(LiveImage::ColorScope),
                    transform: paint.at(0.0, 0.0),
                    size_px: PxSize::new(size.width() as f32, size.height() as f32),
                    fit: ImageFit::Stretch,
                    clip: ImageClip::None,
                    opacity: paint.opacity(),
                }),
            );
        }
        self.draw_labels(ctx, &paint, layers, texts)
    }

    /// Подпись прибора сверху и точки-страницы снизу; текущая страница ярче.
    fn draw_labels(
        &self,
        ctx: &Ctx<'_>,
        paint: &NodePaint,
        layers: &mut DrawLayers,
        texts: &mut TextRuns,
    ) -> Result<(), WidgetError> {
        let config = &ctx.config.calibration.widgets;
        let colors = &ctx.tokens.colors;
        let kind = ctx.state.calibration().scope_kind();
        let half_height = self.frame.size.height / 2.0;
        let title = texts.run(ctx, scope_title(kind), TextRoleKind::Caption)?;
        let title_y = -half_height + config.scope_label_inset_pt + f64::from(title.cap_height_px / ctx.px.get()) / 2.0;
        layers.push(
            Layer::Content,
            paint.text_centered(title, 0.0, title_y, colors.panel_label),
        );
        let count = ScopeKind::ALL.len();
        let pitch = config.scope_page_dot_diameter_pt + config.scope_page_dot_gap_pt;
        let dots_y = half_height - config.scope_label_inset_pt - config.scope_page_dot_diameter_pt / 2.0;
        for (index, page) in ScopeKind::ALL.iter().enumerate() {
            let x = (index as f64 - (count as f64 - 1.0) / 2.0) * pitch;
            let color = if *page == kind {
                colors.text_primary
            } else {
                colors.text_muted.with_alpha(0.5)
            };
            layers.push(
                Layer::Content,
                paint.disc(x, dots_y, config.scope_page_dot_diameter_pt, color),
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn neighbours_walk_the_linear_row_and_stop_at_the_ends() {
        assert_eq!(neighbour_scope(ScopeKind::Waveform, 1), Some(ScopeKind::RgbParade));
        assert_eq!(neighbour_scope(ScopeKind::RgbParade, 1), Some(ScopeKind::Vectorscope));
        assert_eq!(neighbour_scope(ScopeKind::Vectorscope, 1), None);
        assert_eq!(neighbour_scope(ScopeKind::Waveform, -1), None);
        assert_eq!(neighbour_scope(ScopeKind::Vectorscope, -1), Some(ScopeKind::RgbParade));
    }
}

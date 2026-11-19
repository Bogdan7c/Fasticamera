//! Панель Exposure (`motion.md` 4.1): капсула режимов растворяется, на её месте растёт стеклянная панель (K7
//! `PanelMorph`) с вертикальной подписью EXPOSURE, числом над горизонтальной шкалой (K3 `SnapScale`, шаг 1/3),
//! кнопкой «×» и кнопкой сброса (после первого изменения). Панель однострочная и помещается между кольцом
//! затвора и нижним краем экрана (см. `ExposurePanelLayout`): затвор остаётся видимым и нажимаемым.

use ui_core::geometry::Rect;
use ui_core::gesture::{GestureEvent, PanDirection, PanSpec, RecognizerSpec};
use ui_core::id::NodeId;
use ui_draw::color::Opacity;
use ui_draw::draw_list::DrawItem;
use ui_draw::icons::IconId;
use ui_draw::scene_bridge::NodePlacementPx;
use ui_draw::transform::Affine2;
use ui_kit::drive::{GrowthDirection, ScrollTransition};
use ui_kit::pan::PanUpdate;
use ui_kit::panel_morph::{PanelMorph, PanelMorphSpec, PanelPhase};
use ui_kit::scale::{ScaleSpec, SnapScale, TickVisibility};
use ui_kit::tokens::{GlassRole, ScaleDensity, TextRoleKind};
use ui_kit::value::{QuantizedLinear, ValueModel};
use ui_kit::view::{centered_text, panel_glass_group, scale_items};

use super::button::{ButtonEvent, PressButton};
use super::error::{WidgetError, element};
use super::exposure_link::{FingerDrive, differs_from_state, exposure_model, format_ev, linear_limits, state_value_ev};
use super::geometry::ExposureButtonCorner;
use super::intent::{ButtonId, ScreenGesture};
use super::kit::SceneKit;
use super::layers::{DrawLayers, Layer, VisualIndex};
use super::text_runs::TextRuns;
use super::{Ctx, Outputs, draw_plain_icon_button};
use crate::action::ScreenAction;
use crate::state::PanelState;

pub(crate) struct ExposurePanelWidget {
    panel_node: NodeId,
    /// Узел ползунка: его размещение отдаёт виду шкалы центр и длину рамки.
    slider_node: NodeId,
    close: PressButton,
    close_content: NodeId,
    reset: PressButton,
    reset_content: NodeId,
    morph: PanelMorph,
    scale: SnapScale<QuantizedLinear>,
    finger: FingerDrive,
    open_wanted: bool,
    reset_shown: bool,
}

impl ExposurePanelWidget {
    pub(crate) fn build(kit: &mut SceneKit, ctx: &Ctx<'_>, parent: NodeId) -> Result<Self, WidgetError> {
        let panel_rect = ctx.geometry.exposure_panel();
        let panel_node = kit.add_node(parent, "exposure_panel", panel_rect, ctx.geometry.screen())?;
        let slider_rect = ctx.geometry.exposure_slider();
        let slider_node = kit.add_node(panel_node, "exposure_slider", slider_rect, panel_rect)?;
        kit.add_recognizer(
            slider_node,
            RecognizerSpec::Pan(PanSpec {
                direction: PanDirection::Horizontal,
                ..PanSpec::default()
            }),
            ScreenGesture::ExposureSliderPan,
        )?;
        let close_rect = ctx.geometry.exposure_button(ExposureButtonCorner::Close);
        let close_node = kit.add_node(panel_node, "exposure_close", close_rect, panel_rect)?;
        let close = PressButton::attach(kit, ButtonId::ExposureClose, close_node, close_rect, ctx)?;
        let close_content = kit.add_rotating_content(close_node, "exposure_close_icon")?;
        let reset_rect = ctx.geometry.exposure_button(ExposureButtonCorner::Reset);
        let reset_node = kit.add_node(panel_node, "exposure_reset", reset_rect, panel_rect)?;
        let reset = PressButton::attach(kit, ButtonId::ExposureReset, reset_node, reset_rect, ctx)?;
        let reset_content = kit.add_rotating_content(reset_node, "exposure_reset_icon")?;
        kit.set_interactive(panel_node, false)?;

        let spec = PanelMorphSpec::new(
            ctx.geometry.capsule(),
            panel_rect,
            ctx.geometry.config().exposure_panel.corner_radius_pt,
        );
        let morph =
            PanelMorph::new(spec, &ctx.environment(panel_rect.size.width), ctx.now).map_err(element("панель EV"))?;
        let scale = Self::new_scale(ctx, slider_rect)?;
        Ok(Self {
            panel_node,
            slider_node,
            close,
            close_content,
            reset,
            reset_content,
            morph,
            scale,
            finger: FingerDrive::default(),
            open_wanted: false,
            reset_shown: false,
        })
    }

    fn new_scale(ctx: &Ctx<'_>, slider_rect: Rect) -> Result<SnapScale<QuantizedLinear>, WidgetError> {
        let spec = ScaleSpec {
            direction: GrowthDirection::FingerLeft,
            density: ScaleDensity::Dense,
            ticks: TickVisibility::Visible,
            ..ScaleSpec::default()
        };
        SnapScale::new(
            exposure_model(ctx)?,
            state_value_ev(ctx),
            spec,
            &ctx.environment(slider_rect.size.width),
            ctx.now,
        )
        .map_err(element("шкала EV"))
    }

    pub(crate) fn handle_gesture(
        &mut self,
        event: &GestureEvent<ScreenGesture>,
        out: &mut Outputs,
    ) -> Result<bool, WidgetError> {
        match event.intent {
            ScreenGesture::ExposureSliderPan => {
                if let Some(update) = PanUpdate::from_gesture(event.state, event.data).map_err(element("пан EV"))? {
                    if matches!(update, PanUpdate::Began { .. }) {
                        self.finger.finger_took_over();
                    }
                    self.scale.apply_pan(event.time, update).map_err(element("шкала EV"))?;
                }
            }
            ScreenGesture::Press(ButtonId::ExposureClose) => {
                if self.close.handle(event.state, event.data, event.time)? == ButtonEvent::Activated {
                    out.actions.push(ScreenAction::ExposurePanelCloseRequested);
                }
            }
            ScreenGesture::Press(ButtonId::ExposureReset) => {
                if self.reset.handle(event.state, event.data, event.time)? == ButtonEvent::Activated {
                    out.actions.push(ScreenAction::ExposureResetRequested);
                }
            }
            _ => return Ok(false),
        }
        Ok(true)
    }

    pub(crate) fn sync(&mut self, kit: &mut SceneKit, ctx: &Ctx<'_>) -> Result<(), WidgetError> {
        let open = ctx.state.panel() == PanelState::ExposurePanel;
        if open != self.open_wanted {
            self.open_wanted = open;
            if open {
                self.morph.open(ctx.now).map_err(element("панель EV"))?;
            } else {
                self.morph.close(ctx.now).map_err(element("панель EV"))?;
            }
            kit.set_interactive(self.panel_node, open)?;
        }
        let reset_shown = open && ctx.state.exposure().steps() != 0;
        if reset_shown != self.reset_shown {
            self.reset_shown = reset_shown;
            kit.set_interactive(self.reset.node(), reset_shown)?;
        }
        // Пределы камеры изменились: шкала переезжает на новые (значение шкалы остаётся).
        if self.scale.model().limits() != &linear_limits(ctx) {
            self.scale
                .set_limits(linear_limits(ctx), ScrollTransition::Animated, ctx.now)
                .map_err(element("шкала EV"))?;
            self.finger.program_took_over();
        }
        // Шкала разошлась с состоянием, а палец её не держит: едем к состоянию (без эха и хаптики).
        let touched = self.finger.is_finger_driven();
        if !touched && differs_from_state(ctx, self.scale.value(ctx.now)) {
            self.finger.program_took_over();
            self.scale
                .set_value(state_value_ev(ctx), ScrollTransition::Animated, ctx.now)
                .map_err(element("шкала EV"))?;
        }
        Ok(())
    }

    pub(crate) fn advance(&mut self, ctx: &Ctx<'_>, out: &mut Outputs) -> Result<(), WidgetError> {
        self.morph.advance(ctx.now);
        // События открытия/закрытия нужны только дрожащему состоянию панели; состояние экрана — в `ScreenState`.
        let _lifecycle = self.morph.drain_events();
        self.scale.advance(ctx.now);
        let events = self.scale.drain_events();
        self.finger.translate(events, ctx, out);
        Ok(())
    }

    /// Прозрачность капсулы режимов не здесь: её растворяет сама капсула по состоянию панели.
    pub(crate) fn draw(
        &self,
        ctx: &Ctx<'_>,
        visuals: &VisualIndex,
        layers: &mut DrawLayers,
        texts: &mut TextRuns,
    ) -> Result<(), WidgetError> {
        let snapshot = self.morph.snapshot(ctx.now);
        if snapshot.phase == PanelPhase::Closed {
            return Ok(());
        }
        let group = panel_glass_group(&snapshot, ctx.tokens.glass.style_for(GlassRole::Panel), ctx.px)?;
        layers.push(Layer::Glass, DrawItem::Glass(group));
        let alpha = snapshot.content_opacity.clamp(0.0, 1.0);
        let opacity =
            Opacity::new(alpha).map_err(|_| WidgetError::Inconsistent("прозрачность панели вне 0..1".to_owned()))?;
        let slide = ctx.px.px(snapshot.content_offset_pt);
        let colors = &ctx.tokens.colors;
        let at = |x_pt: f64, y_pt: f64| Affine2::translate(ctx.px.px(x_pt), ctx.px.px(y_pt) + slide);

        let title = texts.run(ctx, "EXPOSURE", TextRoleKind::Caption)?;
        let title_at = ctx.geometry.exposure_title_center();
        // Подпись вертикальная (`behavior.md` 2.1): читается снизу вверх, поэтому поворот на −90° до сдвига.
        let title_host = Affine2::rotate(-std::f32::consts::FRAC_PI_2).then(at(title_at.x, title_at.y));
        layers.push(
            Layer::Content,
            centered_text(title, title_host, colors.text_muted, opacity),
        );
        let ev = ctx.state.exposure().bias(&ctx.config.exposure).ev();
        let value = texts.run(ctx, &format_ev(ev), TextRoleKind::ScaleValue)?;
        let value_at = ctx.geometry.exposure_value_center();
        layers.push(
            Layer::Content,
            centered_text(value, at(value_at.x, value_at.y), colors.text_primary, opacity),
        );
        self.draw_ticks(ctx, visuals, layers, alpha, slide)?;

        let panel_config = &ctx.geometry.config().exposure_panel;
        let icon_size = panel_config.button_diameter_pt * panel_config.button_icon_scale;
        if let (Some(button), Some(content)) = (
            visuals.placement(self.close.node()),
            visuals.placement(self.close_content),
        ) {
            draw_plain_icon_button(ctx, layers, &button, &content, IconId::Close, icon_size, alpha, slide)?;
        }
        if self.reset_shown
            && let (Some(button), Some(content)) = (
                visuals.placement(self.reset.node()),
                visuals.placement(self.reset_content),
            )
        {
            draw_plain_icon_button(ctx, layers, &button, &content, IconId::Reset, icon_size, alpha, slide)?;
        }
        Ok(())
    }

    /// Риски шкалы и неподвижная метка текущего значения: рисует `ui_kit::view::scale_items` по размещению
    /// узла ползунка (центр рамки = текущее значение, риски за рамкой отсекаются самим видом).
    /// `content_alpha` и `slide_px` — прозрачность и сдвиг содержимого панели из морфа.
    fn draw_ticks(
        &self,
        ctx: &Ctx<'_>,
        visuals: &VisualIndex,
        layers: &mut DrawLayers,
        content_alpha: f32,
        slide_px: f32,
    ) -> Result<(), WidgetError> {
        let Some(node_placement) = visuals.placement(self.slider_node) else {
            return Ok(());
        };
        let placement = NodePlacementPx {
            transform: Affine2::translate(0.0, slide_px).then(node_placement.transform),
            opacity: (node_placement.opacity * content_alpha).clamp(0.0, 1.0),
            ..node_placement
        };
        layers.extend(
            Layer::Content,
            scale_items(
                &placement,
                &self.scale,
                ctx.now,
                ctx.px,
                &ctx.tokens.scale_ticks,
                &ctx.tokens.colors,
            )?,
        );
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn slider_value_ev(&self, now: ui_core::time::MonotonicNanos) -> f64 {
        self.scale.value(now)
    }

    pub(crate) fn invalid_bias_values(&self) -> u64 {
        self.finger.invalid_bias_values()
    }

    pub(crate) fn suppressed_echo_events(&self) -> u64 {
        self.finger.suppressed_echo_events()
    }
}

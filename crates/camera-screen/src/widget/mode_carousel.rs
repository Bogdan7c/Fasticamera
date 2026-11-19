//! Капсула режимов (K4 `SnapCarousel`): стеклянная капсула 220 × 47,8 pt с подписями режимов из реестра.
//!
//! В v1 один режим (PHOTO, решение 80): горизонтальный жест даёт «резинку» без смены режима, тап по текущему
//! режиму открывает меню Controls (`ModeSelected` того же режима, решает редьюсер).

use ui_core::geometry::Point;
use ui_core::gesture::{GestureData, GestureEvent, GestureState, PanDirection, PanSpec, RecognizerSpec, TapSpec};
use ui_core::id::NodeId;
use ui_draw::draw_list::{DrawItem, PressState};
use ui_kit::carousel::{CarouselEntry, CarouselEvent, CarouselSpec, SnapCarousel, TapOutcome};
use ui_kit::drive::ScrollTransition;
use ui_kit::pan::PanUpdate;
use ui_kit::tokens::{GlassRole, MotionToken};
use ui_kit::view::{CarouselLabels, GlassShape, LabelPainter, carousel_items, opacity_of_placement};

use super::error::{WidgetError, element};
use super::intent::ScreenGesture;
use super::kit::SceneKit;
use super::layers::{DrawLayers, Layer, VisualIndex};
use super::{Ctx, Outputs, single_member_group};
use crate::action::ScreenAction;
use crate::modes::CaptureMode;
use crate::state::PanelState;

pub(crate) struct ModeCarouselWidget {
    node: NodeId,
    carousel: SnapCarousel<CaptureMode>,
    /// Подписи, свёрстанные один раз при сборке сцены, в порядке записей реестра.
    labels: CarouselLabels,
    /// К какой видимости едем (`None` — ещё не назначали): капсула растворяется, пока открыта панель Exposure.
    shown: Option<bool>,
}

impl ModeCarouselWidget {
    pub(crate) fn build(kit: &mut SceneKit, ctx: &Ctx<'_>, parent: NodeId) -> Result<Self, WidgetError> {
        let frame = ctx.geometry.capsule();
        let node = kit.add_node(parent, "mode_capsule", frame, ctx.geometry.screen())?;
        kit.add_recognizer(node, RecognizerSpec::Tap(TapSpec::default()), ScreenGesture::CapsuleTap)?;
        for (direction, intent) in [
            (PanDirection::Horizontal, ScreenGesture::ModesPan),
            (PanDirection::Up, ScreenGesture::MenuOpenPan),
        ] {
            kit.add_recognizer(
                node,
                RecognizerSpec::Pan(PanSpec {
                    direction,
                    ..PanSpec::default()
                }),
                intent,
            )?;
        }
        let painter = LabelPainter::new(&ctx.tokens.typography, ctx.px);
        let descriptors = ctx.state.mode_carousel();
        let texts: Vec<&str> = descriptors.iter().map(|descriptor| descriptor.label).collect();
        let labels = CarouselLabels::layout(&painter, &texts)?;
        let mut entries = Vec::with_capacity(descriptors.len());
        for (index, descriptor) in descriptors.iter().enumerate() {
            let advance_px = labels
                .advance_px(index)
                .ok_or_else(|| WidgetError::Inconsistent(format!("для режима {index} нет свёрстанной подписи")))?;
            entries.push(CarouselEntry {
                item: descriptor.mode,
                width_points: f64::from(advance_px / ctx.px.get()),
            });
        }
        let environment = ctx.environment(frame.size.width);
        let carousel = SnapCarousel::new(
            entries,
            &ctx.state.mode(),
            CarouselSpec::default(),
            &environment,
            ctx.now,
        )
        .map_err(element("карусель режимов"))?;
        Ok(Self {
            node,
            carousel,
            labels,
            shown: None,
        })
    }

    pub(crate) fn handle_gesture(
        &mut self,
        event: &GestureEvent<ScreenGesture>,
        ctx: &Ctx<'_>,
        out: &mut Outputs,
    ) -> Result<bool, WidgetError> {
        match event.intent {
            ScreenGesture::ModesPan => {
                if let Some(update) =
                    PanUpdate::from_gesture(event.state, event.data).map_err(element("пан режимов"))?
                {
                    self.carousel
                        .apply_pan(event.time, update)
                        .map_err(element("карусель режимов"))?;
                }
            }
            ScreenGesture::MenuOpenPan => {
                // Меню открывается по завершению жеста: отменённый (разрыв потока) ничего не делает.
                if event.state == GestureState::Ended {
                    out.actions.push(ScreenAction::ControlsMenuOpenRequested);
                }
            }
            ScreenGesture::CapsuleTap => {
                if let (GestureState::Ended, GestureData::Tap { position, .. }) = (event.state, event.data) {
                    self.tapped_at(position, ctx, event, out)?;
                }
            }
            _ => return Ok(false),
        }
        Ok(true)
    }

    /// Тап по капсуле в точке `position` (pt): режим под пальцем, а если мимо подписей — текущий.
    fn tapped_at(
        &mut self,
        position: Point,
        ctx: &Ctx<'_>,
        event: &GestureEvent<ScreenGesture>,
        out: &mut Outputs,
    ) -> Result<(), WidgetError> {
        let center_x = ctx.geometry.capsule().center().x;
        let index = self
            .carousel
            .placements(event.time)
            .into_iter()
            .find(|placement| {
                (position.x - (center_x + placement.center_offset_points)).abs() <= placement.width_points * 0.5
            })
            .map_or_else(|| self.carousel.selected_index(), |placement| placement.index);
        let outcome = self
            .carousel
            .tapped_item(index, event.time)
            .map_err(element("карусель режимов"))?;
        if outcome == TapOutcome::AlreadySelected {
            // Выбор того же режима — «открой меню»; смена режима придёт событием `SelectionChanged`.
            out.actions.push(ScreenAction::ModeSelected(*self.carousel.selected()));
        }
        Ok(())
    }

    pub(crate) fn sync(&mut self, kit: &mut SceneKit, ctx: &Ctx<'_>) -> Result<(), WidgetError> {
        let wanted_index = self
            .carousel
            .entries()
            .iter()
            .position(|entry| entry.item == ctx.state.mode());
        if let Some(index) = wanted_index
            && index != self.carousel.selected_index()
        {
            self.carousel
                .select(index, ScrollTransition::Animated, ctx.now)
                .map_err(element("карусель режимов"))?;
        }
        let visible = ctx.state.panel() != PanelState::ExposurePanel;
        if self.shown != Some(visible) {
            self.shown = Some(visible);
            kit.set_interactive(self.node, visible)?;
            kit.fade_to(
                self.node,
                if visible { 1.0 } else { 0.0 },
                ctx.spring(MotionToken::Smooth)?,
                ctx.now,
            )?;
        }
        Ok(())
    }

    pub(crate) fn advance(&mut self, out: &mut Outputs) {
        for event in self.carousel.drain_events() {
            match event {
                CarouselEvent::SelectionChanged {
                    item,
                    cause: ui_kit::carousel::SelectionCause::Swipe | ui_kit::carousel::SelectionCause::Tap,
                    ..
                } => out.actions.push(ScreenAction::ModeSelected(item)),
                // Программный выбор — следствие состояния экрана, обратно действием не отдаётся (без эха).
                CarouselEvent::SelectionChanged { .. } => {}
                CarouselEvent::Haptic { pattern } => out.haptics.push(pattern),
            }
        }
    }

    pub(crate) fn draw(
        &self,
        ctx: &Ctx<'_>,
        visuals: &VisualIndex,
        layers: &mut DrawLayers,
    ) -> Result<(), WidgetError> {
        let Some(placement) = visuals.placement(self.node) else {
            return Ok(());
        };
        if placement.opacity <= 0.0 {
            return Ok(());
        }
        let (group, host) = single_member_group(
            ctx,
            GlassRole::Button,
            GlassShape::Capsule,
            &placement,
            PressState::RELEASED,
            "mode_capsule",
        )?;
        layers.push(Layer::Glass, DrawItem::Glass(group));
        let opacity = opacity_of_placement(&placement, "mode_labels")?;
        // Подписи ставит вид `ui-kit`: выбранная — `selected_label`, остальные — приглушённые.
        layers.extend(
            Layer::Content,
            carousel_items(
                host,
                opacity,
                &self.carousel,
                ctx.now,
                &self.labels,
                ctx.px,
                &ctx.tokens.colors,
            )?,
        );
        Ok(())
    }
}

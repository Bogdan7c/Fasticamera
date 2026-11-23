//! Шапка колонки ручек: переключатель основы тона (свайп-карусель AGX / ACES / FILMIC / LEGACY) со своей «↻»
//! и кнопка «чистяк» (удержание показывает основу без вида).
//!
//! «Чистяк» — выдумка владельца-идиомы (решение 189): значка «глаз» в наборе нет, поэтому это капсула с
//! подписью CLEAN. Нажатие даёт `CleanViewPressed`, отпускание или отмена — `CleanViewReleased`. Залипнуть кнопка
//! не может: если шапку собрали заново (поворот, пересборка сцены), пока состояние говорит «держится», палец
//! прежней кнопки потерян, и первый же `advance` новой шапки один раз отдаёт `CleanViewReleased`.
//!
//! Проверка — ровно один раз после сборки, а не на каждом кадре: удержание, начатое не пальцем этой кнопки
//! (сценарий, тест, другой хост действием `CleanViewPressed`), законно и не должно отпускаться само.

use look_contract::{LookControl, ToneBaseModel};
use ui_core::geometry::Rect;
use ui_core::gesture::{GestureEvent, GestureState, PanSpec, RecognizerSpec};
use ui_core::id::NodeId;
use ui_draw::icons::IconId;
use ui_kit::carousel::{CarouselEntry, CarouselEvent, CarouselSpec, SelectionCause, SnapCarousel};
use ui_kit::drive::ScrollTransition;
use ui_kit::pan::PanUpdate;
use ui_kit::tokens::TextRoleKind;
use ui_kit::view::{CarouselLabels, LabelPainter, carousel_items_in_window, centered_icon};

use super::paint::NodePaint;
use super::space::{EyeAxis, SectionSpace};
use crate::action::ScreenAction;
use crate::calibration::CleanViewState;
use crate::widget::button::PressButton;
use crate::widget::error::{WidgetError, element};
use crate::widget::intent::{ButtonId, ScreenGesture};
use crate::widget::kit::SceneKit;
use crate::widget::layers::{DrawLayers, Layer, VisualIndex};
use crate::widget::text_runs::TextRuns;
use crate::widget::{Ctx, Outputs};

/// Номер «↻» основы тона среди строк раздела (после восьми ручек и трёх осей полосы).
pub(super) const TONE_BASE_RESET_SLOT: u8 = 11;

/// Подпись модели основы заглавными.
fn base_title(model: ToneBaseModel) -> &'static str {
    match model {
        ToneBaseModel::Agx => "AGX",
        ToneBaseModel::Aces => "ACES",
        ToneBaseModel::Filmic => "FILMIC",
        ToneBaseModel::Legacy => "LEGACY",
    }
}

/// Рамки частей шапки в системе корня раздела.
#[derive(Debug, Clone, Copy)]
struct HeaderFrames {
    carousel: Rect,
    reset: Rect,
    clean: Rect,
}

impl HeaderFrames {
    /// Слева направо: карусель на всё свободное, «↻», «чистяк» у правого края; всё по центру высоты шапки.
    fn of(header: Rect, ctx: &Ctx<'_>) -> Self {
        let config = &ctx.config.calibration.widgets;
        let centered = |x: f64, width: f64, height: f64| {
            Rect::from_xywh(x, header.min_y() + (header.size.height - height) / 2.0, width, height)
        };
        let clean_width = config.clean_button_width_pt;
        let clean = centered(header.max_x() - clean_width, clean_width, config.clean_button_height_pt);
        let reset_side = config.reset_button_diameter_pt;
        let reset = centered(
            clean.min_x() - config.header_gap_pt - reset_side,
            reset_side,
            reset_side,
        );
        let carousel_width = (reset.min_x() - config.header_gap_pt - header.min_x()).max(0.0);
        Self {
            carousel: Rect::from_xywh(header.min_x(), header.min_y(), carousel_width, header.size.height),
            reset,
            clean,
        }
    }
}

/// Шапка колонки.
pub(super) struct ToneHeader {
    frames: HeaderFrames,
    carousel_node: NodeId,
    carousel: SnapCarousel<ToneBaseModel>,
    labels: CarouselLabels,
    reset: PressButton,
    clean: PressButton,
    /// Палец сейчас держит «чистяк».
    clean_held: bool,
    /// Шапка только что собрана и ещё не сверилась с состоянием: удержание, пережившее пересборку, осиротело.
    orphan_hold_check_pending: bool,
    /// Основа из фактов, к которой карусель уже поехала (или которую выбрал сам палец).
    last_followed: Option<ToneBaseModel>,
    enabled: bool,
}

impl ToneHeader {
    pub(super) fn build(
        kit: &mut SceneKit,
        ctx: &Ctx<'_>,
        space: &SectionSpace,
        (parent, parent_rect): (NodeId, Rect),
        header: Rect,
    ) -> Result<Self, WidgetError> {
        let frames = HeaderFrames::of(header, ctx);
        let carousel_node = kit.add_node(parent, "calib_base_carousel", frames.carousel, parent_rect)?;
        kit.add_recognizer(
            carousel_node,
            RecognizerSpec::Pan(PanSpec {
                direction: space.pan_direction(EyeAxis::Horizontal),
                ..PanSpec::default()
            }),
            ScreenGesture::CalibrationBasePan,
        )?;
        let painter = LabelPainter::new(&ctx.tokens.typography, ctx.px);
        let titles: Vec<&str> = ToneBaseModel::ALL.iter().map(|model| base_title(*model)).collect();
        let labels = CarouselLabels::layout(&painter, &titles)?;
        let mut entries = Vec::with_capacity(titles.len());
        for (index, model) in ToneBaseModel::ALL.iter().enumerate() {
            let advance_px = labels
                .advance_px(index)
                .ok_or_else(|| WidgetError::Inconsistent(format!("для основы {index} нет свёрстанной подписи")))?;
            entries.push(CarouselEntry {
                item: *model,
                width_points: f64::from(advance_px / ctx.px.get()),
            });
        }
        let selected = ctx
            .state
            .look_facts()
            .map_or(ToneBaseModel::Agx, |facts| facts.current.base);
        let carousel = SnapCarousel::new(
            entries,
            &selected,
            CarouselSpec::default(),
            &ctx.environment(frames.carousel.size.width),
            ctx.now,
        )
        .map_err(element("карусель основы"))?;
        let reset_node = kit.add_node(parent, "calib_base_reset", frames.reset, parent_rect)?;
        let reset = PressButton::attach(
            kit,
            ButtonId::CalibrationReset(TONE_BASE_RESET_SLOT),
            reset_node,
            frames.reset,
            ctx,
        )?;
        kit.ensure_min_touch_side(
            reset_node,
            frames.reset,
            ctx.geometry.config().card.button_touch_side_pt,
        )?;
        let clean_node = kit.add_node(parent, "calib_clean", frames.clean, parent_rect)?;
        let clean = PressButton::attach(kit, ButtonId::CleanView, clean_node, frames.clean, ctx)?;
        kit.ensure_min_touch_side(
            clean_node,
            frames.clean,
            ctx.geometry.config().card.button_touch_side_pt,
        )?;
        Ok(Self {
            frames,
            carousel_node,
            carousel,
            labels,
            reset,
            clean,
            clean_held: false,
            orphan_hold_check_pending: true,
            last_followed: None,
            enabled: false,
        })
    }

    /// Жесты шапки: свайп основы, «↻» основы, «чистяк». `false` — событие чужое.
    pub(super) fn handle_gesture(
        &mut self,
        kit: &SceneKit,
        space: &SectionSpace,
        ctx: &Ctx<'_>,
        event: &GestureEvent<ScreenGesture>,
        out: &mut Outputs,
    ) -> Result<bool, WidgetError> {
        match event.intent {
            ScreenGesture::CalibrationBasePan => {
                if let Some(update) = PanUpdate::from_gesture(event.state, event.data).map_err(element("пан основы"))?
                    && self.enabled
                {
                    self.carousel
                        .apply_pan(event.time, space.eye_pan(update))
                        .map_err(element("карусель основы"))?;
                }
            }
            ScreenGesture::Press(ButtonId::CalibrationReset(slot)) if slot == TONE_BASE_RESET_SLOT => {
                let data = kit.gesture_data_in_node(self.reset.node(), event.data);
                if self.reset.handle(event.state, data, event.time)? == crate::widget::button::ButtonEvent::Activated {
                    self.reset_if_differs(ctx, out);
                }
            }
            ScreenGesture::Press(ButtonId::CleanView) => self.handle_clean(kit, event, out)?,
            _ => return Ok(false),
        }
        Ok(true)
    }

    fn reset_if_differs(&self, ctx: &Ctx<'_>, out: &mut Outputs) {
        let differs = self.enabled
            && ctx
                .state
                .look_facts()
                .is_some_and(|facts| facts.control_differs(&LookControl::ToneBase));
        if differs {
            out.actions
                .push(ScreenAction::ResetControlTapped(LookControl::ToneBase));
        }
    }

    /// Удержание: палец лёг — показать основу, поднялся или жест отменён — вернуть вид с правками.
    fn handle_clean(
        &mut self,
        kit: &SceneKit,
        event: &GestureEvent<ScreenGesture>,
        out: &mut Outputs,
    ) -> Result<(), WidgetError> {
        let data = kit.gesture_data_in_node(self.clean.node(), event.data);
        self.clean.handle(event.state, data, event.time)?;
        match event.state {
            GestureState::Began if !self.clean_held => {
                self.clean_held = true;
                out.actions.push(ScreenAction::CleanViewPressed);
            }
            GestureState::Ended | GestureState::Cancelled => {
                self.clean_held = false;
                out.actions.push(ScreenAction::CleanViewReleased);
            }
            _ => {}
        }
        Ok(())
    }

    pub(super) fn sync(&mut self, kit: &mut SceneKit, ctx: &Ctx<'_>) -> Result<(), WidgetError> {
        let facts = ctx.state.look_facts();
        self.enabled = facts.is_some();
        kit.set_interactive(self.carousel_node, self.enabled)?;
        kit.set_interactive(self.reset.node(), self.enabled)?;
        let Some(facts) = facts else {
            return Ok(());
        };
        // Факты основы изменились (сброс, пресет): карусель едет программно, без действия.
        if self.last_followed != Some(facts.current.base) {
            self.last_followed = Some(facts.current.base);
            if let Some(index) = ToneBaseModel::ALL.iter().position(|model| *model == facts.current.base) {
                self.carousel
                    .select(index, ScrollTransition::Animated, ctx.now)
                    .map_err(element("карусель основы"))?;
            }
        }
        Ok(())
    }

    pub(super) fn advance(&mut self, ctx: &Ctx<'_>, out: &mut Outputs) {
        for event in self.carousel.drain_events() {
            match event {
                CarouselEvent::SelectionChanged {
                    item,
                    cause: SelectionCause::Swipe | SelectionCause::Tap,
                    ..
                } => {
                    // Выбор сделал палец: он уже «наш», повторный факт этой же основы ехать не заставит.
                    self.last_followed = Some(item);
                    out.actions.push(ScreenAction::ToneBaseSelected(item));
                }
                // Программный выбор — следствие фактов, обратно действием не уходит (без эха).
                CarouselEvent::SelectionChanged { .. } => {}
                CarouselEvent::Haptic { pattern } => out.haptics.push(pattern),
            }
        }
        // Первый кадр после сборки: «чистяк» держится по состоянию, а держал его палец прежней шапки — отпускаем,
        // чтобы вид не залип. Дальше удержание без пальца (действием) не трогаем.
        if std::mem::take(&mut self.orphan_hold_check_pending)
            && !self.clean_held
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
        let config = &ctx.config.calibration.widgets;
        let enabled_factor = if self.enabled { 1.0 } else { config.disabled_row_opacity };
        if let Some(placement) = visuals.placement(self.carousel_node)
            && let Some(paint) = NodePaint::new(ctx, placement, enabled_factor)?
        {
            // Соседние подписи не должны вылезать за переключатель на чистую зону превью: окно — его ширина.
            layers.extend(
                Layer::Content,
                carousel_items_in_window(
                    paint.at(0.0, 0.0),
                    paint.opacity(),
                    &self.carousel,
                    ctx.now,
                    &self.labels,
                    ctx.px,
                    &ctx.tokens.colors,
                    self.frames.carousel.size.width / 2.0,
                )?,
            );
        }
        self.draw_reset(ctx, visuals, layers, enabled_factor)?;
        self.draw_clean(ctx, visuals, layers, texts, enabled_factor)
    }

    fn draw_reset(
        &self,
        ctx: &Ctx<'_>,
        visuals: &VisualIndex,
        layers: &mut DrawLayers,
        enabled_factor: f32,
    ) -> Result<(), WidgetError> {
        let Some(placement) = visuals.placement(self.reset.node()) else {
            return Ok(());
        };
        let Some(paint) = NodePaint::new(ctx, placement, enabled_factor)? else {
            return Ok(());
        };
        let card = &ctx.geometry.config().card;
        let differs = ctx
            .state
            .look_facts()
            .is_some_and(|facts| facts.control_differs(&LookControl::ToneBase));
        let dim = if differs { 1.0 } else { card.reset_dim_opacity };
        let paint = paint.dimmed(dim);
        let side = ctx.config.calibration.widgets.reset_button_diameter_pt;
        layers.push(
            Layer::Content,
            paint.disc(
                0.0,
                0.0,
                side,
                ui_draw::color::DisplayColor {
                    a: card.button_disc_alpha,
                    ..ui_draw::color::DisplayColor::BLACK
                },
            ),
        );
        layers.push(
            Layer::Content,
            centered_icon(
                IconId::Reset,
                card.button_icon_size_pt * side / card.button_diameter_pt,
                ctx.px,
                paint.at(0.0, 0.0),
                ctx.tokens.colors.text_primary,
                paint.opacity(),
            ),
        );
        Ok(())
    }

    fn draw_clean(
        &self,
        ctx: &Ctx<'_>,
        visuals: &VisualIndex,
        layers: &mut DrawLayers,
        texts: &mut TextRuns,
        enabled_factor: f32,
    ) -> Result<(), WidgetError> {
        let Some(placement) = visuals.placement(self.clean.node()) else {
            return Ok(());
        };
        let Some(paint) = NodePaint::new(ctx, placement, enabled_factor)? else {
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
                (self.frames.clean.size.width, self.frames.clean.size.height),
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

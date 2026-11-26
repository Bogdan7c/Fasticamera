//! Шапка колонки ручек: переключатель основы тона (карусель AGX / ACES / FILMIC / LEGACY: свайп или тап по видимому
//! соседу, решение 229; тап по выбранной ничего не делает) со своей «↻».
//!
//! Кнопка CLEAN ушла вниз, к капсуле режимов (решение 259, `clean_button.rs`): освободившаяся ширина — карусели, и в
//! портрете рядом с выбранной основой видны соседи. Зона касания «↻» шире её рисунка (44 pt), поэтому карусель
//! кончается там, где начинается эта зона: общих точек у них нет.

use look_contract::{LookControl, ToneBaseModel};
use ui_core::geometry::{Point, Rect};
use ui_core::gesture::{GestureData, GestureEvent, GestureState, PanSpec, RecognizerSpec, TapSpec};
use ui_core::id::NodeId;
use ui_core::time::MonotonicNanos;
use ui_draw::icons::IconId;
use ui_kit::carousel::{CarouselEntry, CarouselEvent, CarouselSpec, SelectionCause, SnapCarousel};
use ui_kit::drive::ScrollTransition;
use ui_kit::pan::PanUpdate;
use ui_kit::tokens::ColorTokens;
use ui_kit::view::{CarouselLabels, LabelPainter, carousel_items_in_window, centered_icon};

use super::paint::NodePaint;
use super::space::{EyeAxis, SectionSpace};
use crate::action::ScreenAction;
use crate::widget::button::PressButton;
use crate::widget::error::{WidgetError, element};
use crate::widget::intent::{ButtonId, ScreenGesture};
use crate::widget::kit::SceneKit;
use crate::widget::layers::{DrawLayers, Layer, VisualIndex};
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

/// Цвета карусели основы: вид карусели (`ui-kit`) рисует соседей приглушённым текстом экрана, а на панели раздела
/// подписи читаемые (решение 228). Выбранная по-прежнему отличается цветом.
fn neighbour_label_colors(colors: &ColorTokens) -> ColorTokens {
    ColorTokens {
        text_muted: colors.panel_label,
        ..*colors
    }
}

/// Рамки частей шапки в системе корня раздела.
#[derive(Debug, Clone, Copy)]
struct HeaderFrames {
    carousel: Rect,
    reset: Rect,
}

impl HeaderFrames {
    /// Слева направо: карусель на всё свободное, «↻» у правого края; всё по центру высоты шапки.
    fn of(header: Rect, ctx: &Ctx<'_>) -> Self {
        let config = &ctx.config.calibration.widgets;
        let reset_side = config.reset_button_diameter_pt;
        let reset = Rect::from_xywh(
            header.max_x() - reset_side,
            header.min_y() + (header.size.height - reset_side) / 2.0,
            reset_side,
            reset_side,
        );
        // Зона касания «↻» выходит за её рисунок до `button_touch_side_pt`: карусель кончается перед этой зоной, а не
        // перед самим рисунком, иначе их области касания делили бы полосу.
        let reset_reach = ((ctx.geometry.config().card.button_touch_side_pt - reset_side) / 2.0).max(0.0);
        let gap = config.header_gap_pt.max(reset_reach);
        let carousel_width = (reset.min_x() - gap - header.min_x()).max(0.0);
        Self {
            carousel: Rect::from_xywh(header.min_x(), header.min_y(), carousel_width, header.size.height),
            reset,
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
        kit.add_recognizer(
            carousel_node,
            RecognizerSpec::Tap(TapSpec::default()),
            ScreenGesture::CalibrationBaseTap,
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
        Ok(Self {
            frames,
            carousel_node,
            carousel,
            labels,
            reset,
            last_followed: None,
            enabled: false,
        })
    }

    /// Жесты шапки: свайп основы и «↻» основы. `false` — событие чужое.
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
            ScreenGesture::CalibrationBaseTap => {
                // Отменённый тап (разрыв потока) и тап без фактов ничего не выбирают.
                if let (GestureState::Ended, GestureData::Tap { position, .. }) = (event.state, event.data)
                    && self.enabled
                {
                    self.select_neighbour_at(kit, position, event.time)?;
                }
            }
            ScreenGesture::Press(ButtonId::CalibrationReset(slot)) if slot == TONE_BASE_RESET_SLOT => {
                let data = kit.gesture_data_in_node(self.reset.node(), event.data);
                if self.reset.handle(event.state, data, event.time)? == crate::widget::button::ButtonEvent::Activated {
                    self.reset_if_differs(ctx, out);
                }
            }
            _ => return Ok(false),
        }
        Ok(true)
    }

    /// Тап в точке экрана `screen_point`: модель, чья подпись под пальцем и видна в окне переключателя, становится
    /// выбранной; выбранная и пустое место молчат. Действие уйдёт событием карусели (`advance`), как от свайпа.
    fn select_neighbour_at(
        &mut self,
        kit: &SceneKit,
        screen_point: Point,
        time: MonotonicNanos,
    ) -> Result<(), WidgetError> {
        // Вырожденное преобразование узла: точку перевести нельзя, выбирать нечего.
        let Some(point) = kit.point_in_node(self.carousel_node, screen_point) else {
            return Ok(());
        };
        let window = self.frames.carousel;
        let offset_points = point.x - window.center().x;
        // В узком портретном окне соседа не видно (торчит погасший край подписи): невидимое не выбирается.
        let is_visible = |index: usize| {
            self.carousel.placements(time).iter().any(|placement| {
                placement.index == index && placement.center_offset_points.abs() <= window.size.width / 2.0
            })
        };
        let tapped = self
            .carousel
            .item_index_at(offset_points, time)
            .filter(|index| *index != self.carousel.selected_index() && is_visible(*index));
        if let Some(index) = tapped {
            self.carousel
                .tapped_item(index, time)
                .map_err(element("карусель основы"))?;
        }
        Ok(())
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

    pub(super) fn advance(&mut self, out: &mut Outputs) {
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
    }

    pub(super) fn draw(
        &self,
        ctx: &Ctx<'_>,
        visuals: &VisualIndex,
        layers: &mut DrawLayers,
    ) -> Result<(), WidgetError> {
        let config = &ctx.config.calibration.widgets;
        let enabled_factor = if self.enabled { 1.0 } else { config.disabled_row_opacity };
        if let Some(placement) = visuals.placement(self.carousel_node)
            && let Some(paint) = NodePaint::new(ctx, placement, enabled_factor)?
        {
            // Соседние подписи не должны вылезать за переключатель на «↻» и чистую зону: гаснут у края его рамки и
            // режутся по ней.
            let labels = carousel_items_in_window(
                paint.at(0.0, 0.0),
                paint.opacity(),
                &self.carousel,
                ctx.now,
                &self.labels,
                ctx.px,
                &neighbour_label_colors(&ctx.tokens.colors),
                self.frames.carousel.size.width / 2.0,
            )?;
            if let Some(window) = placement.screen_rect_px() {
                layers.push_clipped(Layer::Content, window, labels);
            }
        }
        self.draw_reset(ctx, visuals, layers, enabled_factor)
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
}

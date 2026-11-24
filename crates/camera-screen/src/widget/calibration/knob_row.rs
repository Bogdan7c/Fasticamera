//! Строка ручки: УПЕРКЕЙС-заголовок слева, значение справа, «↻» и линейка с жёлтой отметкой (малая шкала в духе
//! карточки Exposure, решение 199). Одна реализация на глобальные ручки и на оси выбранной полосы.
//!
//! Значение ВСЕГДА берётся из `LookFacts` (экран своих значений не хранит): линейка отвечает только за физику,
//! подпись пишется из фактов, а не из линейки, поэтому значение между ступенями (0,15 при шаге 0,1) выводится
//! как есть. Эхо запрещено: действие рождает только палец (затвор пальца `ui-kit`); синхронизация с фактами
//! идёт программно и привязана к ИЗМЕНЕНИЮ фактов, а не к сравнению с линейкой каждый кадр.

use look_contract::{BandAxis, GlobalKnob, HueBandId, LookControl, LookFacts, LookKnob, WheelPart, WheelZone};
use std::num::NonZeroU32;
use ui_core::geometry::Rect;
use ui_core::gesture::{GestureEvent, PanSpec, RecognizerSpec};
use ui_core::id::NodeId;
use ui_draw::draw_list::DrawItem;
use ui_kit::pan::PanUpdate;
use ui_kit::ruler::{GatedEvent, RulerControl, RulerSpec};
use ui_kit::tokens::{ScaleDensity, TextRoleKind};
use ui_kit::value::{LinearLimits, QuantizedLinear, ReferenceValue};

use super::paint::NodePaint;
use super::reset_disc::draw_reset_disc;
use super::space::SectionSpace;
use crate::action::ScreenAction;
use crate::calibration::CalibrationWidgetsConfig;
use crate::widget::button::{ButtonEvent, PressButton};
use crate::widget::error::{WidgetError, element};
use crate::widget::intent::{ButtonId, ScreenGesture};
use crate::widget::kit::SceneKit;
use crate::widget::layers::{DrawLayers, Layer, VisualIndex};
use crate::widget::text_runs::TextRuns;
use crate::widget::{Ctx, Outputs};

/// Что регулирует строка.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RowTarget {
    /// Глобальная ручка вида.
    Knob(GlobalKnob),
    /// Ось ВЫБРАННОЙ полосы оттенка (какая полоса — решает выбор экрана).
    BandAxis(BandAxis),
    /// Кольцо яркости ВЫБРАННОГО колеса (какое колесо — решает выбор экрана; решение 205).
    WheelRing,
}

/// Номер строки кольца яркости колеса среди строк раздела: после восьми ручек, трёх осей полосы и основы тона
/// (`TONE_BASE_RESET_SLOT` = 11). Отдельное имя, а не очередное «магическое» число слота.
pub(super) const WHEEL_RING_SLOT: u8 = 12;

/// К чему привязана строка, если её цель зависит от выбора экрана. Привязка снимается на `sync`, а на `Began`
/// пальца фиксируется отдельно: смена выбора посреди жеста не перенаправляет значение (идиома `dragged_band`).
#[derive(Debug, Clone, PartialEq)]
enum RowBinding {
    /// Цель не зависит от выбора (глобальная ручка).
    Fixed,
    /// Ось выбранной полосы; `None` — полос нет.
    Band(Option<HueBandId>),
    /// Кольцо выбранного колеса.
    Wheel(WheelZone),
}

impl RowBinding {
    /// Что сейчас выбрано на экране для цели `target`.
    fn selected_for(target: RowTarget, ctx: &Ctx<'_>) -> Self {
        match target {
            RowTarget::Knob(_) => RowBinding::Fixed,
            RowTarget::BandAxis(_) => RowBinding::Band(ctx.state.calibration().selected_band().cloned()),
            RowTarget::WheelRing => RowBinding::Wheel(ctx.state.calibration().selected_wheel()),
        }
    }
}

impl RowTarget {
    /// Номер строки в жестах и кнопках: ручки 0..8, оси полосы 8..11 (11 — основа тона, ею владеет шапка),
    /// кольцо колеса — [`WHEEL_RING_SLOT`].
    pub(super) fn slot(self) -> u8 {
        match self {
            RowTarget::Knob(knob) => GlobalKnob::ALL
                .iter()
                .position(|known| *known == knob)
                .map_or(0, |i| i as u8),
            RowTarget::BandAxis(axis) => {
                GlobalKnob::ALL.len() as u8
                    + BandAxis::ALL
                        .iter()
                        .position(|known| *known == axis)
                        .map_or(0, |i| i as u8)
            }
            RowTarget::WheelRing => WHEEL_RING_SLOT,
        }
    }

    /// Заголовок строки заглавными.
    pub(super) fn title(self) -> &'static str {
        match self {
            RowTarget::Knob(GlobalKnob::Contrast) => "CONTRAST",
            RowTarget::Knob(GlobalKnob::Midtones) => "MIDTONES",
            RowTarget::Knob(GlobalKnob::Shadows) => "SHADOWS",
            RowTarget::Knob(GlobalKnob::Highlights) => "HIGHLIGHTS",
            RowTarget::Knob(GlobalKnob::Saturation) => "SATURATION",
            RowTarget::Knob(GlobalKnob::Vibrance) => "VIBRANCE",
            RowTarget::Knob(GlobalKnob::Warmth) => "WARMTH",
            RowTarget::Knob(GlobalKnob::Tint) => "TINT",
            RowTarget::BandAxis(BandAxis::Hue) => "HUE",
            RowTarget::BandAxis(BandAxis::Saturation) => "SAT",
            RowTarget::BandAxis(BandAxis::Luminance) => "LUM",
            RowTarget::WheelRing => "RING",
        }
    }

    /// Адрес органа управления для «↻» и сравнения с пресетом; `None` — у оси полосы нет выбранной полосы.
    fn control(self, binding: &RowBinding) -> Option<LookControl> {
        match (self, binding) {
            (RowTarget::Knob(knob), _) => Some(LookControl::Knob(knob)),
            (RowTarget::BandAxis(axis), RowBinding::Band(Some(band))) => Some(LookControl::Band {
                band: band.clone(),
                axis,
            }),
            (RowTarget::WheelRing, RowBinding::Wheel(zone)) => Some(LookControl::Wheel {
                zone: *zone,
                part: WheelPart::Ring,
            }),
            _ => None,
        }
    }

    /// Значение строки по фактам; нет фактов или привязки — нейтраль.
    fn value_in(self, facts: Option<&LookFacts>, binding: &RowBinding) -> LookKnob {
        let Some(facts) = facts else {
            return LookKnob::NEUTRAL;
        };
        match (self, binding) {
            (RowTarget::Knob(knob), _) => facts.current.settings.knob(knob),
            (RowTarget::BandAxis(axis), RowBinding::Band(Some(band))) => {
                facts.current.settings.bands.for_band(band).axis(axis)
            }
            (RowTarget::WheelRing, RowBinding::Wheel(zone)) => facts.current.settings.wheels.wheel(*zone).ring,
            _ => LookKnob::NEUTRAL,
        }
    }

    /// Умеет ли текущая основа эту ручку (ось полосы и кольцо колеса умеют все основы).
    fn supported_in(self, facts: &LookFacts) -> bool {
        match self {
            RowTarget::Knob(knob) => facts.is_knob_supported(knob),
            RowTarget::BandAxis(_) | RowTarget::WheelRing => true,
        }
    }
}

/// Подпись значения: `+0.30`, `−0.30` (минус U+2212), `0.00`. Два знака после запятой: шаг ручки 0,1, а
/// значение из пресета может лежать между ступенями — подпись показывает его как есть.
pub(super) fn format_knob_value(value: f32) -> String {
    debug_assert!(
        value.is_finite(),
        "значение ручки в подписи должно быть конечным: {value}"
    );
    let hundredths = (value * 100.0).round() as i32;
    let sign = match hundredths {
        0 => "",
        positive if positive > 0 => "+",
        _ => "\u{2212}",
    };
    format!("{sign}{}.{:02}", hundredths.abs() / 100, hundredths.abs() % 100)
}

/// Геометрия строки в системе корня раздела.
#[derive(Debug, Clone, Copy)]
pub(super) struct RowFrames {
    pub row: Rect,
    pub ruler: Rect,
    pub reset: Rect,
}

impl RowFrames {
    /// Рамки внутри строки `row`: сверху линия «заголовок · значение · ↻», под ней линейка.
    pub(super) fn of(row: Rect, config: &CalibrationWidgetsConfig) -> Self {
        let reset_side = config.reset_button_diameter_pt;
        let title_height = config.row_title_height_pt;
        Self {
            row,
            ruler: Rect::from_xywh(
                row.min_x(),
                row.min_y() + title_height,
                row.size.width,
                row.size.height - title_height,
            ),
            reset: Rect::from_xywh(
                row.max_x() - reset_side,
                row.min_y() + (title_height - reset_side) / 2.0,
                reset_side,
                reset_side,
            ),
        }
    }
}

/// Строка-линейка ручки.
pub(super) struct RulerRow {
    target: RowTarget,
    frames: RowFrames,
    node: NodeId,
    ruler_node: NodeId,
    reset: PressButton,
    ruler: RulerControl,
    /// Привязка строки к выбору экрана (полоса или колесо): по ней строка читает и пишет значения.
    bound: RowBinding,
    /// Привязка, которую палец взял на `Began`: действие уходит ей, даже если выбор сменился посреди жеста.
    dragged: Option<RowBinding>,
    /// Значение фактов, к которому линейка уже поехала (или которое она сама выдала пальцем).
    last_followed: Option<f32>,
    /// Доступна ли строка касаниям (факты есть, основа умеет ручку, полоса выбрана).
    enabled: bool,
    /// Сколько значений линейки не стали ручкой вида (NaN/вне диапазона): ошибка видна в диагностике.
    invalid_values: u64,
}

impl RulerRow {
    /// Строит узлы строки внутри `parent` (рамка `parent_rect`); `row_rect` — рамка строки в системе корня.
    pub(super) fn build(
        kit: &mut SceneKit,
        ctx: &Ctx<'_>,
        space: &SectionSpace,
        (parent, parent_rect): (NodeId, Rect),
        (target, row_rect): (RowTarget, Rect),
    ) -> Result<Self, WidgetError> {
        let config = &ctx.config.calibration.widgets;
        let frames = RowFrames::of(row_rect, config);
        let node = kit.add_node(parent, "calib_row", frames.row, parent_rect)?;
        let ruler_node = kit.add_node(node, "calib_ruler", frames.ruler, frames.row)?;
        kit.add_recognizer(
            ruler_node,
            RecognizerSpec::Pan(PanSpec {
                direction: space.pan_direction(super::space::EyeAxis::Horizontal),
                ..PanSpec::default()
            }),
            ScreenGesture::CalibrationRulerPan(target.slot()),
        )?;
        let reset_node = kit.add_node(node, "calib_reset", frames.reset, frames.row)?;
        let reset = PressButton::attach(
            kit,
            ButtonId::CalibrationReset(target.slot()),
            reset_node,
            frames.reset,
            ctx,
        )?;
        kit.ensure_min_touch_side(
            reset_node,
            frames.reset,
            ctx.geometry.config().card.button_touch_side_pt,
        )?;
        let facts = ctx.state.look_facts();
        let binding = RowBinding::selected_for(target, ctx);
        let initial = f64::from(target.value_in(facts, &binding).get());
        let ruler = RulerControl::new(
            Self::ruler_spec(config)?,
            initial,
            &ctx.environment(frames.ruler.size.width),
            ctx.now,
        )
        .map_err(element("линейка ручки"))?;
        Ok(Self {
            target,
            frames,
            node,
            ruler_node,
            reset,
            ruler,
            bound: binding,
            dragged: None,
            last_followed: None,
            enabled: false,
            invalid_values: 0,
        })
    }

    fn ruler_spec(config: &CalibrationWidgetsConfig) -> Result<RulerSpec, WidgetError> {
        let model = QuantizedLinear::new(
            config.knob_step,
            ReferenceValue::At(0.0),
            LinearLimits { min: -1.0, max: 1.0 },
        )
        .map_err(element("модель ручки"))?;
        let major_every_steps = NonZeroU32::new(config.knob_major_every_steps)
            .ok_or_else(|| WidgetError::Inconsistent("период крупной риски ручки равен нулю".to_owned()))?;
        Ok(RulerSpec {
            model,
            density: ScaleDensity::KnobRuler,
            major_every_steps,
        })
    }

    pub(super) fn slot(&self) -> u8 {
        self.target.slot()
    }

    pub(super) fn frame(&self) -> Rect {
        self.frames.row
    }

    pub(super) fn invalid_values(&self) -> u64 {
        self.invalid_values
    }

    pub(super) fn suppressed_echo_events(&self) -> u64 {
        self.ruler.suppressed_echo_events()
    }

    /// Положение линейки в шагах (проба для тестов: между ступенями не округляется).
    #[cfg(test)]
    pub(super) fn position_steps(&self, now: ui_core::time::MonotonicNanos) -> f64 {
        self.ruler.position_steps(now)
    }

    /// Жесты строки: пан линейки и «↻». `false` — событие чужое.
    pub(super) fn handle_gesture(
        &mut self,
        kit: &SceneKit,
        space: &SectionSpace,
        ctx: &Ctx<'_>,
        event: &GestureEvent<ScreenGesture>,
        out: &mut Outputs,
    ) -> Result<bool, WidgetError> {
        if event.intent == ScreenGesture::CalibrationRulerPan(self.slot()) {
            self.handle_pan(space, ctx, event)?;
            return Ok(true);
        }
        if event.intent == ScreenGesture::Press(ButtonId::CalibrationReset(self.slot())) {
            let data = kit.gesture_data_in_node(self.reset.node(), event.data);
            if self.reset.handle(event.state, data, event.time)? == ButtonEvent::Activated {
                self.reset_if_differs(ctx, out);
            }
            return Ok(true);
        }
        Ok(false)
    }

    fn handle_pan(
        &mut self,
        space: &SectionSpace,
        ctx: &Ctx<'_>,
        event: &GestureEvent<ScreenGesture>,
    ) -> Result<(), WidgetError> {
        let Some(update) = PanUpdate::from_gesture(event.state, event.data).map_err(element("пан ручки"))?
        else {
            return Ok(());
        };
        if !self.enabled && !self.ruler.is_finger_driven() {
            // Строка выключена (основа не умеет ручку, полосы нет): палец не берёт линейку.
            return Ok(());
        }
        if matches!(update, PanUpdate::Began { .. }) {
            self.dragged = Some(RowBinding::selected_for(self.target, ctx));
        }
        self.ruler
            .apply_pan(event.time, space.eye_pan(update))
            .map_err(element("линейка ручки"))
    }

    /// «↻»: сбросить к значению пресета, только если оно отличается (тусклая кнопка ничего не делает).
    fn reset_if_differs(&self, ctx: &Ctx<'_>, out: &mut Outputs) {
        if !self.enabled {
            return;
        }
        let Some(facts) = ctx.state.look_facts() else {
            return;
        };
        let Some(control) = self.target.control(&self.bound) else {
            return;
        };
        if facts.control_differs(&control) {
            out.actions.push(ScreenAction::ResetControlTapped(control));
        }
    }

    /// Выравнивает строку по состоянию: доступность, привязку к полосе и положение линейки (без эха).
    pub(super) fn sync(&mut self, kit: &mut SceneKit, ctx: &Ctx<'_>) -> Result<(), WidgetError> {
        let facts = ctx.state.look_facts();
        let selected = RowBinding::selected_for(self.target, ctx);
        if self.bound != selected {
            // Выбрана другая полоса или другое колесо: строка теперь показывает его значения. Забываем, к чему ехали.
            self.bound = selected;
            self.last_followed = None;
        }
        let value = self.target.value_in(facts, &self.bound);
        let has_target = self.bound != RowBinding::Band(None);
        self.enabled = facts.is_some_and(|facts| self.target.supported_in(facts) && has_target);
        kit.set_interactive(self.node, self.enabled)?;
        if self.last_followed != Some(value.get()) && !self.ruler.is_finger_driven() {
            self.last_followed = Some(value.get());
            self.ruler
                .follow_exact_value(
                    f64::from(value.get()),
                    ctx.config.calibration.widgets.follow_tolerance_steps,
                    ctx.now,
                )
                .map_err(element("линейка ручки"))?;
        }
        Ok(())
    }

    /// Кадр линейки: значения пальца → `KnobDragged` / `BandKnobDragged`, хаптика.
    pub(super) fn advance(&mut self, ctx: &Ctx<'_>, out: &mut Outputs) {
        for event in self.ruler.advance(ctx.now) {
            match event {
                GatedEvent::FingerValue(value) => self.emit_dragged(value, out),
                GatedEvent::Haptic(pattern) => out.haptics.push(pattern),
            }
        }
    }

    fn emit_dragged(&mut self, value: f64, out: &mut Outputs) {
        let Ok(knob) = LookKnob::new(value as f32) else {
            self.invalid_values += 1;
            return;
        };
        // Значение уже наше: следующий такой же факт ехать не заставит.
        self.last_followed = Some(knob.get());
        match self.target {
            RowTarget::Knob(which) => out.actions.push(ScreenAction::KnobDragged {
                knob: which,
                value: knob,
            }),
            RowTarget::BandAxis(axis) => {
                let Some(RowBinding::Band(Some(band))) = self.dragged.clone().or_else(|| Some(self.bound.clone()))
                else {
                    self.invalid_values += 1;
                    return;
                };
                out.actions.push(ScreenAction::BandKnobDragged {
                    band,
                    axis,
                    value: knob,
                });
            }
            RowTarget::WheelRing => {
                let Some(RowBinding::Wheel(zone)) = self.dragged.clone().or_else(|| Some(self.bound.clone())) else {
                    self.invalid_values += 1;
                    return;
                };
                out.actions.push(ScreenAction::WheelRingDragged { zone, ring: knob });
            }
        }
    }

    /// Рисует строку целиком; что не помещается в окно списка, режет окно (`KnobsColumn::draw`).
    pub(super) fn draw(
        &self,
        ctx: &Ctx<'_>,
        visuals: &VisualIndex,
        layers: &mut DrawLayers,
        texts: &mut TextRuns,
    ) -> Result<(), WidgetError> {
        let config = &ctx.config.calibration.widgets;
        let disabled_factor = if self.enabled { 1.0 } else { config.disabled_row_opacity };
        let Some(row_placement) = visuals.placement(self.node) else {
            return Ok(());
        };
        let Some(paint) = NodePaint::new(ctx, row_placement, disabled_factor)? else {
            return Ok(());
        };
        let colors = &ctx.tokens.colors;
        let facts = ctx.state.look_facts();
        let value = self.target.value_in(facts, &self.bound);
        let reset_side = config.reset_button_diameter_pt;
        let half_width = self.frames.row.size.width / 2.0;
        let title_y = config.row_title_height_pt / 2.0 - self.frames.row.size.height / 2.0;
        let title = texts.run(ctx, self.target.title(), TextRoleKind::CardTitle)?;
        layers.push(
            Layer::Content,
            paint.text_left(title, -half_width, title_y, colors.panel_label),
        );
        let value_run = texts.run(ctx, &format_knob_value(value.get()), TextRoleKind::ScaleValue)?;
        let value_right = half_width - reset_side - config.value_gap_pt;
        layers.push(
            Layer::Content,
            paint.text_right(value_run, value_right, title_y, colors.text_primary),
        );
        self.draw_reset(ctx, &paint, layers, value_is_edited(self, ctx));
        self.draw_ruler(ctx, visuals, layers, disabled_factor)
    }

    /// Диск «↻»: яркий, если значение отличается от пресета, иначе приглушён (`widgets.card.reset_dim_opacity`).
    fn draw_reset(&self, ctx: &Ctx<'_>, paint: &NodePaint, layers: &mut DrawLayers, differs: bool) {
        let center = (
            self.frames.reset.center().x - self.frames.row.center().x,
            self.frames.reset.center().y - self.frames.row.center().y,
        );
        draw_reset_disc(ctx, paint, layers, center, differs);
    }

    fn draw_ruler(
        &self,
        ctx: &Ctx<'_>,
        visuals: &VisualIndex,
        layers: &mut DrawLayers,
        extra_opacity: f32,
    ) -> Result<(), WidgetError> {
        let Some(placement) = visuals.placement(self.ruler_node) else {
            return Ok(());
        };
        let Some(paint) = NodePaint::new(ctx, placement, extra_opacity)? else {
            return Ok(());
        };
        let items: Vec<DrawItem> = self.ruler.items(
            &paint.placement(),
            ctx.now,
            ctx.px,
            &ctx.tokens.ruler,
            &ctx.tokens.colors,
        )?;
        layers.extend(Layer::Content, items);
        Ok(())
    }
}

/// Отличается ли значение строки от пресета (по фактам и привязанной полосе).
fn value_is_edited(row: &RulerRow, ctx: &Ctx<'_>) -> bool {
    let (Some(facts), Some(control)) = (ctx.state.look_facts(), row.target.control(&row.bound)) else {
        return false;
    };
    facts.control_differs(&control)
}

#[cfg(test)]
mod tests {
    use super::format_knob_value;

    #[test]
    fn knob_value_text_has_sign_two_decimals_and_a_true_minus() {
        assert_eq!(format_knob_value(0.3), "+0.30");
        assert_eq!(format_knob_value(-0.3), "\u{2212}0.30");
        assert_eq!(format_knob_value(0.0), "0.00");
        assert_eq!(format_knob_value(-0.0), "0.00");
        assert_eq!(format_knob_value(0.15), "+0.15");
        assert_eq!(format_knob_value(1.0), "+1.00");
        assert_eq!(
            format_knob_value(-0.004),
            "0.00",
            "значение, округлённое до нуля, без знака"
        );
    }
}

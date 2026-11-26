//! Строка ручки: УПЕРКЕЙС-заголовок слева, значение справа, «↻» и линейка с жёлтой отметкой (малая шкала в духе
//! карточки Exposure, решение 199). Одна реализация на глобальные ручки и на оси выбранной полосы.
//!
//! Значение ВСЕГДА берётся из `LookFacts` (экран своих значений не хранит): линейка отвечает только за физику,
//! подпись пишется из фактов, а не из линейки, поэтому значение между ступенями (0,15 при шаге 0,1) выводится
//! как есть. Эхо запрещено: действие рождает только палец (затвор пальца `ui-kit`); синхронизация с фактами
//! идёт программно и привязана к ИЗМЕНЕНИЮ фактов, а не к сравнению с линейкой каждый кадр.

use look_contract::{
    BandAxis, GlobalKnob, HueBandId, LookControl, LookFacts, LookKnob, LookSettings, WheelPart, WheelZone,
};
use std::num::NonZeroU32;
use ui_core::geometry::Rect;
use ui_core::gesture::{GestureEvent, PanSpec, RecognizerSpec};
use ui_core::id::NodeId;
use ui_draw::draw_list::DrawItem;
use ui_kit::drive::DragOrigin;
use ui_kit::pan::PanUpdate;
use ui_kit::ruler::{GatedEvent, HapticPacing, RulerControl, RulerFeel, RulerSpec};
use ui_kit::scale::ZeroMagnet;
use ui_kit::tokens::TextRoleKind;
use ui_kit::value::{LinearLimits, QuantizedLinear, ReferenceValue, TickPitch};
use ui_kit::view::{RulerMarkerStyle, RulerStopTickStyle};

use super::knob_scale::KnobScale;
use super::knob_stops::RowStops;

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
        facts.map_or(LookKnob::NEUTRAL, |facts| {
            self.value_in_settings(&facts.current.settings, binding)
        })
    }

    /// Значение строки в рецепте АКТИВНОГО ПРЕСЕТА (без правок); нет фактов или привязки — нейтраль.
    fn preset_value_in(self, facts: Option<&LookFacts>, binding: &RowBinding) -> LookKnob {
        facts.map_or(LookKnob::NEUTRAL, |facts| {
            self.value_in_settings(&facts.preset.settings, binding)
        })
    }

    /// Значение строки в данных настройках вида (текущих или пресета); нет привязки — нейтраль.
    fn value_in_settings(self, settings: &LookSettings, binding: &RowBinding) -> LookKnob {
        match (self, binding) {
            (RowTarget::Knob(knob), _) => settings.knob(knob),
            (RowTarget::BandAxis(axis), RowBinding::Band(Some(band))) => settings.bands.for_band(band).axis(axis),
            (RowTarget::WheelRing, RowBinding::Wheel(zone)) => settings.wheels.wheel(*zone).ring,
            _ => LookKnob::NEUTRAL,
        }
    }
}

/// Геометрия строки в системе корня раздела.
#[derive(Debug, Clone, Copy)]
pub(super) struct RowFrames {
    pub row: Rect,
    /// Рамка линейки (по ней она рисуется).
    pub ruler: Rect,
    /// Где линейка ловит палец: рамка без нижней полосы, которую забирает «↻» строки ниже (решение 279).
    pub ruler_touch: Rect,
    /// Рамка кнопки «↻» (по ней она рисуется).
    pub reset: Rect,
    /// Где «↻» ловит палец: ширина цели касания у правого края строки, высота — полоса заголовка и полоса над
    /// строкой. Внутри строки по ширине, чтобы окно списка её не резало; со своей линейкой общих точек нет.
    pub reset_touch: Rect,
}

impl RowFrames {
    /// Рамки внутри строки `row`: сверху линия «заголовок · значение · ↻», под ней линейка. `touch_side_pt` —
    /// сторона цели касания маленькой кнопки (HIG 44 pt).
    pub(super) fn of(row: Rect, config: &CalibrationWidgetsConfig, touch_side_pt: f64) -> Self {
        let reset_side = config.reset_button_diameter_pt;
        let title_height = config.row_title_height_pt;
        let reach_above = config.row_reset_reach_above_pt;
        let ruler = Rect::from_xywh(
            row.min_x(),
            row.min_y() + title_height,
            row.size.width,
            row.size.height - title_height,
        );
        let reset_touch_width = touch_side_pt.min(row.size.width);
        Self {
            row,
            ruler,
            ruler_touch: Rect::from_xywh(
                ruler.min_x(),
                ruler.min_y(),
                ruler.size.width,
                ruler.size.height - reach_above,
            ),
            reset: Rect::from_xywh(
                row.max_x() - reset_side,
                row.min_y() + (title_height - reset_side) / 2.0,
                reset_side,
                reset_side,
            ),
            reset_touch: Rect::from_xywh(
                row.max_x() - reset_touch_width,
                row.min_y() - reach_above,
                reset_touch_width,
                title_height + reach_above,
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
    /// Шкала чисел ручки (−N…+N): как число на линейке связано со значением и подписью.
    scale: KnobScale,
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
    /// Сколько раз пришло продолжение пана без начала (страховка: на нормальном пути 0, см. `handle_pan`).
    orphan_pan_continuations: u64,
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
        let frames = RowFrames::of(row_rect, config, ctx.geometry.config().card.button_touch_side_pt);
        let node = kit.add_node(parent, "calib_row", frames.row, parent_rect)?;
        let ruler_node = kit.add_node(node, "calib_ruler", frames.ruler, frames.row)?;
        kit.set_touch_area(ruler_node, frames.ruler, frames.ruler_touch)?;
        kit.add_recognizer(
            ruler_node,
            RecognizerSpec::Pan(PanSpec {
                direction: space.pan_direction(super::space::EyeAxis::Horizontal),
                // Меньший порог распознавания, чем у остальных панов: путь пальца до него не теряется (линейка
                // считает смещение от точки касания, см. `ruler_spec`), а вертикальная прокрутка списка по-прежнему
                // уходит списку: ось решается в момент превышения порога.
                hysteresis_pt: config.knob_pan_hysteresis_pt,
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
        kit.set_touch_area(reset_node, frames.reset, frames.reset_touch)?;
        let facts = ctx.state.look_facts();
        let binding = RowBinding::selected_for(target, ctx);
        let initial = f64::from(target.value_in(facts, &binding).get());
        let scale = KnobScale::from_config(config)?;
        let ruler = RulerControl::new(
            Self::ruler_spec(config, scale)?,
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
            scale,
            bound: binding,
            dragged: None,
            last_followed: None,
            enabled: false,
            invalid_values: 0,
            orphan_pan_continuations: 0,
        })
    }

    fn ruler_spec(config: &CalibrationWidgetsConfig, scale: KnobScale) -> Result<RulerSpec, WidgetError> {
        let tick_every_steps = NonZeroU32::new(config.knob_tick_every_steps)
            .ok_or_else(|| WidgetError::Inconsistent("период риски ручки равен нулю".to_owned()))?;
        // Шаг значения мелкий (1 из N), риска и щелчок — только через `knob_tick_every_steps` шагов и на нуле.
        let model = QuantizedLinear::new(
            scale.step(),
            ReferenceValue::At(0.0),
            LinearLimits { min: -1.0, max: 1.0 },
        )
        .map_err(element("модель ручки"))?
        .with_tick_pitch(TickPitch::EveryNSteps(tick_every_steps));
        let major_every_steps = NonZeroU32::new(config.knob_major_every_steps)
            .ok_or_else(|| WidgetError::Inconsistent("период крупной риски ручки равен нулю".to_owned()))?;
        Ok(RulerSpec {
            model,
            density: scale.density(),
            major_every_steps,
            feel: RulerFeel {
                // Значение догоняет палец с точки касания: порог распознавания пана не «съедает» ход.
                drag_origin: DragOrigin::FromTouchDown,
                // Мелкий шаг: быстрый взмах не должен сыпать щелчки каждый кадр.
                haptic_pacing: HapticPacing::Throttled,
                // Ноль на мгновение держится под пальцем (решение 284); 0 в конфиге — без магнита.
                zero_magnet: ZeroMagnet::hold_within(config.knob_zero_magnet_pt)
                    .map_err(element("магнит нуля ручки"))?,
            },
        })
    }

    /// Узел строки: корень её поддерева (заголовок, значение, «↻», линейка).
    pub(super) fn node(&self) -> NodeId {
        self.node
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

    /// Продолжения пана без начала, пришедшие в эту строку (диагностика, на нормальном пути 0).
    pub(super) fn orphan_pan_continuations(&self) -> u64 {
        self.orphan_pan_continuations
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
        if !matches!(update, PanUpdate::Began { .. }) && !self.ruler.is_finger_driven() {
            // Страховка: продолжение жеста, начало которого эта строка не видела. Пересборка раздела посреди
            // протяжки сюда больше не ведёт (`Stage` отдаёт `Cancelled` уходящего раздела ему самому), поэтому
            // событие считается — ненулевой счётчик значит новую дыру в маршрутизации. Выключенная строка событий
            // не ждёт, но и не жалуется (палец её не берёт).
            if self.enabled {
                self.orphan_pan_continuations += 1;
            }
            return Ok(());
        }
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
        self.enabled = facts.is_some() && has_target;
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
        let Ok(knob) = self.scale.knob_at(value) else {
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
        let value_run = texts.run(ctx, &self.scale.label(value), TextRoleKind::ScaleValue)?;
        let value_right = half_width - reset_side - config.value_gap_pt;
        // За упором картинка не меняется: число тусклое (решения 288, 293).
        let value_color = if RowStops::of(self.target, ctx).holds_beyond(value) {
            colors.text_primary.with_alpha(config.knob_stops.beyond_value_alpha)
        } else {
            colors.text_primary
        };
        layers.push(
            Layer::Content,
            paint.text_right(value_run, value_right, title_y, value_color),
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
        // Упоры теплоты и оттенка: риски за упором тусклее, на самом упоре своя риска. У остальных строк упоров нет,
        // список промежутков пуст, и линейка рисуется как раньше.
        let stops = RowStops::of(self.target, ctx);
        let stops_look = &ctx.config.calibration.widgets.knob_stops;
        let items: Vec<DrawItem> = self.ruler.items_with_dimmed_ranges(
            &paint.placement(),
            ctx.now,
            ctx.px,
            (&ctx.tokens.ruler, &ctx.tokens.colors),
            &stops.dimmed_ranges(stops_look),
        )?;
        layers.extend(Layer::Content, items);
        let stop_tick_style = RulerStopTickStyle {
            thickness_pt: stops_look.tick_thickness_pt,
            length_pt: stops_look.tick_length_pt,
            color: ctx.tokens.colors.text_primary.with_alpha(stops_look.tick_alpha),
        };
        for value in stops.tick_values() {
            if let Some(tick) = self.ruler.stop_tick_item(
                &paint.placement(),
                ctx.now,
                ctx.px,
                &ctx.tokens.ruler,
                (value, stop_tick_style),
            )? {
                layers.push(Layer::Content, tick);
            }
        }
        if let Some(marker) = self.preset_marker(ctx, &paint)? {
            layers.push(Layer::Content, marker);
        }
        Ok(())
    }

    /// Метка значения пресета на линейке: точка под рисками, пока текущее значение отличается от пресета.
    /// Метка — только рисунок: ни действий, ни магнита (значение к ней не липнет, ноль остаётся опорой).
    fn preset_marker(&self, ctx: &Ctx<'_>, paint: &NodePaint) -> Result<Option<DrawItem>, WidgetError> {
        let Some(control) = self.target.control(&self.bound) else {
            return Ok(None);
        };
        let facts = ctx.state.look_facts();
        if !facts.is_some_and(|facts| facts.control_differs(&control)) {
            return Ok(None);
        }
        let config = &ctx.config.calibration.widgets;
        let preset_value = self.target.preset_value_in(facts, &self.bound);
        let style = RulerMarkerStyle {
            diameter_pt: config.preset_marker_diameter_pt,
            gap_pt: config.preset_marker_gap_pt,
            color: ctx.tokens.colors.text_primary.with_alpha(config.preset_marker_alpha),
        };
        let item = self.ruler.marker_item(
            &paint.placement(),
            ctx.now,
            ctx.px,
            &ctx.tokens.ruler,
            (f64::from(preset_value.get()), style),
        )?;
        Ok(item)
    }
}

/// Отличается ли значение строки от пресета (по фактам и привязанной полосе).
fn value_is_edited(row: &RulerRow, ctx: &Ctx<'_>) -> bool {
    let (Some(facts), Some(control)) = (ctx.state.look_facts(), row.target.control(&row.bound)) else {
        return false;
    };
    facts.control_differs(&control)
}

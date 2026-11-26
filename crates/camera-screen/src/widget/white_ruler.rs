//! Шкалы карточки белого: Кельвины (по майредам) и оттенок, по одной на ось (WB28, WB34).
//!
//! # Владение
//! Значение белого держит движок. Шкала — только физика и вид: она встаёт на значение из фактов
//! ([`super::white_follow`]) и отдаёт значения пальца как намерения (`WhiteTemperatureDragged` /
//! `WhiteTintDragged`). Обе шкалы следуют за фактами ВСЕГДА, даже закрытая карточка и невыбранная ось: при открытии
//! или смене оси шкала уже стоит там, где белое, и показ намерений не порождает.
//!
//! # Оси и знак
//! * Кельвины — `MiredKelvin`: шкала равномерна по майредам, больше Кельвинов — вправо. Значение шкалы — Кельвины.
//! * Оттенок — `ContinuousLinear` в единицах показа (Duv × `tint_units_per_duv`); плюс — вправо и к пурпурному,
//!   как у ручки вида TINT. Палец влево — значение растёт, как у линейки Exposure (`GrowthDirection::FingerLeft`).
//!   Значение пальца → Duv = единицы / `tint_units_per_duv`, без инверсии знака.
//! * Движется только ось, которую ведёт палец: вторую ось намерение не несёт (точная вторая ось — у движка).
//!
//! Шкала непрерывная: значение от пальца не округляется, после отпускания она не липнет к рискам. Пределы
//! берутся из `facts.white.range` и меняются на лету; невозможные пределы или значения не роняют кадр, а
//! считаются ([`WhiteRuler::invalid_values`]).

use std::num::NonZeroU32;

use camera_contract::control::{ColorTemperature, Tint};
use ui_core::geometry::Rect;
use ui_core::gesture::{GestureEvent, PanSpec, RecognizerSpec};
use ui_core::id::NodeId;
use ui_draw::scene_bridge::NodePlacementPx;
use ui_draw::transform::Affine2;
use ui_kit::pan::PanUpdate;
use ui_kit::ruler::{GatedEvent, RulerControl, RulerFeel, RulerSpec};
use ui_kit::tokens::ScaleDensity;
use ui_kit::value::{ContinuousLinear, ExactPosition, LinearLimits, MiredKelvin, RulerAxis, ValueModel};

use super::error::{WidgetError, element};
use super::intent::ScreenGesture;
use super::kit::SceneKit;
use super::layers::{DrawLayers, Layer, VisualIndex};
use super::white_card_site::WhiteCardSite;
use super::white_follow::{AxisFollower, EchoRules, FollowVerdict};
use super::{Ctx, Outputs};
use crate::action::ScreenAction;
use crate::white::WhiteAxis;
use crate::white_facts::WhiteRange;

/// Одна шкала с её слежением за фактами.
struct AxisRuler<M: RulerAxis + ExactPosition + ValueModel<Value = f64, Limits = LinearLimits>> {
    control: RulerControl<M>,
    follower: AxisFollower,
}

impl<M> AxisRuler<M>
where
    M: RulerAxis + ExactPosition + ValueModel<Value = f64, Limits = LinearLimits>,
{
    fn new(
        spec: RulerSpec<M>,
        initial_value: f64,
        width_pt: f64,
        ctx: &Ctx<'_>,
        what: &'static str,
    ) -> Result<Self, WidgetError> {
        Ok(Self {
            control: RulerControl::new(spec, initial_value, &ctx.environment(width_pt), ctx.now)
                .map_err(element(what))?,
            follower: AxisFollower::default(),
        })
    }

    /// Пределы и факт: шкала встаёт на изменившийся факт точно, без намерений. Возвращает, сколько раз пришлось
    /// отвергнуть невозможное (пределы/значение).
    fn follow_fact(&mut self, fact: f64, limits: LinearLimits, rules: &EchoRules, ctx: &Ctx<'_>) -> u64 {
        let mut rejected = 0;
        if self.control.retarget_limits(limits, ctx.now).is_err() {
            rejected += 1;
        }
        let verdict = self
            .follower
            .on_fact(fact, self.control.is_finger_driven(), ctx.now, rules);
        let verdict = match verdict {
            FollowVerdict::Stay => self
                .follower
                .reconcile(fact, self.control.is_finger_driven(), ctx.now, rules)
                .map_or(FollowVerdict::Stay, FollowVerdict::Follow),
            follow @ FollowVerdict::Follow(_) => follow,
        };
        if let FollowVerdict::Follow(value) = verdict
            && self
                .control
                .show_exact_value(value, ctx.config.widgets.white_card.follow_tolerance_units, ctx.now)
                .is_err()
        {
            rejected += 1;
        }
        rejected
    }

    /// Кадр шкалы: значения пальца возвращаются (и запоминаются как «отправленные»), хаптика уходит в `out`.
    /// `intents_allowed` — карточка белого открыта: только тогда значения уходят намерениями. Иначе (карточку
    /// закрыли или сменили посреди ведения) значения гасятся и «отправленными» не помнятся.
    fn advance(&mut self, rules: &EchoRules, ctx: &Ctx<'_>, out: &mut Outputs, intents_allowed: bool) -> Vec<f64> {
        let mut finger_values = Vec::new();
        for event in self.control.advance(ctx.now) {
            if !intents_allowed {
                continue;
            }
            match event {
                GatedEvent::FingerValue(value) => {
                    self.follower.note_sent(value, ctx.now, rules);
                    finger_values.push(value);
                }
                GatedEvent::Haptic(pattern) => out.haptics.push(pattern),
            }
        }
        finger_values
    }
}

/// Обе шкалы карточки белого в одной зоне пана.
pub(crate) struct WhiteRuler {
    /// Узел зоны пана; его размещение отдаёт виду центр и протяжённость.
    node: NodeId,
    temperature: AxisRuler<MiredKelvin>,
    tint: AxisRuler<ContinuousLinear>,
    /// Сколько значений не стали белым (не число, вне возможного) или пределов, которые шкала не приняла.
    invalid_values: u64,
    /// Идёт ли пан зоны: после снятия ведения (карточку закрыли) хвост жеста шкале не передаётся.
    pan_active: bool,
    /// Дом шкалы: от него зависят номер пана, оси смещений и ответ «карточка открыта» (PHOTO или раздел).
    site: WhiteCardSite,
}

impl WhiteRuler {
    /// Зона пана — ребёнок узла белой карточки; пока карточка не открыта, зона не ловит касаний.
    pub(crate) fn build(
        kit: &mut SceneKit,
        ctx: &Ctx<'_>,
        (card_node, card_rect): (NodeId, Rect),
        site: &WhiteCardSite,
    ) -> Result<Self, WidgetError> {
        let rect = site.geometry().white_ruler();
        let node = kit.add_node(card_node, "white_ruler", rect, card_rect)?;
        let (direction, pan_gesture) = site.ruler_pan();
        kit.add_recognizer(
            node,
            RecognizerSpec::Pan(PanSpec {
                direction,
                ..PanSpec::default()
            }),
            pan_gesture,
        )?;
        kit.set_interactive(node, false)?;
        let facts = ctx.state.camera_facts().white;
        let white = &ctx.config.white;
        let temperature_spec = RulerSpec {
            model: MiredKelvin::new(white.mired_step, kelvin_limits(&facts.range))
                .map_err(element("модель Кельвинов"))?,
            density: ScaleDensity::MiredRuler,
            major_every_steps: period(white.mired_major_every, "mired_major_every")?,
            feel: RulerFeel::default(),
        };
        let tint_spec = RulerSpec {
            model: ContinuousLinear::new(
                white.tint_step_units,
                tint_limits(&facts.range, white.tint_units_per_duv),
            )
            .map_err(element("модель оттенка"))?,
            density: ScaleDensity::TintRuler,
            major_every_steps: period(white.tint_major_every, "tint_major_every")?,
            feel: RulerFeel::default(),
        };
        Ok(Self {
            node,
            temperature: AxisRuler::new(
                temperature_spec,
                facts.held.setting.temperature.kelvin(),
                rect.size.width,
                ctx,
                "шкала Кельвинов",
            )?,
            tint: AxisRuler::new(
                tint_spec,
                facts.held.setting.tint.duv() * white.tint_units_per_duv,
                rect.size.width,
                ctx,
                "шкала оттенка",
            )?,
            invalid_values: 0,
            pan_active: false,
            site: site.clone(),
        })
    }

    /// Ловит ли зона касания (открыта именно карточка белого).
    pub(crate) fn set_active(&mut self, kit: &mut SceneKit, active: bool) -> Result<(), WidgetError> {
        kit.set_interactive(self.node, active)
    }

    /// Пан зоны ведёт шкалу выбранной оси. `false` — событие чужое.
    pub(crate) fn handle_gesture(
        &mut self,
        event: &GestureEvent<ScreenGesture>,
        selected: WhiteAxis,
    ) -> Result<bool, WidgetError> {
        if event.intent != self.site.ruler_pan().1 {
            return Ok(false);
        }
        let Some(update) = PanUpdate::from_gesture(event.state, event.data).map_err(element("пан белого"))?
        else {
            return Ok(true);
        };
        // Шкала думает вдоль себя: в разделе пан поворачивается из осей поверхности в глазные.
        let update = self.site.scale_pan(update);
        match update {
            PanUpdate::Began { .. } => self.pan_active = true,
            PanUpdate::Changed { .. } | PanUpdate::Ended { .. } if !self.pan_active => return Ok(true),
            PanUpdate::Ended { .. } | PanUpdate::Cancelled => {}
            PanUpdate::Changed { .. } => {}
        }
        if matches!(update, PanUpdate::Ended { .. } | PanUpdate::Cancelled) {
            if !self.pan_active {
                return Ok(true);
            }
            self.pan_active = false;
        }
        // Палец, уже ведущий одну шкалу, не перескакивает на другую при смене выбранной оси посреди жеста.
        let axis = if self.temperature.control.is_finger_driven() {
            WhiteAxis::Temperature
        } else if self.tint.control.is_finger_driven() {
            WhiteAxis::Tint
        } else {
            selected
        };
        match axis {
            WhiteAxis::Temperature => self.temperature.control.apply_pan(event.time, update),
            WhiteAxis::Tint => self.tint.control.apply_pan(event.time, update),
        }
        .map_err(element("шкала белого"))?;
        Ok(true)
    }

    /// Выравнивает обе шкалы по фактам: пределы на лету, точное значение при изменении факта.
    pub(crate) fn sync(&mut self, ctx: &Ctx<'_>) {
        if !self.site.is_open(ctx.state) {
            // Карточку закрыли или сменили посреди ведения: ведение снимается, намерений после этого нет.
            self.cancel_drag(ctx);
        }
        let facts = ctx.state.camera_facts().white;
        let white = &ctx.config.white;
        let rules = EchoRules::from_layout(&ctx.config.widgets.white_card);
        self.invalid_values += self.temperature.follow_fact(
            facts.held.setting.temperature.kelvin(),
            kelvin_limits(&facts.range),
            &rules,
            ctx,
        );
        self.invalid_values += self.tint.follow_fact(
            facts.held.setting.tint.duv() * white.tint_units_per_duv,
            tint_limits(&facts.range, white.tint_units_per_duv),
            &rules,
            ctx,
        );
    }

    /// Снять ведение шкалы пальцем (жест отменён без действия).
    fn cancel_drag(&mut self, ctx: &Ctx<'_>) {
        self.pan_active = false;
        for (driven, control) in [
            (
                self.temperature.control.is_finger_driven(),
                &mut self.temperature.control as &mut dyn CancelPan,
            ),
            (
                self.tint.control.is_finger_driven(),
                &mut self.tint.control as &mut dyn CancelPan,
            ),
        ] {
            if driven && control.cancel(ctx.now).is_err() {
                self.invalid_values += 1;
            }
        }
    }

    /// Кадр шкал: значения пальца → намерения экрана, тики → хаптика.
    pub(crate) fn advance(&mut self, ctx: &Ctx<'_>, out: &mut Outputs) {
        let rules = EchoRules::from_layout(&ctx.config.widgets.white_card);
        let allowed = self.site.is_open(ctx.state);
        for kelvin in self.temperature.advance(&rules, ctx, out, allowed) {
            match ColorTemperature::from_kelvin(kelvin) {
                Ok(temperature) => out.actions.push(ScreenAction::WhiteTemperatureDragged(temperature)),
                Err(_rejected) => self.invalid_values += 1,
            }
        }
        let units_per_duv = ctx.config.white.tint_units_per_duv;
        for units in self.tint.advance(&rules, ctx, out, allowed) {
            match Tint::from_duv(units / units_per_duv) {
                Ok(tint) => out.actions.push(ScreenAction::WhiteTintDragged(tint)),
                Err(_rejected) => self.invalid_values += 1,
            }
        }
    }

    /// Риски и отметка выбранной оси (общий вид `RulerControl::items`). `content_alpha` и `slide_px` — прозрачность
    /// и сдвиг содержимого карточки.
    pub(crate) fn draw(
        &self,
        ctx: &Ctx<'_>,
        visuals: &VisualIndex,
        layers: &mut DrawLayers,
        (axis, content_alpha, slide_px): (WhiteAxis, f32, f32),
    ) -> Result<(), WidgetError> {
        let Some(node_placement) = visuals.placement(self.node) else {
            return Ok(());
        };
        let placement = NodePlacementPx {
            transform: Affine2::translate(0.0, slide_px).then(node_placement.transform),
            opacity: (node_placement.opacity * content_alpha).clamp(0.0, 1.0),
            ..node_placement
        };
        let (px, ruler_tokens, colors) = (ctx.px, &ctx.tokens.ruler, &ctx.tokens.colors);
        let items = match axis {
            WhiteAxis::Temperature => self
                .temperature
                .control
                .items(&placement, ctx.now, px, ruler_tokens, colors)?,
            WhiteAxis::Tint => self.tint.control.items(&placement, ctx.now, px, ruler_tokens, colors)?,
        };
        layers.extend(Layer::Content, items);
        Ok(())
    }

    /// Сколько значений или пределов шкала отвергла как невозможные.
    pub(crate) fn invalid_values(&self) -> u64 {
        self.invalid_values
    }

    #[cfg(test)]
    pub(crate) fn driven(&self) -> bool {
        self.temperature.control.is_finger_driven() || self.tint.control.is_finger_driven()
    }

    /// Положение шкалы оси в единицах положения (майреды со знаком / единицы оттенка): без округления.
    #[cfg(test)]
    pub(crate) fn position_of(&self, axis: WhiteAxis, now: ui_core::time::MonotonicNanos) -> f64 {
        match axis {
            WhiteAxis::Temperature => self.temperature.control.position_steps(now),
            WhiteAxis::Tint => self.tint.control.position_steps(now),
        }
    }
}

fn kelvin_limits(range: &WhiteRange) -> LinearLimits {
    LinearLimits {
        min: range.min_kelvin,
        max: range.max_kelvin,
    }
}

fn tint_limits(range: &WhiteRange, units_per_duv: f64) -> LinearLimits {
    let limit = range.max_abs_duv * units_per_duv;
    LinearLimits {
        min: -limit,
        max: limit,
    }
}

fn period(value: u32, name: &'static str) -> Result<NonZeroU32, WidgetError> {
    NonZeroU32::new(value).ok_or_else(|| WidgetError::Inconsistent(format!("период крупной риски {name} равен нулю")))
}

/// Отмена пана у шкалы любой модели (чтобы не плодить копии кода для двух осей).
trait CancelPan {
    fn cancel(&mut self, now: ui_core::time::MonotonicNanos) -> Result<(), ui_kit::scale::ScaleError>;
}

impl<M: RulerAxis> CancelPan for RulerControl<M> {
    fn cancel(&mut self, now: ui_core::time::MonotonicNanos) -> Result<(), ui_kit::scale::ScaleError> {
        self.apply_pan(now, PanUpdate::Cancelled)
    }
}

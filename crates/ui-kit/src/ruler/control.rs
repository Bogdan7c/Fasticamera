//! Элемент «линейка-ручка»: шкала со ступенями + затвор пальца + вид рисок.
//!
//! Значением владеет экран, линейка отвечает за физику. Два способа поставить значение программно:
//! * [`RulerControl::follow_grid_value`] — значение сразу ставится на ступень (Exposure: значения всегда на сетке);
//! * [`RulerControl::follow_exact_value`] — линейка показывает значение ТОЧНО, даже между ступенями (ручка вида
//!   после сброса к пресету 0,15 при шаге 0,1). Подпись значения рисует экран из своих данных, не из линейки.
//!
//! Оба способа не порождают намерений: движение идёт от программы, затвор пальца гасит эхо.
//!
//! # Модели
//! Линейка обобщена над моделью значений ([`RulerAxis`]), по умолчанию — ступенчатая [`QuantizedLinear`]
//! (Exposure, ручки вида: значение липнет к ступеням). С **непрерывной** моделью ([`crate::value::ContinuousLinear`],
//! [`crate::value::MiredKelvin`]) линейка работает иначе по построению, а не по флагу:
//! * значение от пальца не округляется, шкала после отпускания не доезжает до риски и не липнет к ней;
//! * программная установка ([`RulerControl::show_exact_value`]) показывает значение точно и не даёт намерений;
//! * инерция после броска остаётся продолжением пальца: пока шкала не `Settled`, её значения — намерения.

use std::num::NonZeroU32;

use ui_core::time::MonotonicNanos;
use ui_draw::draw_list::DrawItem;
use ui_draw::scene_bridge::NodePlacementPx;

use super::finger_gate::{FingerGate, GatedEvent};
use super::pacing::{HapticPacer, HapticPacing};
use crate::drive::{DragOrigin, GrowthDirection, ScrollTransition};
use crate::environment::ElementEnvironment;
use crate::pan::PanUpdate;
use crate::scale::{ScaleError, ScaleSpec, SnapPolicy, SnapScale, TickVisibility, ZeroMagnet};
use crate::tokens::{ColorTokens, RulerTokens, ScaleDensity};
use crate::value::{ExactPosition, Quantization, QuantizedLinear, RulerAxis};
use crate::view::{
    DimmedRange, DimmedSpan, MajorTickPeriod, PxPerPt, RulerMarkerStyle, RulerStopTickStyle, ViewError, ruler_items,
    ruler_items_with_dimmed, ruler_marker_item, ruler_stop_tick_item,
};

/// Ощущение линейки под пальцем. По умолчанию — прежнее поведение (Exposure, белое): отсчёт от точки распознавания
/// пана и щелчок на каждый кадр с пройденной риской. Ручки раздела калибровки включают другое явно.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct RulerFeel {
    /// От какой точки считать смещение пальца: [`DragOrigin::FromTouchDown`] — значение сразу догоняет палец
    /// (путь до распознавания пана не пропадает).
    pub drag_origin: DragOrigin,
    /// Темп щелчков отдачи.
    pub haptic_pacing: HapticPacing,
    /// Магнит нуля под пальцем (решение 284). По умолчанию выключен: Exposure, белое и зум идут за пальцем 1:1.
    pub zero_magnet: ZeroMagnet,
}

/// Описание линейки без чисел экрана: модель, плотность и период крупной риски.
#[derive(Debug, Clone)]
pub struct RulerSpec<M = QuantizedLinear> {
    /// Модель значений (шаг, пределы, опорное значение).
    pub model: M,
    /// Сколько pt занимает одна единица положения модели (токен плотности).
    pub density: ScaleDensity,
    /// Каждый какой шаг рисуется крупной риской. «Шаг» — расстояние между соседними рисками модели
    /// ([`RulerAxis::tick_step_units`]).
    pub major_every_steps: NonZeroU32,
    /// Ощущение под пальцем (по умолчанию — прежнее).
    pub feel: RulerFeel,
}

/// Линейка-ручка. Палец ведёт её горизонтально: палец влево — значение растёт (как линейка Exposure).
pub struct RulerControl<M: RulerAxis = QuantizedLinear> {
    scale: SnapScale<M>,
    gate: FingerGate,
    major_period: MajorTickPeriod,
    pacer: HapticPacer,
}

impl<M: RulerAxis> RulerControl<M> {
    /// Линейка со значением `initial_value` (у ступенчатой модели — на ступени). `environment.viewport_extent_points`
    /// — ширина зоны пана.
    ///
    /// Прилипание выбирает модель: ступенчатая липнет к каждой ступени, непрерывная останавливается где угодно
    /// (магнит включён только для опорных рисок, а у непрерывных осей этого пакета их нет).
    pub fn new(
        spec: RulerSpec<M>,
        initial_value: M::Value,
        environment: &ElementEnvironment<'_>,
        now: MonotonicNanos,
    ) -> Result<Self, ScaleError> {
        let snap = match spec.model.quantization() {
            Quantization::Stepped => SnapPolicy::EveryStep,
            Quantization::Continuous => SnapPolicy::ContinuousWithMagnets,
        };
        let scale_spec = ScaleSpec {
            direction: GrowthDirection::FingerLeft,
            snap,
            density: spec.density,
            ticks: TickVisibility::Visible,
            drag_origin: spec.feel.drag_origin,
        };
        Ok(Self {
            scale: SnapScale::new(spec.model, initial_value, scale_spec, environment, now)?
                .with_zero_magnet(spec.feel.zero_magnet)?,
            gate: FingerGate::default(),
            major_period: MajorTickPeriod::every_steps(spec.major_every_steps),
            pacer: HapticPacer::new(spec.feel.haptic_pacing, environment.tokens)?,
        })
    }

    /// Значение линейки сейчас, с учётом идущего движения (у ступенчатой модели — ступень).
    pub fn value(&self, now: MonotonicNanos) -> M::Value {
        self.scale.value(now)
    }

    /// Положение в единицах модели без округления: по нему видно, где линейка стоит между ступенями.
    pub fn position_steps(&self, now: MonotonicNanos) -> f64 {
        self.scale.position_units(now)
    }

    /// Сколько щелчков отдачи подавил ограничитель частоты (0, если темп не ограничен): для диагностики и тестов.
    pub fn suppressed_haptics(&self) -> u64 {
        self.pacer.suppressed_count()
    }

    /// Пределы линейки сейчас.
    pub fn limits(&self) -> &M::Limits {
        self.scale.model().limits()
    }

    /// Ведёт ли линейку палец.
    pub fn is_finger_driven(&self) -> bool {
        self.gate.is_finger_driven()
    }

    /// Сколько значений погашено как эхо программных перемещений.
    pub fn suppressed_echo_events(&self) -> u64 {
        self.gate.suppressed_echo_events()
    }

    /// Пан пальца. `Began` отдаёт линейку пальцу; обновление уже должно быть в осях линейки
    /// (экран переводит поворот раздела до вызова).
    pub fn apply_pan(&mut self, time: MonotonicNanos, update: PanUpdate) -> Result<(), ScaleError> {
        if matches!(update, PanUpdate::Began { .. }) {
            self.gate.finger_took_over();
        }
        self.scale.apply_pan(time, update)
    }

    /// Новые пределы (другая камера): линейка переезжает, движение программное.
    pub fn retarget_limits(&mut self, limits: M::Limits, now: MonotonicNanos) -> Result<(), ScaleError> {
        if self.scale.model().limits() == &limits {
            return Ok(());
        }
        self.scale.set_limits(limits, ScrollTransition::Animated, now)?;
        self.gate.program_took_over();
        Ok(())
    }

    /// Кадр: события линейки через затвор пальца.
    pub fn advance(&mut self, now: MonotonicNanos) -> Vec<GatedEvent<M::Value>> {
        self.scale.advance(now);
        let events = self.scale.drain_events();
        let routed = self.gate.route(events);
        self.pacer.filter(routed, now)
    }

    /// Риски и жёлтая отметка внутри размещения `placement` узла зоны пана.
    pub fn items(
        &self,
        placement: &NodePlacementPx,
        now: MonotonicNanos,
        px_per_pt: PxPerPt,
        ruler_tokens: &RulerTokens,
        colors: &ColorTokens,
    ) -> Result<Vec<DrawItem>, ViewError> {
        ruler_items(
            placement,
            &self.scale,
            now,
            px_per_pt,
            ruler_tokens,
            self.major_period,
            colors,
        )
    }
}

impl<M: RulerAxis> RulerControl<M> {
    /// Риски и жёлтая отметка, где риски внутри `dimmed` тусклее (ход ручки за упором). Пустой список — то же, что
    /// [`Self::items`], до бита. Это только рисунок: значение и жесты промежутки не меняют.
    pub fn items_with_dimmed_ranges(
        &self,
        placement: &NodePlacementPx,
        now: MonotonicNanos,
        px_per_pt: PxPerPt,
        (ruler_tokens, colors): (&RulerTokens, &ColorTokens),
        dimmed: &[DimmedRange<M::Value>],
    ) -> Result<Vec<DrawItem>, ViewError> {
        let spans = dimmed
            .iter()
            .map(|range| self.dimmed_span_of(range))
            .collect::<Result<Vec<_>, _>>()?;
        ruler_items_with_dimmed(
            placement,
            &self.scale,
            now,
            px_per_pt,
            ruler_tokens,
            self.major_period,
            colors,
            &spans,
        )
    }

    /// Риска-упор на значении `value`: толще и длиннее обычной. `None` — упор сейчас вне видимой части линейки.
    /// Только рисунок: не даёт действий и не магнитит.
    pub fn stop_tick_item(
        &self,
        placement: &NodePlacementPx,
        now: MonotonicNanos,
        px_per_pt: PxPerPt,
        ruler_tokens: &RulerTokens,
        (value, style): (M::Value, RulerStopTickStyle),
    ) -> Result<Option<DrawItem>, ViewError> {
        let position_units = self
            .scale
            .model()
            .position_of(value)
            .map_err(|_| ViewError::UnrepresentableOverlayValue)?;
        ruler_stop_tick_item(
            placement,
            &self.scale,
            now,
            px_per_pt,
            ruler_tokens,
            (position_units, style),
        )
    }

    fn dimmed_span_of(&self, range: &DimmedRange<M::Value>) -> Result<DimmedSpan, ViewError> {
        let position_of = |value: M::Value| {
            self.scale
                .model()
                .position_of(value)
                .map_err(|_| ViewError::UnrepresentableOverlayValue)
        };
        Ok(DimmedSpan {
            from_units: position_of(range.from)?,
            to_units: position_of(range.to)?,
            alpha_factor: range.alpha_factor,
        })
    }

    /// Программная метка на значении `value` (например, «значение пресета»): точка под рядом рисок. `None` — метка
    /// сейчас вне видимой части линейки. Метка не даёт действий, не магнитит и не двигает линейку: это рисунок.
    pub fn marker_item(
        &self,
        placement: &NodePlacementPx,
        now: MonotonicNanos,
        px_per_pt: PxPerPt,
        ruler_tokens: &RulerTokens,
        (value, style): (M::Value, RulerMarkerStyle),
    ) -> Result<Option<DrawItem>, ViewError> {
        let position_units = self
            .scale
            .model()
            .position_of(value)
            .map_err(|_| ViewError::UnrepresentableMarkerValue)?;
        ruler_marker_item(
            placement,
            &self.scale,
            now,
            px_per_pt,
            ruler_tokens,
            (position_units, style),
        )
    }
}

impl<M: ExactPosition + RulerAxis> RulerControl<M> {
    /// Показать значение `target` ТОЧНО: линейка едет к положению значения без округления и не даёт намерений
    /// (движение программное, затвор пальца гасит эхо). Если линейка уже стоит там (в пределах
    /// `tolerance_units` единиц положения) или её держит палец — ничего не делает, поэтому значение, которое экран
    /// держит неизменным, не перепосылается каждый кадр.
    ///
    /// Значение вне пределов зажимается к границе; значение, которое модель не может представить (не число),
    /// — ошибка, шкала не двигается.
    pub fn show_exact_value(
        &mut self,
        target: M::Value,
        tolerance_units: f64,
        now: MonotonicNanos,
    ) -> Result<(), ScaleError> {
        if self.gate.is_finger_driven() {
            return Ok(());
        }
        let target_position = self.scale.model().position_of(target)?;
        if (self.scale.position_units(now) - target_position).abs() <= tolerance_units {
            return Ok(());
        }
        self.gate.program_took_over();
        self.scale.set_value(target, ScrollTransition::Animated, now)?;
        Ok(())
    }
}

impl RulerControl<QuantizedLinear> {
    /// Шаг модели.
    pub fn step(&self) -> f64 {
        self.scale.model().step()
    }

    /// Значение экрана `target` лежит на сетке: если линейка разошлась с ним больше `tolerance` и палец её не
    /// держит, она едет к ступени `target`.
    pub fn follow_grid_value(&mut self, target: f64, tolerance: f64, now: MonotonicNanos) -> Result<(), ScaleError> {
        if self.gate.is_finger_driven() || (self.scale.value(now) - target).abs() <= tolerance {
            return Ok(());
        }
        self.gate.program_took_over();
        self.scale.set_value(target, ScrollTransition::Animated, now)?;
        Ok(())
    }

    /// Значение экрана `target` может лежать МЕЖДУ ступенями: линейка едет к точному положению. Если она уже
    /// стоит там (в пределах `tolerance_steps` шагов) или её держит палец — ничего не делает, поэтому
    /// значение между ступенями не перепосылается каждый кадр.
    pub fn follow_exact_value(
        &mut self,
        target: f64,
        tolerance_steps: f64,
        now: MonotonicNanos,
    ) -> Result<(), ScaleError> {
        let target_steps = target / self.step();
        if self.gate.is_finger_driven() || (self.scale.position_units(now) - target_steps).abs() <= tolerance_steps {
            return Ok(());
        }
        self.gate.program_took_over();
        self.scale.show_position(target_steps, ScrollTransition::Animated, now)
    }
}

//! Механика рамки фокуса: этапы, удержание, «солнце», снимок кадра.

use ui_core::geometry::{Point, Rect, Size};
use ui_core::time::{MonotonicNanos, MotionPreference, UiDuration, add_duration};

use super::appear::TimedAppearance;
use super::error::FocusFrameError;
use super::spec::{FocusFrameSpec, FocusHold};
use crate::drive::{MotionPhase, ScrollTransition};
use crate::environment::ElementEnvironment;
use crate::pan::PanUpdate;
use crate::progress::{SpringProgress, SpringSource};
use crate::scale::{ScaleEvent, ScaleSnapshot, SetValueOutcome, SnapScale};
use crate::tokens::{FocusTokens, MotionToken};
use crate::value::ValueModel;

/// Этап жизни рамки.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusPhase {
    /// Рамки нет.
    Hidden,
    /// Рамка видна в полную силу.
    Active,
    /// Рамка притихла перед исчезновением.
    Dimmed,
    /// Рамка исчезает.
    Fading,
}

/// Событие рамки. События шкалы «солнца» идут вслед за собственными событиями рамки
/// (порядок между двумя группами не сохраняется, внутри групп — сохраняется).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FocusFrameEvent<V> {
    /// Рамка потускнела по таймеру.
    Dimmed,
    /// Рамка полностью исчезла.
    Hidden,
    /// Событие шкалы «солнца» (значение, тик, хаптика, успокоение).
    Sun(ScaleEvent<V>),
}

/// Что делает палец с «солнцем» в момент выборки: от этого зависит направляющая линия.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SunInteraction {
    /// Палец не на «солнце» (в том числе «солнце» ещё доезжает само после отпускания).
    Idle,
    /// Палец на «солнце» (тянет его или держит): рисуется вертикальная направляющая.
    Dragged,
}

/// Срез состояния рамки для рисования и проверок.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FocusFrameSnapshot<V> {
    pub phase: FocusPhase,
    /// Центр рамки, pt (последнее данное через `place`).
    pub center: Point,
    /// Масштаб рамки и «солнца» вокруг центра: падает от `appear_scale` к 1.
    pub scale: f64,
    /// Прозрачность рамки и «солнца» 0..1.
    pub opacity: f32,
    /// Смещение «солнца» вверх от исходного положения, pt (вниз — отрицательное): значение шкалы
    /// относительно момента последнего `place`.
    pub sun_travel_up_pt: f64,
    /// Палец на «солнце» или нет (направляющая линия видна только при `Dragged`).
    pub sun_interaction: SunInteraction,
    pub sun: ScaleSnapshot<V>,
}

/// Рамка фокуса с «солнцем» экспозиции.
pub struct FocusFrame<M: ValueModel + Clone> {
    tokens: FocusTokens,
    hold: FocusHold,
    springs: SpringSource,
    /// Появление по времени: масштаб и проявление (см. [`TimedAppearance`]).
    appearance: TimedAppearance,
    /// Прозрачность удержания 0..1 (значения, не прогресс): 1 у живой рамки, `dimmed_opacity` у
    /// притихшей, 0 у исчезнувшей. Проявление при появлении считает `appearance`.
    opacity: SpringProgress,
    phase: FocusPhase,
    center: Point,
    /// Момент последней активности: от него считаются сроки удержания.
    idle_since: MonotonicNanos,
    /// Смещение шкалы «солнца» в момент `place`: от него считается ход «солнца».
    sun_anchor_offset_points: f64,
    sun: SnapScale<M>,
    events: Vec<FocusFrameEvent<M::Value>>,
}

impl<M: ValueModel + Clone> FocusFrame<M> {
    /// Пружина прозрачности: `.smooth`, без перелёта.
    const OPACITY_SPRING: MotionToken = MotionToken::Smooth;

    /// Создать скрытую рамку. `sun_model`/`sun_initial` — модель значений «солнца» (экран передаёт
    /// ту же, что у шкалы EV). `environment.viewport_extent_points` — высота области «солнца».
    pub fn new(
        sun_model: M,
        sun_initial: M::Value,
        spec: FocusFrameSpec,
        environment: &ElementEnvironment<'_>,
        now: MonotonicNanos,
    ) -> Result<Self, FocusFrameError> {
        spec.validate()?;
        environment.tokens.validate()?;
        let springs = SpringSource::new(&environment.tokens.motion, environment.motion);
        let sun = SnapScale::new(sun_model, sun_initial, spec.sun, environment, now)?;
        Ok(Self {
            tokens: environment.tokens.focus,
            hold: spec.hold,
            appearance: TimedAppearance::new(
                environment.tokens.focus.appear_scale,
                environment.tokens.focus.appear_duration_s,
                environment.motion,
            ),
            opacity: SpringProgress::at_rest(0.0, springs.spring(Self::OPACITY_SPRING)?, now)?,
            springs,
            phase: FocusPhase::Hidden,
            center: Point::ORIGIN,
            idle_since: now,
            sun_anchor_offset_points: sun.snapshot(now).offset_points,
            sun,
            events: Vec::new(),
        })
    }

    /// Рамка узла для вёрстки: размер из токенов вокруг `center`.
    pub fn node_frame(&self, center: Point) -> Rect {
        Rect::centered_at(
            center,
            Size::new(self.tokens.frame_width_pt, self.tokens.frame_height_pt),
        )
    }

    pub fn phase(&self) -> FocusPhase {
        self.phase
    }

    pub fn hold(&self) -> FocusHold {
        self.hold
    }

    /// Поставить рамку в точку `center` (тап или внешний источник). Рамка появляется заново
    /// (масштаб с увеличенного к 1), срок удержания отсчитывается от `now`, «солнце» берёт
    /// текущее значение за исходное положение.
    pub fn place(&mut self, center: Point, now: MonotonicNanos) -> Result<(), FocusFrameError> {
        if !center.x.is_finite() || !center.y.is_finite() {
            return Err(FocusFrameError::InvalidPoint(center));
        }
        // Удержание сразу полное: проявление при появлении ведёт `appearance`, а не пружина.
        self.opacity = SpringProgress::at_rest(1.0, self.springs.spring(Self::OPACITY_SPRING)?, now)?;
        self.appearance.restart(now);
        self.center = center;
        self.phase = FocusPhase::Active;
        self.idle_since = now;
        self.sun_anchor_offset_points = self.sun.snapshot(now).offset_points;
        Ok(())
    }

    /// Сменить правило удержания. Срок отсчитывается заново от `now`; притихшая рамка
    /// возвращается в полную силу (её только что «взяли»).
    pub fn set_hold(&mut self, hold: FocusHold, now: MonotonicNanos) -> Result<(), FocusFrameError> {
        hold.validate()?;
        self.hold = hold;
        self.keep_alive(now)
    }

    /// Была активность (жест на «солнце», внешний сигнал): срок удержания отсчитывается заново,
    /// притихшая рамка возвращается в полную силу. Скрытой или исчезающей рамке не нужна.
    pub fn keep_alive(&mut self, now: MonotonicNanos) -> Result<(), FocusFrameError> {
        if !matches!(self.phase, FocusPhase::Active | FocusPhase::Dimmed) {
            return Ok(());
        }
        self.idle_since = now;
        if self.phase == FocusPhase::Dimmed {
            self.phase = FocusPhase::Active;
            let fade = self.springs.spring(Self::OPACITY_SPRING)?;
            self.opacity.retarget(fade, 1.0, now)?;
        }
        Ok(())
    }

    /// Убрать рамку сейчас (смена камеры, снимок сделан): прозрачность уходит в 0.
    pub fn dismiss(&mut self, now: MonotonicNanos) -> Result<(), FocusFrameError> {
        if matches!(self.phase, FocusPhase::Hidden | FocusPhase::Fading) {
            return Ok(());
        }
        self.begin_fading(now)
    }

    fn begin_fading(&mut self, at: MonotonicNanos) -> Result<(), FocusFrameError> {
        let fade = self.springs.spring(Self::OPACITY_SPRING)?;
        self.opacity.retarget(fade, 0.0, at)?;
        self.phase = FocusPhase::Fading;
        Ok(())
    }

    // ---- «солнце» ----

    /// Жест пана на «солнце». Считается активностью: срок удержания обновляется, притихшая или
    /// исчезающая рамка возвращается в полную силу (см. [`Self::hold_for_sun_gesture`]).
    pub fn apply_pan(&mut self, time: MonotonicNanos, update: PanUpdate) -> Result<(), FocusFrameError> {
        self.require_visible(time)?;
        self.sun.apply_pan(time, update)?;
        self.hold_for_sun_gesture(time)
    }

    /// Палец, поймавший «солнце», отпущен без перетаскивания.
    pub fn release_sun_without_drag(&mut self, now: MonotonicNanos) -> Result<(), FocusFrameError> {
        self.require_visible(now)?;
        self.sun.release_without_drag(now)?;
        self.hold_for_sun_gesture(now)
    }

    /// Жест на «солнце» держит рамку: как в iOS, палец на «солнце» не даёт рамке исчезнуть.
    ///
    /// Выбор (а не отказ `NotVisible`): «солнце» видно, пока рамка исчезает, и жест на нём
    /// меняет экспозицию; если рамка при этом пропадёт, пользователь крутит невидимую шкалу.
    /// Поэтому исчезающая рамка возвращается в `Active` (прозрачность снова к 1, срок удержания
    /// заново), а событие `Hidden` не выдаётся — рамка так и не скрылась. Полностью скрытая
    /// рамка отвергается в [`Self::require_visible`].
    fn hold_for_sun_gesture(&mut self, now: MonotonicNanos) -> Result<(), FocusFrameError> {
        if self.phase == FocusPhase::Fading {
            let fade = self.springs.spring(Self::OPACITY_SPRING)?;
            self.opacity.retarget(fade, 1.0, now)?;
            self.phase = FocusPhase::Active;
            self.idle_since = now;
            return Ok(());
        }
        self.keep_alive(now)
    }

    /// Установить значение «солнца» программно (сброс экспозиции при новом фокусе). Вибрации нет.
    pub fn set_sun_value(
        &mut self,
        value: M::Value,
        transition: ScrollTransition,
        now: MonotonicNanos,
    ) -> Result<SetValueOutcome<M::Value>, FocusFrameError> {
        Ok(self.sun.set_value(value, transition, now)?)
    }

    /// Сменить пределы «солнца» без пересоздания рамки (другая камера, другой диапазон EV).
    /// Положение рамки, фаза и сроки удержания не меняются. Значение вне новых пределов становится
    /// крайним допустимым; вибрации нет (смена программная). Атомарно: при ошибке «солнце» не меняется.
    pub fn set_sun_limits(
        &mut self,
        limits: M::Limits,
        transition: ScrollTransition,
        now: MonotonicNanos,
    ) -> Result<(), FocusFrameError> {
        Ok(self.sun.set_limits(limits, transition, now)?)
    }

    /// Шкала «солнца» только для чтения (значение, пределы).
    pub fn sun(&self) -> &SnapScale<M> {
        &self.sun
    }

    /// Рамка должна быть видна. Сперва фиксируем уже завершившееся исчезновение: рамка, чья
    /// прозрачность дошла до 0 к `now`, скрыта (с событием `Hidden`), даже если кадр это ещё не заметил.
    fn require_visible(&mut self, now: MonotonicNanos) -> Result<(), FocusFrameError> {
        self.settle_fade(now);
        if matches!(self.phase, FocusPhase::Hidden) {
            Err(FocusFrameError::NotVisible)
        } else {
            Ok(())
        }
    }

    /// Reduce Motion включили или выключили на лету.
    pub fn set_motion(&mut self, motion: MotionPreference) {
        self.springs.set_motion(motion);
        self.appearance.set_motion(motion);
        self.sun.set_motion(motion);
    }

    // ---- кадр, события, снимок ----

    /// Раз в кадр: сроки удержания, успокоение исчезновения, шкала «солнца».
    pub fn advance(&mut self, now: MonotonicNanos) -> Result<(), FocusFrameError> {
        self.sun.advance(now);
        if let FocusHold::AutoFade { dim_after, hide_after } = self.hold {
            // Этапы стартуют в момент дедлайна, а не кадра: положение не зависит от частоты кадров.
            if self.phase == FocusPhase::Active && self.deadline_passed(dim_after, now) {
                let at = add_duration(self.idle_since, dim_after);
                let fade = self.springs.spring(Self::OPACITY_SPRING)?;
                self.opacity.retarget(fade, self.tokens.dimmed_opacity, at)?;
                self.phase = FocusPhase::Dimmed;
                self.events.push(FocusFrameEvent::Dimmed);
            }
            if matches!(self.phase, FocusPhase::Active | FocusPhase::Dimmed) && self.deadline_passed(hide_after, now) {
                self.begin_fading(add_duration(self.idle_since, hide_after))?;
            }
        }
        self.settle_fade(now);
        Ok(())
    }

    /// Исчезновение закончилось к `now`: рамка скрыта, событие `Hidden` выдаётся ровно один раз.
    fn settle_fade(&mut self, now: MonotonicNanos) {
        if self.phase == FocusPhase::Fading && !self.opacity.is_moving(now) {
            self.phase = FocusPhase::Hidden;
            self.events.push(FocusFrameEvent::Hidden);
        }
    }

    fn deadline_passed(&self, after: UiDuration, now: MonotonicNanos) -> bool {
        add_duration(self.idle_since, after) <= now
    }

    /// Забрать накопленные события (очередь очищается): сначала события рамки, затем «солнца».
    pub fn drain_events(&mut self) -> Vec<FocusFrameEvent<M::Value>> {
        let mut drained = std::mem::take(&mut self.events);
        drained.extend(self.sun.drain_events().into_iter().map(FocusFrameEvent::Sun));
        drained
    }

    /// Всё для рисования за один вызов.
    pub fn snapshot(&self, now: MonotonicNanos) -> FocusFrameSnapshot<M::Value> {
        let sun = self.sun.snapshot(now);
        FocusFrameSnapshot {
            phase: self.phase,
            center: self.center,
            scale: self.appearance.scale(now),
            opacity: self.opacity.value(now).clamp(0.0, 1.0) as f32 * self.appearance.opacity(now),
            sun_travel_up_pt: sun.offset_points - self.sun_anchor_offset_points,
            sun_interaction: match sun.phase {
                MotionPhase::Touched => SunInteraction::Dragged,
                MotionPhase::Resting | MotionPhase::Settling => SunInteraction::Idle,
            },
            sun,
        }
    }
}

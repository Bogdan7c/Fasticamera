//! Исполнитель шагов сценария `zoom` (сессия 13): чистая машина состояний «тик наблюдения → что положить в экран».
//!
//! Исполнитель ничего не вызывает сам. На каждом тике он получает [`ZoomObservation`] — срез того, что видно
//! снаружи (события и статус движка, кольцо показанных кадров, счётчики непрерывности, отчёт экрана), — и возвращает
//! [`ZoomTick`]: что вложить в экран (действие или образец касания) и в каком состоянии прогон. Вложение выполняет
//! сценарий-обвязка (`zoom.rs`); благодаря этому все правила проверяются тестами на подставных наблюдениях.
//!
//! # Шаг
//! 1. **Начало:** по состоянию движка выбирается ожидание ([`SwitchExpectation`]); запоминаются базовые линии
//!    (счётчики кадров, вибраций, намерений) и момент действия на шкале рендера.
//! 2. **Ожидание:** событие `LensSwitched` с подходящей ролью, затем первый ЖИВОЙ кадр новой камеры в кольце кадров.
//!    Шаг без смены объектива ждёт только времени. Любой `LensSwitchFailed`, неожиданная смена или срок шага —
//!    провал шага и прогона.
//! 3. **Наблюдение после:** `step_settle_ms` исполнитель продолжает смотреть (чёрные и удержанные кадры, частота,
//!    зеркало), затем записывает [`ZoomStepRecord`] и берёт следующий шаг.
//!
//! Одна шкала времени: действие и «первый живой кадр» меряются по времени кадров рендера (`render_now`), а время
//! события движка и срок шага — по часам маршрутизатора (`Instant`); смешивать шкалы нельзя.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use camera_contract::ids::{CameraId, LensFacing, LensRole};
use camera_core::CameraEvent;
use camera_screen::ScreenAction;
use render_loop::viewfinder::{ContinuityCounters, ViewfinderCounters};
use shell_contract::clock::MonotonicNanos;
use shell_contract::touch::{PointerId, TouchPhase};

use super::zoom_home::{QueuedStep, StepRole, steps_to_home, stray_event_failure};
use super::zoom_plan::{SwitchExpectation, ZoomAction, ZoomStep, expectation_for_tap, nearest_stop};
use super::zoom_presented::{ContinuityDelta, presented_histogram};
use super::zoom_record::{
    MirrorCheck, RingWatch, SwitchTiming, ZoomStepRecord, first_live_frame_ms, haptic_window, mirror_check,
};
use super::zoom_reveal::RevealFaults;
use super::zoom_seamless::SeamlessProgress;
use super::zoom_seamless_fingers::Fingers;
use super::zoom_seamless_record::{SeamlessRecord, SwitchGap, switch_gap};
pub use super::zoom_tick::{Injection, RunnerState, TouchInjection, ZoomObservation, ZoomTick};
pub use super::zoom_views::{EngineView, ScreenView, camera_of_role};
use crate::zoom_scenario_config::ZoomScenarioConfig;

/// Допуск «экран показывает то значение, которое выбрали»: 2 % коэффициента в тысячных.
const SCREEN_RATIO_TOLERANCE_MILLI: u32 = 20;
/// Имя роли для отчёта (совпадает с serde `LensRole`).
fn role_name(role: LensRole) -> &'static str {
    role.name()
}

/// Базовые линии на начало шага: приращения считаются от них.
struct Baseline {
    counters: ContinuityCounters,
    isp_counters: ViewfinderCounters,
    haptics_total: u64,
    lens_published: u64,
    active_facing: Option<LensFacing>,
}

/// Идущий шаг.
struct ActiveStep {
    step: ZoomStep,
    role: StepRole,
    expectation: SwitchExpectation,
    started_at: Instant,
    action_render: MonotonicNanos,
    baseline: Baseline,
    /// Коэффициент, который должен показать экран после тапа (в тысячных).
    target_ratio_milli: Option<u32>,
    switch: Option<SwitchTiming>,
    /// Камера ПЕРВОЙ смены шага: от неё меряются «первый живой кадр» и разрыв показа.
    new_camera: Option<CameraId>,
    /// Камера ПОСЛЕДНЕЙ смены шага: на ней шаг заканчивается, её кадры видны в кольце в конце шага, по ним
    /// проверяется зеркало. У шагов с одной сменой совпадает с `new_camera`; у «зависания у порога» смен много,
    /// и кадры первой камеры к концу шага уже вытеснены из кольца.
    last_switched_camera: Option<CameraId>,
    first_live_ms: Option<f64>,
    retargets: u32,
    notes: Vec<String>,
    drag: Option<DragProgress>,
    /// Часть бесшовного жеста 13b (`None` — обычный шаг).
    seamless: Option<SeamlessProgress>,
    /// Сколько событий `LensSwitched` пришло за шаг.
    switch_events: u32,
    /// Что показывал экран, когда сценарий увидел первое событие смены объектива (тысячные).
    ratio_at_switch_milli: Option<u32>,
    /// Разрыв показа при смене камеры (измеряется в тот тик, когда новая камера впервые показана живой).
    gap: Option<SwitchGap>,
    /// Когда наблюдение «после» кончается; `None` — ещё ждём смену/кадр.
    settle_until: Option<Instant>,
}

/// Исполнитель шагов.
pub struct ZoomRunner {
    config: ZoomScenarioConfig,
    pending: VecDeque<QueuedStep>,
    active: Option<ActiveStep>,
    records: Vec<ZoomStepRecord>,
    ring: RingWatch,
    /// Итог прогона, когда он уже известен: `AllDone` или `Failed`; дальше исполнитель только повторяет его.
    outcome: Option<RunnerState>,
    /// Приводить ли экран к дому (Back 1×) в начале и после провала; см. [`ZoomRunner::anchored_at_back_one_x`].
    anchored: bool,
    /// Подготовка к дому уже поставлена в очередь.
    prepared: bool,
    /// Провал, о котором скажем после возврата к дому (идёт возврат).
    deferred_failure: Option<String>,
    /// Имя последнего законченного шага плана: в тексте провала про «чужие» события.
    last_step_name: Option<String>,
    /// С какого момента подготовка ждёт, пока экран успокоится (колесо закрыто, плавного перехода нет).
    calming_since: Option<Instant>,
    /// Шаги плана, которые не запускались из-за провала раньше.
    skipped: Vec<String>,
    /// Пальцы бесшовного жеста, которые лежат на экране между его частями.
    held: Option<Fingers>,
}

impl ZoomRunner {
    pub fn new(config: ZoomScenarioConfig, steps: Vec<ZoomStep>) -> Self {
        Self {
            config,
            pending: steps
                .into_iter()
                .map(|step| QueuedStep::new(step, StepRole::Planned))
                .collect(),
            active: None,
            records: Vec::new(),
            ring: RingWatch::default(),
            outcome: None,
            anchored: false,
            prepared: false,
            deferred_failure: None,
            last_step_name: None,
            calming_since: None,
            skipped: Vec::new(),
            held: None,
        }
    }

    /// Привязать прогон к «дому»: в начале экран приводится к задней основной камере на 1× с закрытым колесом, а после
    /// провала шага исполнитель возвращает его туда же, прежде чем сообщить провал (см. `zoom_home`). Тесты правил
    /// отдельных шагов работают без привязки: они подают нужное состояние сами.
    pub fn anchored_at_back_one_x(mut self) -> Self {
        self.anchored = true;
        self
    }

    /// Записи законченных шагов (и провалившегося, если был).
    pub fn records(&self) -> &[ZoomStepRecord] {
        &self.records
    }

    /// Имена шагов плана, до которых дело не дошло (включая идущий). Подготовка и возврат к дому — не шаги плана.
    pub fn unfinished_names(&self) -> Vec<String> {
        let running = self
            .active
            .iter()
            .filter(|active| active.role == StepRole::Planned)
            .map(|active| active.step.name.clone());
        let waiting = self
            .pending
            .iter()
            .filter(|queued| queued.role == StepRole::Planned)
            .map(|queued| queued.step.name.clone());
        running.chain(waiting).chain(self.skipped.iter().cloned()).collect()
    }

    /// Один тик наблюдения.
    pub fn tick(&mut self, observation: &ZoomObservation<'_>) -> ZoomTick {
        self.ring.absorb(observation.frames);
        if let Some(outcome) = &self.outcome {
            return ZoomTick::idle(outcome.clone());
        }
        match self.active.take() {
            None => self.begin_next(observation),
            Some(active) => self.advance(active, observation),
        }
    }

    // ---- начало шага -------------------------------------------------------------------------------------------

    fn begin_next(&mut self, observation: &ZoomObservation<'_>) -> ZoomTick {
        self.queue_preparation_once(observation);
        // События движка, которых не ждал ни один шаг (между шагами и после последнего), тоже судятся. Во время
        // возврата к дому провал уже случился: его события второй раз не разбираем.
        if self.deferred_failure.is_none() {
            let context = match &self.last_step_name {
                Some(name) => format!("после шага {name}"),
                None => "до первого шага".to_owned(),
            };
            if let Some(reason) = stray_event_failure(observation.events, &context) {
                let record = ZoomStepRecord::not_started("unjudged_events", &reason);
                return self.step_failed(observation, StepRole::Planned, Some(record), "unjudged_events", &reason);
            }
        }
        let Some(QueuedStep { step, role }) = self.pending.pop_front() else {
            return self.conclude_run();
        };
        let engine = observation.engine;
        if !engine.streaming {
            return self.fail_before_start(observation, role, step, "движок не отдаёт поток: шаг начать нельзя");
        }
        if role == StepRole::Preparation && (observation.screen.zoom.wheel_open || observation.screen.zoom.gliding) {
            // Приведение к дому ждёт, пока колесо закроется, а плавный переход дойдёт до упора: тап поверх движения
            // был бы перебит инерцией.
            let since = *self.calming_since.get_or_insert(observation.now);
            if observation.now.saturating_duration_since(since) >= Duration::from_millis(self.config.step_timeout_ms) {
                return self.fail_before_start(
                    observation,
                    role,
                    step,
                    "экран не успокоился: колесо открыто или идёт переход",
                );
            }
            self.pending.push_front(QueuedStep::new(step, role));
            return ZoomTick::idle(RunnerState::Running);
        }
        let current_role = engine.active.as_ref().map(|active| active.role);
        let mut target_ratio_milli = None;
        let (expectation, injections, drag, seamless) = match step.action {
            ZoomAction::TapStop { ratio } => {
                let Some(group) = engine.catalog.back.as_ref() else {
                    return self.fail_before_start(observation, role, step, "в каталоге зума нет задних камер");
                };
                let Some((index, kind)) = nearest_stop(group, ratio) else {
                    return self.fail_before_start(
                        observation,
                        role,
                        step,
                        &format!("на ленте нет кнопки для {ratio}×"),
                    );
                };
                target_ratio_milli = Some((f64::from(group.stops[index].ratio.value()) * 1000.0).round() as u32);
                (
                    expectation_for_tap(kind, current_role),
                    vec![Injection::action(ScreenAction::ZoomStopTapped {
                        stop: index,
                        at: observation.render_now,
                    })],
                    None,
                    None,
                )
            }
            ZoomAction::Flip => {
                let to_front = engine
                    .active
                    .as_ref()
                    .is_some_and(|active| active.facing == LensFacing::Back);
                let expectation = if to_front {
                    SwitchExpectation::ToRole(LensRole::Front)
                } else {
                    SwitchExpectation::ToAnyBack
                };
                (
                    expectation,
                    vec![Injection::action(ScreenAction::FlipCameraRequested {
                        at: observation.render_now,
                    })],
                    None,
                    None,
                )
            }
            ZoomAction::WheelDrag => {
                let start = TouchInjection {
                    pointer: PointerId::new(0),
                    phase: TouchPhase::Began,
                    point: self.config.wheel_drag.start,
                    time: observation.render_now,
                };
                let drag = DragProgress {
                    moved: 0,
                    ended: false,
                    next_at: observation.now + Duration::from_millis(self.config.wheel_drag.sample_interval_ms),
                };
                (
                    SwitchExpectation::NoSwitch,
                    vec![Injection::Touch(start)],
                    Some(drag),
                    None,
                )
            }
            ZoomAction::Seamless { gesture, leg } => match self.begin_seamless(observation, gesture, leg) {
                Ok((expectation, injections, progress)) => (expectation, injections, None, Some(progress)),
                Err(reason) => return self.fail_before_start(observation, role, step, &reason),
            },
        };
        self.ring.reset_overrun();
        self.active = Some(ActiveStep {
            role,
            expectation,
            started_at: observation.now,
            action_render: observation.render_now,
            baseline: Baseline {
                counters: observation.counters.clone(),
                isp_counters: observation.isp_counters.clone(),
                haptics_total: observation.screen.haptics.total(),
                lens_published: observation.screen.lens_published,
                active_facing: engine.active.as_ref().map(|active| active.facing),
            },
            target_ratio_milli,
            switch: None,
            new_camera: None,
            last_switched_camera: None,
            first_live_ms: None,
            retargets: 0,
            notes: Vec::new(),
            drag,
            seamless,
            switch_events: 0,
            ratio_at_switch_milli: None,
            gap: None,
            settle_until: None,
            step,
        });
        ZoomTick {
            injections,
            state: RunnerState::Running,
        }
    }

    fn fail_before_start(
        &mut self,
        observation: &ZoomObservation<'_>,
        role: StepRole,
        step: ZoomStep,
        reason: &str,
    ) -> ZoomTick {
        let record = ZoomStepRecord::not_started(&step.name, reason);
        self.step_failed(observation, role, Some(record), &step.name, reason)
    }

    // ---- дом: подготовка, возврат, итог -------------------------------------------------------------------------

    /// В первый раз перед планом ставит шаги приведения к дому (если прогон привязан к дому и экран не дома).
    fn queue_preparation_once(&mut self, observation: &ZoomObservation<'_>) {
        if !self.anchored || self.prepared || self.deferred_failure.is_some() {
            return;
        }
        self.prepared = true;
        let steps = steps_to_home(observation.engine, observation.screen, "00_prepare");
        for step in steps.into_iter().rev() {
            self.pending.push_front(QueuedStep::new(step, StepRole::Preparation));
        }
    }

    /// Шаг не удался. Планируемый и подготовительный шаги оставляют запись и обрывают прогон (после возврата к дому);
    /// неудавшийся шаг возврата только обрывает возврат: исходный провал уже назван, а состояние экрана
    /// покажет итог прогона.
    fn step_failed(
        &mut self,
        observation: &ZoomObservation<'_>,
        role: StepRole,
        record: Option<ZoomStepRecord>,
        name: &str,
        reason: &str,
    ) -> ZoomTick {
        // Пальцы бесшовного жеста не должны остаться на экране после провала: сначала поднимаем их.
        let lift = self.lift_stranded_fingers(observation);
        let mut tick = self.record_failure(observation, role, record, name, reason);
        tick.injections.splice(0..0, lift);
        tick
    }

    fn record_failure(
        &mut self,
        observation: &ZoomObservation<'_>,
        role: StepRole,
        record: Option<ZoomStepRecord>,
        name: &str,
        reason: &str,
    ) -> ZoomTick {
        if role == StepRole::Restoration {
            self.pending.clear();
            return ZoomTick::idle(RunnerState::Running);
        }
        self.records.extend(record);
        self.skipped.extend(
            self.pending
                .drain(..)
                .filter(|queued| queued.role == StepRole::Planned)
                .map(|queued| queued.step.name),
        );
        self.deferred_failure = Some(format!("{name}: {reason}"));
        if self.anchored && observation.engine.streaming {
            let steps = steps_to_home(observation.engine, observation.screen, "restore");
            self.pending.extend(
                steps
                    .into_iter()
                    .map(|step| QueuedStep::new(step, StepRole::Restoration)),
            );
        }
        if self.pending.is_empty() {
            return self.conclude_run();
        }
        ZoomTick::idle(RunnerState::Running)
    }

    /// Очередь пуста: прогон закончен — провалом (названным раньше) или успехом.
    fn conclude_run(&mut self) -> ZoomTick {
        let outcome = match self.deferred_failure.take() {
            Some(reason) => RunnerState::Failed(reason),
            None => RunnerState::AllDone,
        };
        self.outcome = Some(outcome.clone());
        ZoomTick::idle(outcome)
    }

    // ---- ход шага ----------------------------------------------------------------------------------------------

    fn advance(&mut self, mut active: ActiveStep, observation: &ZoomObservation<'_>) -> ZoomTick {
        let mut injections = Vec::new();
        if let Some(reason) = self.absorb_events(&mut active, observation) {
            return self.fail_active(active, observation, &reason);
        }
        self.absorb_frames(&mut active, observation);
        self.continue_drag(&mut active, observation, &mut injections);
        if let Some(reason) = self.drive_seamless(&mut active, observation, &mut injections) {
            return self.fail_active(active, observation, &reason);
        }
        if active.settle_until.is_none() && self.is_complete(&active, observation) {
            active.settle_until = Some(observation.now + Duration::from_millis(self.config.step_settle_ms));
        }
        if let Some(until) = active.settle_until
            && observation.now >= until
            && !self.screen_still_moving(&active, observation)
        {
            return self.finish_step(active, observation);
        }
        let timeout = Duration::from_millis(self.config.step_timeout_ms);
        if active.settle_until.is_none()
            && generic_timeout_applies(&active)
            && observation.now.saturating_duration_since(active.started_at) >= timeout
        {
            let reason = self.timeout_reason(&active);
            return self.fail_active(active, observation, &reason);
        }
        self.active = Some(active);
        ZoomTick {
            injections,
            state: RunnerState::Running,
        }
    }

    /// События движка за тик. `Some(причина)` — шаг провален.
    fn absorb_events(&self, active: &mut ActiveStep, observation: &ZoomObservation<'_>) -> Option<String> {
        for event in observation.events {
            match event {
                CameraEvent::LensSwitched {
                    from,
                    to,
                    elapsed,
                    method,
                } => {
                    if !active.expectation.accepts(*to) {
                        return Some(format!(
                            "движок сменил объектив {}→{}, а ждали {}",
                            role_name(*from),
                            role_name(*to),
                            expectation_name(active.expectation)
                        ));
                    }
                    active.switch_events += 1;
                    active.last_switched_camera = camera_of_role(&observation.engine.catalog, *to);
                    if active.switch.is_none() {
                        active.ratio_at_switch_milli = Some(observation.screen.zoom.ratio_milli);
                        active.switch = Some(SwitchTiming {
                            from: role_name(*from),
                            to: role_name(*to),
                            engine_elapsed_ms: u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX),
                            observed_ms: u64::try_from(
                                observation.now.saturating_duration_since(active.started_at).as_millis(),
                            )
                            .unwrap_or(u64::MAX),
                            method: *method,
                        });
                        active.new_camera = camera_of_role(&observation.engine.catalog, *to);
                    }
                }
                CameraEvent::LensSwitchRetargeted { .. } => active.retargets += 1,
                CameraEvent::LensSwitchFailed { from, to, reason } => {
                    return Some(format!(
                        "смена объектива {}→{} не удалась: {reason:?}",
                        role_name(*from),
                        role_name(*to)
                    ));
                }
                CameraEvent::StreamDegraded {
                    requested, fallback, ..
                } => active.notes.push(format!(
                    "частота откатилась {}→{} к/с",
                    requested.frame_rate.frames_per_second(),
                    fallback.frame_rate.frames_per_second()
                )),
                CameraEvent::StreamLimitedByCamera {
                    camera,
                    requested,
                    effective,
                } => active.notes.push(format!(
                    "камера {camera} держит {} из {} к/с",
                    effective.frame_rate.frames_per_second(),
                    requested.frame_rate.frames_per_second()
                )),
                CameraEvent::CommandHandled { .. }
                | CameraEvent::StateChanged { .. }
                | CameraEvent::Unavailable { .. }
                | CameraEvent::Recovered { .. }
                | CameraEvent::StreamConfigured { .. }
                | CameraEvent::StreamConfigurationFailed { .. }
                | CameraEvent::FocusSettled { .. }
                | CameraEvent::ControlsAdjusted { .. }
                | CameraEvent::BracketCaptured { .. }
                | CameraEvent::FlashStillFinished { .. }
                | CameraEvent::WhiteChanged { .. }
                | CameraEvent::WhiteRequestFinished { .. } => {}
            }
        }
        None
    }

    fn absorb_frames(&self, active: &mut ActiveStep, observation: &ZoomObservation<'_>) {
        if active.first_live_ms.is_none()
            && let Some(camera) = &active.new_camera
        {
            active.first_live_ms = first_live_frame_ms(observation.frames, camera, active.action_render);
            if active.first_live_ms.is_some()
                && let Some(old) = active.seamless.as_ref().and_then(SeamlessProgress::leaving_camera)
            {
                active.gap = switch_gap(observation.frames, old, camera, active.action_render);
            }
        }
    }

    /// Дошёл ли шаг до конца ожидания (после него идёт наблюдение).
    fn is_complete(&self, active: &ActiveStep, observation: &ZoomObservation<'_>) -> bool {
        if let Some(seamless) = &active.seamless {
            let switched = !active.expectation.expects_switch()
                || (active.switch.is_some() && (active.first_live_ms.is_some() || self.ring.overran()));
            return seamless.is_done() && switched && observation.engine.streaming;
        }
        if let Some(drag) = &active.drag {
            return drag.ended;
        }
        if !active.expectation.expects_switch() {
            return true;
        }
        active.switch.is_some() && (active.first_live_ms.is_some() || self.ring.overran()) && {
            // Движок уже показывает поток, а не очередную перестройку.
            observation.engine.streaming
        }
    }

    /// Значение зума на экране ещё движется, итог шага судить рано:
    /// * плавный переход к упору после тапа по кнопке ленты ещё идёт;
    /// * после протяжки колесо ещё открыто: палец поднят, но шкала катится по инерции и сама ведёт значение, пока не
    ///   остановится и колесо не уйдёт — тап следующего шага в это время был бы перебит инерцией.
    ///
    /// Срок шага ограничивает ожидание: застрявшее движение судится как есть («экран показывает X, а выбран Y»).
    fn screen_still_moving(&self, active: &ActiveStep, observation: &ZoomObservation<'_>) -> bool {
        let within_deadline = observation.now.saturating_duration_since(active.started_at)
            < Duration::from_millis(self.config.step_timeout_ms);
        let zoom = observation.screen.zoom;
        let moving = zoom.gliding || ((active.drag.is_some() || active.seamless.is_some()) && zoom.wheel_open);
        moving && within_deadline
    }

    // ---- конец шага --------------------------------------------------------------------------------------------

    fn finish_step(&mut self, mut active: ActiveStep, observation: &ZoomObservation<'_>) -> ZoomTick {
        let record = self.build_record(&active, observation, None);
        self.stash_fingers(&mut active);
        let failure = self.judge_screen(&active, &record, observation);
        let record = ZoomStepRecord { failure, ..record };
        let Some(reason) = record.failure.clone() else {
            // Шаг плана попадает в отчёт; подготовка и возврат к дому — нет (их провал виден отдельно).
            if active.role == StepRole::Planned {
                self.last_step_name = Some(record.name.clone());
                self.records.push(record);
            }
            return ZoomTick::idle(RunnerState::Running);
        };
        let name = record.name.clone();
        self.step_failed(observation, active.role, Some(record), &name, &reason)
    }

    fn fail_active(&mut self, mut active: ActiveStep, observation: &ZoomObservation<'_>, reason: &str) -> ZoomTick {
        let record = self.build_record(&active, observation, Some(reason.to_owned()));
        self.stash_fingers(&mut active);
        let name = record.name.clone();
        self.step_failed(observation, active.role, Some(record), &name, reason)
    }

    fn build_record(
        &self,
        active: &ActiveStep,
        observation: &ZoomObservation<'_>,
        failure: Option<String>,
    ) -> ZoomStepRecord {
        let counters = observation.counters;
        let base = &active.baseline.counters;
        let facing_after = active.expectation.facing_after(
            observation
                .engine
                .active
                .as_ref()
                .map(|lens| lens.facing)
                .or(active.baseline.active_facing),
        );
        // Зеркало — по камере, на которой шаг закончился (см. `last_switched_camera`).
        let mirror = match (&active.last_switched_camera, facing_after) {
            (Some(camera), Some(facing)) => mirror_check(observation.frames, camera, active.action_render, facing),
            _ => MirrorCheck::NotApplicable,
        };
        let expected_fps = facing_after.map(|facing| match facing {
            LensFacing::Front => self.config.front_frame_rate,
            LensFacing::Back | LensFacing::External => self.config.back_frame_rate,
        });
        ZoomStepRecord {
            name: active.step.name.clone(),
            expected: expectation_name(active.expectation),
            switch: active.switch,
            first_live_frame_ms: active.first_live_ms,
            new_camera: active.new_camera.as_ref().map(ToString::to_string),
            held_frames: counters
                .held_frames_presented
                .saturating_sub(base.held_frames_presented),
            black_frames_while_picture_expected: counters
                .black_frames_while_picture_expected
                .saturating_sub(base.black_frames_while_picture_expected),
            reveal_faults: RevealFaults::between(base, counters),
            continuity: ContinuityDelta::between(
                base,
                counters,
                &active.baseline.isp_counters,
                observation.isp_counters,
            ),
            presented_frames: presented_histogram(observation.frames, active.action_render),
            retargets: active.retargets,
            lens_intents_published: observation
                .screen
                .lens_published
                .saturating_sub(active.baseline.lens_published),
            measured_fps: observation.engine.measured_fps,
            expected_fps,
            mirror,
            haptics: active.drag.is_some().then(|| {
                haptic_window(
                    observation.screen.haptics.total(),
                    active.baseline.haptics_total,
                    active.action_render,
                    observation.render_now,
                )
            }),
            screen_ratio: f64::from(observation.screen.zoom.ratio_milli) / 1000.0,
            ring_overran: self.ring.overran(),
            seamless: active.seamless.as_ref().map(|progress| SeamlessRecord {
                gap: active.gap,
                ratio_at_switch_milli: active.ratio_at_switch_milli,
                ..progress.record(active.switch_events)
            }),
            failure,
            notes: active.notes.clone(),
        }
    }
}

/// Имя ожидания для отчёта.
pub fn expectation_name(expectation: SwitchExpectation) -> String {
    match expectation {
        SwitchExpectation::NoSwitch => "no_switch".to_owned(),
        SwitchExpectation::ToRole(role) => format!("to_{}", role_name(role)),
        SwitchExpectation::ToAnyBack => "to_any_back".to_owned(),
        SwitchExpectation::MaybeSwitchesBack => "maybe_switches_back".to_owned(),
    }
}

/// Действует ли общий срок шага (бесшовные части ведут свои сроки по фазам).
fn generic_timeout_applies(active: &ActiveStep) -> bool {
    active
        .seamless
        .as_ref()
        .is_none_or(SeamlessProgress::generic_timeout_applies)
}

#[path = "zoom_runner_drag.rs"]
mod drag;
#[path = "zoom_runner_judge.rs"]
mod judge;
#[path = "zoom_runner_seamless.rs"]
mod seamless;

use drag::DragProgress;

#[cfg(test)]
#[path = "zoom_runner_tests.rs"]
mod tests;

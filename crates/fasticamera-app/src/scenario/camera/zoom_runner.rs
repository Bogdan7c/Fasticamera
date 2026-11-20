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

use std::sync::Arc;
use std::time::{Duration, Instant};

use camera_contract::ids::{CameraId, LensFacing, LensRole};
use camera_contract::zoom::ZoomCatalog;
use camera_core::{CameraEvent, CameraStatus};
use camera_screen::ScreenAction;
use render_loop::viewfinder::{ContinuityCounters, FrameRecord};
use shell_contract::clock::MonotonicNanos;
use shell_contract::touch::TouchPhase;

use super::zoom_plan::{SwitchExpectation, ZoomAction, ZoomStep, expectation_for_tap, nearest_stop};
use super::zoom_record::{
    MirrorCheck, RingWatch, SwitchTiming, ZoomStepRecord, first_live_frame_ms, haptic_window, mirror_check,
};
use crate::camera_screen_effects::RecentHapticTimes;
use crate::camera_screen_source::{ScreenReport, ZoomView};
use crate::screen_scenario_config::ScreenPoint;
use crate::zoom_scenario_config::ZoomScenarioConfig;

/// Допуск «экран показывает то значение, которое выбрали»: 2 % коэффициента в тысячных.
const SCREEN_RATIO_TOLERANCE_MILLI: u32 = 20;
/// Имя роли для отчёта (совпадает с serde `LensRole`).
fn role_name(role: LensRole) -> &'static str {
    role.name()
}

/// Объектив, который сейчас отдаёт кадры, глазами исполнителя.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveLensView {
    pub role: LensRole,
    pub camera: CameraId,
    pub facing: LensFacing,
}

/// Что исполнителю нужно знать о движке (срез `CameraStatus`: сам статус тяжёлый и тесту незачем его собирать).
#[derive(Debug, Clone)]
pub struct EngineView {
    /// Поток идёт.
    pub streaming: bool,
    pub active: Option<ActiveLensView>,
    pub catalog: Arc<ZoomCatalog>,
    /// Измеренная частота потока текущей сессии по меткам сенсора, к/с.
    pub measured_fps: Option<f64>,
}

impl EngineView {
    pub fn of(status: &CameraStatus) -> Self {
        Self {
            streaming: matches!(status.state, camera_core::CameraState::Streaming { .. }),
            active: status.lens.active.as_ref().map(|active| ActiveLensView {
                role: active.role,
                camera: active.camera.clone(),
                facing: active.facing,
            }),
            catalog: Arc::clone(&status.lens.zoom_catalog),
            measured_fps: status.stream.measured_frames_per_second,
        }
    }
}

/// Что исполнителю нужно знать об экране камеры (срез отчёта).
#[derive(Debug, Clone, Copy)]
pub struct ScreenView {
    pub zoom: ZoomView,
    /// Когда принимались последние вибрации (кольцо исполнителя эффектов).
    pub haptics: RecentHapticTimes,
    /// Сколько намерений объектива экран положил в ящик за всё время.
    pub lens_published: u64,
}

impl ScreenView {
    pub fn of(report: &ScreenReport) -> Self {
        Self {
            zoom: report.state.zoom,
            haptics: report.effects.haptics.recent_queued,
            lens_published: report.effects.lens.published,
        }
    }
}

/// Один тик наблюдения.
pub struct ZoomObservation<'a> {
    /// Часы маршрутизатора.
    pub now: Instant,
    /// Самое свежее время на шкале рендера (кадры видоискателя/UI): им метятся действия и касания.
    pub render_now: MonotonicNanos,
    pub engine: &'a EngineView,
    /// События движка, пришедшие с прошлого тика (исполнитель — единственный, кто их забирает).
    pub events: &'a [CameraEvent],
    /// Кольцо последних показанных кадров видоискателя (старые первыми).
    pub frames: &'a [FrameRecord],
    pub counters: &'a ContinuityCounters,
    pub screen: &'a ScreenView,
}

/// Образец касания в долях экрана (сценарий-обвязка переводит в пиксели поверхности).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TouchInjection {
    pub phase: TouchPhase,
    pub point: ScreenPoint,
    pub time: MonotonicNanos,
}

/// Что вложить в экран.
#[derive(Debug, Clone, PartialEq)]
pub enum Injection {
    Action(ScreenAction),
    Touch(TouchInjection),
}

/// Состояние прогона после тика.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunnerState {
    Running,
    AllDone,
    Failed(String),
}

/// Итог тика.
#[derive(Debug, Clone, PartialEq)]
pub struct ZoomTick {
    pub injections: Vec<Injection>,
    pub state: RunnerState,
}

impl ZoomTick {
    fn idle(state: RunnerState) -> Self {
        Self {
            injections: Vec::new(),
            state,
        }
    }
}

/// Базовые линии на начало шага: приращения считаются от них.
struct Baseline {
    counters: ContinuityCounters,
    haptics_total: u64,
    lens_published: u64,
    active_facing: Option<LensFacing>,
}

/// Протяжка пальцем: сколько образцов отправлено.
struct DragProgress {
    /// Сколько образцов `Moved` уже отправлено.
    moved: u32,
    /// Отправлен ли `Ended`.
    ended: bool,
    /// Когда следующий образец (часы маршрутизатора).
    next_at: Instant,
}

/// Идущий шаг.
struct ActiveStep {
    step: ZoomStep,
    expectation: SwitchExpectation,
    started_at: Instant,
    action_render: MonotonicNanos,
    baseline: Baseline,
    /// Коэффициент, который должен показать экран после тапа (в тысячных).
    target_ratio_milli: Option<u32>,
    switch: Option<SwitchTiming>,
    new_camera: Option<CameraId>,
    first_live_ms: Option<f64>,
    retargets: u32,
    notes: Vec<String>,
    drag: Option<DragProgress>,
    /// Когда наблюдение «после» кончается; `None` — ещё ждём смену/кадр.
    settle_until: Option<Instant>,
}

/// Исполнитель шагов.
pub struct ZoomRunner {
    config: ZoomScenarioConfig,
    pending: std::vec::IntoIter<ZoomStep>,
    active: Option<ActiveStep>,
    records: Vec<ZoomStepRecord>,
    ring: RingWatch,
    finished: bool,
}

impl ZoomRunner {
    pub fn new(config: ZoomScenarioConfig, steps: Vec<ZoomStep>) -> Self {
        Self {
            config,
            pending: steps.into_iter(),
            active: None,
            records: Vec::new(),
            ring: RingWatch::default(),
            finished: false,
        }
    }

    /// Записи законченных шагов (и провалившегося, если был).
    pub fn records(&self) -> &[ZoomStepRecord] {
        &self.records
    }

    /// Имена шагов, до которых дело не дошло (включая идущий).
    pub fn unfinished_names(&self) -> Vec<String> {
        self.active
            .iter()
            .map(|active| active.step.name.clone())
            .chain(self.pending.clone().map(|step| step.name))
            .collect()
    }

    /// Один тик наблюдения.
    pub fn tick(&mut self, observation: &ZoomObservation<'_>) -> ZoomTick {
        self.ring.absorb(observation.frames);
        if self.finished {
            return ZoomTick::idle(RunnerState::AllDone);
        }
        match self.active.take() {
            None => self.begin_next(observation),
            Some(active) => self.advance(active, observation),
        }
    }

    // ---- начало шага -------------------------------------------------------------------------------------------

    fn begin_next(&mut self, observation: &ZoomObservation<'_>) -> ZoomTick {
        let Some(step) = self.pending.next() else {
            self.finished = true;
            return ZoomTick::idle(RunnerState::AllDone);
        };
        let engine = observation.engine;
        if !engine.streaming {
            return self.fail_before_start(step, "движок не отдаёт поток: шаг начать нельзя");
        }
        let current_role = engine.active.as_ref().map(|active| active.role);
        let mut target_ratio_milli = None;
        let (expectation, injections, drag) = match step.action {
            ZoomAction::TapStop { ratio } => {
                let Some(group) = engine.catalog.back.as_ref() else {
                    return self.fail_before_start(step, "в каталоге зума нет задних камер");
                };
                let Some((index, kind)) = nearest_stop(group, ratio) else {
                    return self.fail_before_start(step, &format!("на ленте нет кнопки для {ratio}×"));
                };
                target_ratio_milli = Some((f64::from(group.stops[index].ratio.value()) * 1000.0).round() as u32);
                (
                    expectation_for_tap(kind, current_role),
                    vec![Injection::Action(ScreenAction::ZoomStopTapped {
                        stop: index,
                        at: observation.render_now,
                    })],
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
                    vec![Injection::Action(ScreenAction::FlipCameraRequested {
                        at: observation.render_now,
                    })],
                    None,
                )
            }
            ZoomAction::WheelDrag => {
                let start = TouchInjection {
                    phase: TouchPhase::Began,
                    point: self.config.wheel_drag.start,
                    time: observation.render_now,
                };
                let drag = DragProgress {
                    moved: 0,
                    ended: false,
                    next_at: observation.now + Duration::from_millis(self.config.wheel_drag.sample_interval_ms),
                };
                (SwitchExpectation::NoSwitch, vec![Injection::Touch(start)], Some(drag))
            }
        };
        self.ring.reset_overrun();
        self.active = Some(ActiveStep {
            expectation,
            started_at: observation.now,
            action_render: observation.render_now,
            baseline: Baseline {
                counters: observation.counters.clone(),
                haptics_total: observation.screen.haptics.total(),
                lens_published: observation.screen.lens_published,
                active_facing: engine.active.as_ref().map(|active| active.facing),
            },
            target_ratio_milli,
            switch: None,
            new_camera: None,
            first_live_ms: None,
            retargets: 0,
            notes: Vec::new(),
            drag,
            settle_until: None,
            step,
        });
        ZoomTick {
            injections,
            state: RunnerState::Running,
        }
    }

    fn fail_before_start(&mut self, step: ZoomStep, reason: &str) -> ZoomTick {
        self.records.push(failed_record(&step.name, reason));
        self.finished = true;
        ZoomTick::idle(RunnerState::Failed(format!("{}: {reason}", step.name)))
    }

    // ---- ход шага ----------------------------------------------------------------------------------------------

    fn advance(&mut self, mut active: ActiveStep, observation: &ZoomObservation<'_>) -> ZoomTick {
        let mut injections = Vec::new();
        if let Some(reason) = self.absorb_events(&mut active, observation) {
            return self.fail_active(active, observation, &reason);
        }
        self.absorb_frames(&mut active, observation);
        self.continue_drag(&mut active, observation, &mut injections);
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
        if active.settle_until.is_none() && observation.now.saturating_duration_since(active.started_at) >= timeout {
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
                CameraEvent::LensSwitched { from, to, elapsed } => {
                    if !active.expectation.accepts(*to) {
                        return Some(format!(
                            "движок сменил объектив {}→{}, а ждали {}",
                            role_name(*from),
                            role_name(*to),
                            expectation_name(active.expectation)
                        ));
                    }
                    if active.switch.is_none() {
                        active.switch = Some(SwitchTiming {
                            from: role_name(*from),
                            to: role_name(*to),
                            engine_elapsed_ms: u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX),
                            observed_ms: u64::try_from(
                                observation.now.saturating_duration_since(active.started_at).as_millis(),
                            )
                            .unwrap_or(u64::MAX),
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
                | CameraEvent::BracketCaptured { .. } => {}
            }
        }
        None
    }

    fn absorb_frames(&self, active: &mut ActiveStep, observation: &ZoomObservation<'_>) {
        if active.first_live_ms.is_none()
            && let Some(camera) = &active.new_camera
        {
            active.first_live_ms = first_live_frame_ms(observation.frames, camera, active.action_render);
        }
    }

    fn continue_drag(&self, active: &mut ActiveStep, observation: &ZoomObservation<'_>, out: &mut Vec<Injection>) {
        let plan = self.config.wheel_drag;
        let Some(drag) = active.drag.as_mut() else {
            return;
        };
        let interval = Duration::from_millis(plan.sample_interval_ms);
        while !drag.ended && observation.now >= drag.next_at {
            let step_index = drag.moved + 1;
            // Метки образцов идут ровным шагом от момента действия: скорость пальца не зависит от дрожания тиков.
            let time = MonotonicNanos::from_nanos(
                active.action_render.as_nanos()
                    + i64::from(step_index) * i64::try_from(plan.sample_interval_ms).unwrap_or(i64::MAX) * 1_000_000,
            );
            let point = ScreenPoint::new(plan.start.x + plan.step_dx * f64::from(step_index), plan.start.y);
            if drag.moved < plan.samples {
                drag.moved += 1;
                out.push(Injection::Touch(TouchInjection {
                    phase: TouchPhase::Moved,
                    point,
                    time,
                }));
            } else {
                drag.ended = true;
                out.push(Injection::Touch(TouchInjection {
                    phase: TouchPhase::Ended,
                    point: ScreenPoint::new(plan.start.x + plan.step_dx * f64::from(plan.samples), plan.start.y),
                    time,
                }));
            }
            drag.next_at += interval;
        }
    }

    /// Дошёл ли шаг до конца ожидания (после него идёт наблюдение).
    fn is_complete(&self, active: &ActiveStep, observation: &ZoomObservation<'_>) -> bool {
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
        let moving = zoom.gliding || (active.drag.is_some() && zoom.wheel_open);
        moving && within_deadline
    }

    fn timeout_reason(&self, active: &ActiveStep) -> String {
        match (&active.switch, active.expectation) {
            (None, expectation) => format!(
                "за {} мс движок не сообщил о смене объектива ({})",
                self.config.step_timeout_ms,
                expectation_name(expectation)
            ),
            (Some(_), _) => format!(
                "смена объектива сообщена, но живого кадра новой камеры на экране нет за {} мс",
                self.config.step_timeout_ms
            ),
        }
    }

    // ---- конец шага --------------------------------------------------------------------------------------------

    fn finish_step(&mut self, active: ActiveStep, observation: &ZoomObservation<'_>) -> ZoomTick {
        let record = self.build_record(&active, observation, None);
        let failure = self.judge_screen(&active, &record, observation);
        let record = ZoomStepRecord { failure, ..record };
        let failed = record.failure.clone();
        let name = record.name.clone();
        self.records.push(record);
        if let Some(reason) = failed {
            self.finished = true;
            return ZoomTick::idle(RunnerState::Failed(format!("{name}: {reason}")));
        }
        ZoomTick::idle(RunnerState::Running)
    }

    fn fail_active(&mut self, active: ActiveStep, observation: &ZoomObservation<'_>, reason: &str) -> ZoomTick {
        let record = self.build_record(&active, observation, Some(reason.to_owned()));
        let name = record.name.clone();
        self.records.push(record);
        self.finished = true;
        ZoomTick::idle(RunnerState::Failed(format!("{name}: {reason}")))
    }

    /// Проверки состояния экрана в конце шага (то, что видно пользователю): показанный коэффициент.
    fn judge_screen(
        &self,
        active: &ActiveStep,
        record: &ZoomStepRecord,
        observation: &ZoomObservation<'_>,
    ) -> Option<String> {
        if let Some(target) = active.target_ratio_milli {
            let shown = observation.screen.zoom.ratio_milli;
            if shown.abs_diff(target) > SCREEN_RATIO_TOLERANCE_MILLI {
                return Some(format!("экран показывает {shown} тыс., а выбран упор {target} тыс."));
            }
        }
        let moves_zoom = active.expectation.expects_switch() || active.drag.is_some();
        if moves_zoom && record.lens_intents_published == 0 {
            return Some("экран не положил намерение объектива в ящик движка".to_owned());
        }
        if active.expectation.expects_switch() && active.new_camera.is_none() {
            return Some("смена объектива есть, а камеру новой роли в каталоге зума найти не удалось".to_owned());
        }
        record.failure.clone()
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
        let mirror = match (&active.new_camera, facing_after) {
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
                    &observation.screen.haptics,
                    active.baseline.haptics_total,
                    active.action_render,
                )
            }),
            screen_ratio: f64::from(observation.screen.zoom.ratio_milli) / 1000.0,
            ring_overran: self.ring.overran(),
            failure,
            notes: active.notes.clone(),
        }
    }
}

/// Запись провалившегося до начала шага.
fn failed_record(name: &str, reason: &str) -> ZoomStepRecord {
    ZoomStepRecord {
        name: name.to_owned(),
        expected: "not_started".to_owned(),
        switch: None,
        first_live_frame_ms: None,
        new_camera: None,
        held_frames: 0,
        black_frames_while_picture_expected: 0,
        retargets: 0,
        lens_intents_published: 0,
        measured_fps: None,
        expected_fps: None,
        mirror: MirrorCheck::NotApplicable,
        haptics: None,
        screen_ratio: 0.0,
        ring_overran: false,
        failure: Some(reason.to_owned()),
        notes: Vec::new(),
    }
}

/// Камера роли по каталогу зума.
fn camera_of_role(catalog: &ZoomCatalog, role: LensRole) -> Option<CameraId> {
    [catalog.back.as_ref(), catalog.front.as_ref()]
        .into_iter()
        .flatten()
        .find_map(|group| group.lens_for_role(role))
        .map(|lens| lens.camera.clone())
}

/// Имя ожидания для отчёта.
pub fn expectation_name(expectation: SwitchExpectation) -> String {
    match expectation {
        SwitchExpectation::NoSwitch => "no_switch".to_owned(),
        SwitchExpectation::ToRole(role) => format!("to_{}", role_name(role)),
        SwitchExpectation::ToAnyBack => "to_any_back".to_owned(),
    }
}

#[cfg(test)]
#[path = "zoom_runner_tests.rs"]
mod tests;

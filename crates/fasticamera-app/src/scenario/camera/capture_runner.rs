//! Исполнитель шагов сценария `capture`: автомат над срезами наблюдения (без маршрутизатора и экрана).
//!
//! Нажатие — действие экрана `ShutterTapped { at }` с меткой последнего показанного кадра (так же нажимает палец:
//! служба выбирает кадр «на момент нажатия», будущих меток нет). RAW переключает действие экрана по значку RAW,
//! а результат проверяется по настройке экрана, а не по факту вызова.
//!
//! Ошибка любого шага останавливает прогон: следующие шаги в отчёте — «не дошли».

use std::time::{Duration, Instant};

use camera_screen::ScreenAction;
use camera_screen::settings::RawCapture;
use camera_screen::shutter::ShotRequest;
use camera_screen::tools::ToolId;
use shell_contract::clock::MonotonicNanos;

use super::capture_burst::{BurstBaselines, BurstProgress, BurstRecord, BurstState};
use super::capture_observation::{CaptureObservation, PhaseBook, SubmitCounts, shot_record};
use super::capture_plan::{PlannedStep, StepKind};
use super::capture_report::{ShotOutcome, ShotRecord, StepRecord};
use crate::capture_scenario_config::CaptureScenarioConfig;
use crate::still_phase_log::PhaseEntry;

/// Как идёт прогон.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunnerState {
    Running,
    AllDone,
    Failed(String),
}

/// Итог одного тика: что вложить в экран и как идёт прогон.
#[derive(Debug)]
pub struct RunnerTick {
    pub injections: Vec<ScreenAction>,
    pub state: RunnerState,
}

/// Нажатие одиночного снимка, ждущее итога.
struct SingleShot {
    pressed_at: Instant,
    at: MonotonicNanos,
    /// Номер самого нового запроса журнала на момент нажатия: новее — наш.
    last_request_before: u64,
    submit_before: SubmitCounts,
    actions_rejected_before: u64,
}

enum Stage {
    /// Пауза между шагами (и перед первым).
    Pause {
        until: Instant,
    },
    /// Приводим настройку RAW экрана к нужной шагу.
    EnsureRaw {
        target: RawCapture,
        started: Instant,
        tapped: bool,
    },
    AwaitShot(SingleShot),
    Burst(Box<BurstState>),
    Finished,
}

/// Исполнитель.
pub struct CaptureRunner {
    config: CaptureScenarioConfig,
    steps: Vec<PlannedStep>,
    /// Номер шага, который идёт сейчас.
    index: usize,
    stage: Stage,
    book: PhaseBook,
    steps_done: Vec<StepRecord>,
    shots: Vec<ShotRecord>,
    burst: Option<BurstRecord>,
    failure: Option<String>,
}

impl CaptureRunner {
    pub fn new(config: CaptureScenarioConfig, steps: Vec<PlannedStep>, now: Instant) -> Self {
        let stage = Stage::Pause { until: now };
        Self {
            config,
            steps,
            index: 0,
            stage,
            book: PhaseBook::default(),
            steps_done: Vec::new(),
            shots: Vec::new(),
            burst: None,
            failure: None,
        }
    }

    /// Новые записи журнала итогов службы (звать перед тиком).
    pub fn absorb_phases(&mut self, entries: Vec<PhaseEntry>, lost: u64) {
        self.book.extend(entries, lost);
    }

    fn step(&self) -> Option<PlannedStep> {
        self.steps.get(self.index).copied()
    }

    fn pause_between_steps(&self) -> Duration {
        Duration::from_millis(self.config.pause_between_steps_ms)
    }

    fn fail(&mut self, reason: String) -> RunnerState {
        if let Some(step) = self.step() {
            // Шаг мог уже записать свой итог (серия): тогда причина доносится только до прогона.
            if !self.steps_done.iter().any(|done| done.name == step.name) {
                self.steps_done.push(StepRecord {
                    name: step.name,
                    shots: self.shot_numbers_of(step.name),
                    failure: Some(reason.clone()),
                });
            }
        }
        self.stage = Stage::Finished;
        self.failure = Some(reason.clone());
        RunnerState::Failed(reason)
    }

    fn shot_numbers_of(&self, step: &'static str) -> Vec<u32> {
        self.shots
            .iter()
            .filter(|shot| shot.step == step)
            .map(|shot| shot.number)
            .collect()
    }

    /// Шаг прошёл: записываем его и переходим к паузе перед следующим (или к концу прогона).
    fn complete_step(&mut self, now: Instant) -> RunnerState {
        if let Some(step) = self.step() {
            self.steps_done.push(StepRecord {
                name: step.name,
                shots: self.shot_numbers_of(step.name),
                failure: None,
            });
        }
        self.index += 1;
        if self.index >= self.steps.len() {
            self.stage = Stage::Finished;
            return RunnerState::AllDone;
        }
        self.stage = Stage::Pause {
            until: now + self.pause_between_steps(),
        };
        RunnerState::Running
    }

    /// Один тик.
    pub fn tick(&mut self, obs: &CaptureObservation<'_>) -> RunnerTick {
        if !obs.mode_is_real && !matches!(self.stage, Stage::Finished) {
            let state = self.fail("сервис снимка перестал быть настоящим посреди прогона".to_owned());
            return RunnerTick {
                injections: Vec::new(),
                state,
            };
        }
        let mut injections = Vec::new();
        let state = self.advance(obs, &mut injections);
        RunnerTick { injections, state }
    }

    fn advance(&mut self, obs: &CaptureObservation<'_>, injections: &mut Vec<ScreenAction>) -> RunnerState {
        match &mut self.stage {
            Stage::Finished => match &self.failure {
                Some(reason) => RunnerState::Failed(reason.clone()),
                None => RunnerState::AllDone,
            },
            Stage::Pause { until } => {
                if obs.now < *until {
                    return RunnerState::Running;
                }
                let Some(step) = self.step() else {
                    self.stage = Stage::Finished;
                    return RunnerState::AllDone;
                };
                self.stage = Stage::EnsureRaw {
                    target: step.required_raw(),
                    started: obs.now,
                    tapped: false,
                };
                RunnerState::Running
            }
            Stage::EnsureRaw {
                target,
                started,
                tapped,
            } => {
                if obs.raw_setting == *target {
                    return self.start_step(obs, injections);
                }
                if !*tapped {
                    *tapped = true;
                    *started = obs.now;
                    injections.push(ScreenAction::StripToolTapped(ToolId::Raw));
                    return RunnerState::Running;
                }
                let waited = obs.now.saturating_duration_since(*started);
                if waited >= Duration::from_millis(self.config.raw_toggle_timeout_ms) {
                    let reason = format!(
                        "значок RAW не переключил настройку экрана за {} мс: стоит {:?}, нужно {:?}",
                        waited.as_millis(),
                        obs.raw_setting,
                        target
                    );
                    return self.fail(reason);
                }
                RunnerState::Running
            }
            Stage::AwaitShot(_) => self.await_single(obs),
            Stage::Burst(_) => self.drive_burst(obs, injections),
        }
    }

    /// Настройка RAW верна: начинаем сам шаг.
    fn start_step(&mut self, obs: &CaptureObservation<'_>, injections: &mut Vec<ScreenAction>) -> RunnerState {
        let Some(step) = self.step() else {
            self.stage = Stage::Finished;
            return RunnerState::AllDone;
        };
        let last_request = obs.journal.iter().map(|request| request.request.0).max().unwrap_or(0);
        match step.kind {
            StepKind::Single { .. } => {
                injections.push(ScreenAction::ShutterTapped { at: obs.render_now });
                self.stage = Stage::AwaitShot(SingleShot {
                    pressed_at: obs.now,
                    at: obs.render_now,
                    last_request_before: last_request,
                    submit_before: obs.submit,
                    actions_rejected_before: obs.actions_rejected,
                });
            }
            StepKind::Burst => {
                let baselines = BurstBaselines {
                    submit: obs.submit,
                    actions_rejected: obs.actions_rejected,
                    live: obs.live,
                    last_request,
                };
                self.stage = Stage::Burst(Box::new(BurstState::new(
                    self.config.burst,
                    self.pause_between_steps(),
                    baselines,
                    obs.now,
                )));
            }
        }
        RunnerState::Running
    }

    /// Ждём итог одиночного снимка.
    fn await_single(&mut self, obs: &CaptureObservation<'_>) -> RunnerState {
        let Some(step) = self.step() else {
            return RunnerState::Running;
        };
        let Stage::AwaitShot(shot) = &self.stage else {
            return RunnerState::Running;
        };
        let (pressed_at, at, last_before) = (shot.pressed_at, shot.at, shot.last_request_before);
        let (submit_before, rejected_before) = (shot.submit_before, shot.actions_rejected_before);
        let waited = obs.now.saturating_duration_since(pressed_at);
        let request: Option<&ShotRequest> = obs
            .journal
            .iter()
            .find(|request| request.request.0 > last_before && request.pressed_at == at);
        let Some(request) = request else {
            if waited >= Duration::from_millis(self.config.secured_timeout_ms) {
                let submit = obs.submit.delta_since(submit_before);
                let reason = format!(
                    "нажатие не дошло до сервиса снимка за {} мс (попыток постановки {}, принято {}, отвергнуто сервисом {}, отказов экрана {})",
                    waited.as_millis(),
                    submit.submitted(),
                    submit.accepted,
                    submit.rejected,
                    obs.actions_rejected.saturating_sub(rejected_before)
                );
                return self.fail(reason);
            }
            return RunnerState::Running;
        };
        let number = u32::try_from(self.shots.len()).unwrap_or(u32::MAX).saturating_add(1);
        let record = shot_record(
            number,
            step.name,
            step.required_raw() == RawCapture::On,
            request,
            pressed_at,
            &self.book,
        );
        match &record.outcome {
            ShotOutcome::Saved { .. } => {
                self.shots.push(record);
                self.complete_step(obs.now)
            }
            ShotOutcome::Failed { failure } => {
                let reason = format!("снимок {number} сорвался: {failure}");
                self.shots.push(record);
                self.fail(reason)
            }
            ShotOutcome::Unfinished { secured } => {
                let secured_overdue = !secured && waited >= Duration::from_millis(self.config.secured_timeout_ms);
                let saved_overdue = waited >= Duration::from_millis(self.config.saved_timeout_ms);
                if secured_overdue {
                    let reason = format!("«кадр захвачен» не пришёл за {} мс", waited.as_millis());
                    self.shots.push(record);
                    return self.fail(reason);
                }
                if saved_overdue {
                    let reason = format!("«сохранено» не пришло за {} мс", waited.as_millis());
                    self.shots.push(record);
                    return self.fail(reason);
                }
                RunnerState::Running
            }
        }
    }

    /// Ведём серию.
    fn drive_burst(&mut self, obs: &CaptureObservation<'_>, injections: &mut Vec<ScreenAction>) -> RunnerState {
        let Stage::Burst(state) = &mut self.stage else {
            return RunnerState::Running;
        };
        let (actions, progress) = state.tick(obs, &self.book);
        injections.extend(actions);
        if progress == BurstProgress::Running {
            return RunnerState::Running;
        }
        // Автомат серии отдаём целиком: итог собирает он.
        let Stage::Burst(state) = std::mem::replace(&mut self.stage, Stage::Finished) else {
            return RunnerState::Running;
        };
        let name = self.step().map_or("burst", |step| step.name);
        let first_number = u32::try_from(self.shots.len()).unwrap_or(u32::MAX).saturating_add(1);
        let (record, shots) = state.finish(obs, &self.book, name, first_number);
        self.shots.extend(shots);
        self.burst = Some(record);
        if progress == BurstProgress::DrainTimedOut {
            return self.fail(format!(
                "итоги снимков серии не пришли за {} мс после последнего нажатия",
                self.config.burst.drain_timeout_ms
            ));
        }
        self.complete_step(obs.now)
    }

    /// Записи прогона и шаги, до которых дело не дошло.
    pub fn into_records(self) -> RunnerRecords {
        let reached = self.steps_done.len();
        let unfinished = self.steps.iter().skip(reached).map(|step| step.name).collect();
        RunnerRecords {
            steps: self.steps_done,
            shots: self.shots,
            burst: self.burst,
            unfinished,
            failure: self.failure,
            phase_entries_lost: self.book.lost(),
        }
    }
}

/// Что исполнитель собрал за прогон.
pub struct RunnerRecords {
    pub steps: Vec<StepRecord>,
    pub shots: Vec<ShotRecord>,
    pub burst: Option<BurstRecord>,
    pub unfinished: Vec<&'static str>,
    pub failure: Option<String>,
    pub phase_entries_lost: u64,
}

#[cfg(test)]
#[path = "capture_runner_tests.rs"]
mod tests;

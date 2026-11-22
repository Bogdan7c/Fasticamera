//! Стенд тестов сценария `capture`: «мир» из экрана и службы записи, которым управляет исполнитель шагов.
//!
//! Мир отвечает на действия так же, как настоящие: нажатие затвора рождает запрос в журнале и через заданное время —
//! фазы службы; нажатие значка RAW переключает настройку; затвор, занятый до «кадр захвачен», отказывает.
//! Время — ручное (`Instant` от базы + прошедшее), поэтому времена фаз в тестах точные.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение — способ сообщить о провале"
)]

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use camera_screen::ScreenAction;
use camera_screen::settings::RawCapture;
use camera_screen::shutter::{PrimaryFormat, ShotRequest, ShotRequestId};
use shell_contract::clock::MonotonicNanos;
use still_contract::plan::StillId;

use super::capture_observation::{CaptureObservation, LiveCounters, SubmitCounts};
use super::capture_runner::{CaptureRunner, RunnerState};
use crate::capture_scenario_config::CaptureScenarioConfig;
use crate::still_phase_log::{PhaseEntry, PhaseKind};
use crate::still_test_support::shot;

/// Как мир отвечает на нажатия.
#[derive(Clone)]
pub struct Behaviour {
    /// Через сколько после нажатия служба говорит «кадр захвачен» (затвор до этого занят).
    pub secured_after: Duration,
    /// Через сколько после нажатия служба говорит «сохранено» или «сорвано».
    pub terminal_after: Duration,
    /// Переключает ли значок RAW настройку.
    pub raw_toggle_works: bool,
    /// Номера запросов экрана, которые служба срывает (`Failed`).
    pub failing_requests: BTreeSet<u64>,
    /// Номера нажатий (с 1), которые сервис отвергает как занятые (до журнала не доходят).
    pub service_rejects_press: BTreeSet<u64>,
    /// Служба вообще не отвечает фазами.
    pub silent_service: bool,
    /// Служба говорит только «кадр захвачен», итога нет.
    pub never_saves: bool,
    /// Запросы с номером больше этого получают только «кадр захвачен», итога нет.
    pub no_result_after_request: Option<u64>,
}

impl Default for Behaviour {
    fn default() -> Self {
        Self {
            secured_after: Duration::from_millis(30),
            terminal_after: Duration::from_millis(400),
            raw_toggle_works: true,
            failing_requests: BTreeSet::new(),
            service_rejects_press: BTreeSet::new(),
            silent_service: false,
            never_saves: false,
            no_result_after_request: None,
        }
    }
}

struct PendingPhase {
    due: Duration,
    request: u64,
    still: u64,
    kind: PhaseKind,
}

/// Мир: экран + служба записи.
pub struct World {
    pub base: Instant,
    pub elapsed: Duration,
    pub behaviour: Behaviour,
    pub journal: Vec<ShotRequest>,
    pub raw: RawCapture,
    pub submit: SubmitCounts,
    pub actions_rejected: u64,
    pub live: LiveCounters,
    pub mode_is_real: bool,
    /// Сколько нажатий затвора получил мир (с 1).
    pub presses: u64,
    /// Сколько раз нажат значок RAW.
    pub raw_taps: u64,
    secured_until: Duration,
    next_request: u64,
    pending: Vec<PendingPhase>,
}

impl World {
    pub fn new(behaviour: Behaviour) -> Self {
        Self {
            base: Instant::now(),
            elapsed: Duration::ZERO,
            behaviour,
            journal: Vec::new(),
            raw: RawCapture::Off,
            submit: SubmitCounts::default(),
            actions_rejected: 0,
            live: LiveCounters::default(),
            mode_is_real: true,
            presses: 0,
            raw_taps: 0,
            secured_until: Duration::ZERO,
            next_request: 1,
            pending: Vec::new(),
        }
    }

    pub fn now(&self) -> Instant {
        self.base + self.elapsed
    }

    /// Метка кадра рендера на текущий момент (одна шкала со временем мира; на тик — новая).
    pub fn render_now(&self) -> MonotonicNanos {
        MonotonicNanos::from_nanos(1_000_000_000 + i64::try_from(self.elapsed.as_nanos()).unwrap())
    }

    fn apply(&mut self, action: ScreenAction) {
        match action {
            ScreenAction::StripToolTapped(_) => {
                self.raw_taps += 1;
                if self.behaviour.raw_toggle_works {
                    self.raw = self.raw.toggled();
                }
            }
            ScreenAction::ShutterTapped { at } => self.press(at),
            other => panic!("сценарий не должен вкладывать {other:?}"),
        }
    }

    fn press(&mut self, at: MonotonicNanos) {
        self.presses += 1;
        // Затвор занят до «кадр захвачен» предыдущего снимка: экран отказывает сам и считает отказ.
        if self.elapsed < self.secured_until {
            self.actions_rejected += 1;
            return;
        }
        if self.behaviour.service_rejects_press.contains(&self.presses) {
            self.submit.rejected += 1;
            return;
        }
        let request_id = self.next_request;
        self.next_request += 1;
        self.submit.accepted += 1;
        self.journal.push(ShotRequest {
            pressed_at: at,
            raw: self.raw,
            primary_format: PrimaryFormat::Heif,
            ..shot(request_id)
        });
        self.secured_until = self.elapsed + self.behaviour.secured_after;
        if self.behaviour.silent_service {
            return;
        }
        let still = request_id + 100;
        self.pending.push(PendingPhase {
            due: self.elapsed + self.behaviour.secured_after,
            request: request_id,
            still,
            kind: PhaseKind::Secured,
        });
        let no_result = self.behaviour.never_saves
            || self
                .behaviour
                .no_result_after_request
                .is_some_and(|last_with_result| request_id > last_with_result);
        if no_result {
            return;
        }
        let kind = if self.behaviour.failing_requests.contains(&request_id) {
            PhaseKind::Failed {
                failure: "запись: MediaStore отказал".to_owned(),
            }
        } else {
            PhaseKind::Saved {
                primary: format!("content://media/external_primary/images/media/{request_id}"),
                raw: (self.raw == RawCapture::On)
                    .then(|| format!("content://media/external_primary/file/{request_id}")),
            }
        };
        self.pending.push(PendingPhase {
            due: self.elapsed + self.behaviour.terminal_after,
            request: request_id,
            still,
            kind,
        });
    }

    /// Фазы, срок которых настал, как записи журнала.
    fn due_entries(&mut self, next_seq: &mut u64) -> Vec<PhaseEntry> {
        let (due, later): (Vec<_>, Vec<_>) = std::mem::take(&mut self.pending)
            .into_iter()
            .partition(|phase| phase.due <= self.elapsed);
        self.pending = later;
        due.into_iter()
            .map(|phase| {
                *next_seq += 1;
                PhaseEntry {
                    seq: *next_seq,
                    still: StillId(phase.still),
                    request: ShotRequestId(phase.request),
                    at: self.base + phase.due,
                    kind: phase.kind,
                }
            })
            .collect()
    }
}

/// Прогоняет исполнитель по миру тиками по `tick` до конца или до `limit` времени.
pub fn drive(runner: &mut CaptureRunner, world: &mut World, tick: Duration, limit: Duration) -> RunnerState {
    let mut seq = 0;
    let mut state = RunnerState::Running;
    while world.elapsed <= limit {
        let entries = world.due_entries(&mut seq);
        runner.absorb_phases(entries, 0);
        let journal = world.journal.clone();
        let observation = CaptureObservation {
            now: world.now(),
            render_now: world.render_now(),
            raw_setting: world.raw,
            actions_rejected: world.actions_rejected,
            submit: world.submit,
            journal: &journal,
            live: world.live,
            mode_is_real: world.mode_is_real,
        };
        let outcome = runner.tick(&observation);
        for action in outcome.injections {
            world.apply(action);
        }
        state = outcome.state;
        if state != RunnerState::Running {
            return state;
        }
        world.elapsed += tick;
    }
    state
}

/// Конфиг с короткими сроками для тестов в ручном времени.
pub fn fast_config() -> CaptureScenarioConfig {
    CaptureScenarioConfig {
        pause_between_steps_ms: 100,
        secured_timeout_ms: 500,
        saved_timeout_ms: 2_000,
        raw_toggle_timeout_ms: 300,
        ..CaptureScenarioConfig::default()
    }
}

/// Отчёт по записям исполнителя: настоящий режим в начале и конце, рендер принял столько команд, сколько снимков.
pub fn report_of(records: super::capture_runner::RunnerRecords) -> super::capture_report::CaptureObservationReport {
    use super::capture_report::{CaptureObservationReport, RenderStillDelta};
    use crate::capture_backend::CaptureMode;
    let accepted = u64::try_from(records.shots.len()).unwrap();
    CaptureObservationReport {
        planned_steps: 3,
        steps: records.steps,
        shots: records.shots,
        burst: records.burst,
        unfinished: records.unfinished,
        run_failure: records.failure,
        mode_at_start: CaptureMode::Real,
        mode_at_end: CaptureMode::Real,
        phase_entries_lost: records.phase_entries_lost,
        render_still: Some(RenderStillDelta {
            begins_accepted: accepted,
            targets_accepted: accepted,
            ..RenderStillDelta::default()
        }),
        render_still_detail: None,
        full_frame: Default::default(),
    }
}

/// Отчёт здорового прогона.
pub fn healthy_report() -> super::capture_report::CaptureObservationReport {
    let mut world = World::new(Behaviour::default());
    let mut runner = CaptureRunner::new(fast_config(), super::capture_plan::planned_steps(), world.now());
    let state = drive(
        &mut runner,
        &mut world,
        Duration::from_millis(10),
        Duration::from_secs(60),
    );
    assert_eq!(state, RunnerState::AllDone);
    report_of(runner.into_records())
}

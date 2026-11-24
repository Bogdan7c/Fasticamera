//! Раннер на подставных работах: порядок, ошибка одной работы, отмена, пауза (без `sleep`), тепло, события.

use std::cell::RefCell;
use std::collections::HashMap;

use super::*;
use crate::saturation_bench::catalog::find_entry;
use crate::saturation_bench::report::{EndReasonName, FailureKind};

/// Часы без реального времени: пауза только записывается и двигает «время» вперёд.
#[derive(Default)]
struct FakeClock {
    now: RefCell<Duration>,
    pauses: RefCell<Vec<Duration>>,
    /// Отменить токен во время паузы с этим номером (с нуля).
    cancel_in_pause: Option<(usize, CancelToken)>,
}

impl RunClock for FakeClock {
    fn elapsed(&self) -> Duration {
        *self.now.borrow()
    }

    fn pause(&self, duration: Duration, cancel: &CancelToken) -> PauseOutcome {
        let index = self.pauses.borrow().len();
        self.pauses.borrow_mut().push(duration);
        if let Some((target, token)) = &self.cancel_in_pause
            && *target == index
        {
            token.cancel();
        }
        if cancel.is_cancelled() {
            return PauseOutcome::Cancelled;
        }
        *self.now.borrow_mut() += duration;
        PauseOutcome::Elapsed
    }
}

/// Тепло, которое растёт с каждым чтением: по нему видно, что снимки «в начале» и «в конце» разные.
#[derive(Default)]
struct RisingThermal(RefCell<u32>);

impl ThermalProbe for RisingThermal {
    fn snapshot(&self) -> ThermalSnapshot {
        let mut reads = self.0.borrow_mut();
        *reads += 1;
        ThermalSnapshot {
            status: None,
            headroom: Some(*reads as f32),
            error: None,
        }
    }
}

enum Script {
    Frames(f64),
    NoFrames,
    Fail(FailureKind),
    /// Работа отменяется токеном посреди прогона и возвращает частичный итог.
    CancelsRun,
}

struct ScriptedExecutor {
    scripts: HashMap<&'static str, Script>,
    started: Vec<&'static str>,
}

fn measurement(fps: Option<f64>, end_reason: EndReasonName) -> Measurement {
    Measurement {
        end_reason,
        warmup_frames: 5,
        measured_frames: fps.map_or(0, |value| value as u64),
        drained_frames: 0,
        window_ms: 1_000.0,
        frames_per_second: fps,
        frame_cost_ms: fps.map(|value| 1_000.0 / value),
        passes: Vec::new(),
        unmeasured_pass_frames: 0,
        mismatched_pass_frames: 0,
        timeline_fps: Vec::new(),
        cost: None,
    }
}

impl WorkloadExecutor for ScriptedExecutor {
    fn execute(&mut self, entry: &CatalogEntry, cancel: &CancelToken) -> Result<ExecutedWorkload, WorkloadFailure> {
        self.started.push(entry.name);
        let done = |measurement| {
            Ok(ExecutedWorkload {
                measurement,
                scene: None,
                ui: None,
                digest: DigestOutcome::Unavailable {
                    reason: "подставной исполнитель".to_owned(),
                },
            })
        };
        match self.scripts.get(entry.name) {
            Some(Script::Frames(fps)) => done(measurement(Some(*fps), EndReasonName::Completed)),
            Some(Script::NoFrames) => done(measurement(None, EndReasonName::Completed)),
            Some(Script::Fail(kind)) => Err(WorkloadFailure::new(*kind, "причина из подставной работы")),
            Some(Script::CancelsRun) => {
                cancel.cancel();
                done(measurement(Some(7.0), EndReasonName::Cancelled))
            }
            None => panic!("в сценарии теста нет работы {}", entry.name),
        }
    }
}

fn entries(names: &[&str]) -> Vec<&'static CatalogEntry> {
    names
        .iter()
        .map(|name| find_entry(name).expect("имя каталога"))
        .collect()
}

struct Outcome {
    summary: RunSummary,
    started: Vec<&'static str>,
    events: Vec<RunEvent>,
    pauses: Vec<Duration>,
}

fn run(
    scripts: Vec<(&'static str, Script)>,
    names: &[&str],
    cooldown: Duration,
    cancel_in_pause: Option<usize>,
) -> Outcome {
    let cancel = CancelToken::new();
    let clock = FakeClock {
        cancel_in_pause: cancel_in_pause.map(|index| (index, cancel.clone())),
        ..FakeClock::default()
    };
    let mut executor = ScriptedExecutor {
        scripts: scripts.into_iter().collect(),
        started: Vec::new(),
    };
    let thermal = RisingThermal::default();
    let mut events: Vec<RunEvent> = Vec::new();
    let mut observer = |event: RunEvent| events.push(event);
    let summary = BenchRunner {
        executor: &mut executor,
        clock: &clock,
        thermal: &thermal,
        observer: &mut observer,
        cooldown_between: cooldown,
    }
    .run(&entries(names), &cancel);
    Outcome {
        summary,
        started: executor.started,
        events,
        pauses: clock.pauses.into_inner(),
    }
}

#[test]
fn works_run_in_list_order_and_every_row_carries_its_number_and_thermal_pair() {
    let outcome = run(
        vec![("viewfinder", Script::Frames(200.0)), ("still", Script::Frames(25.0))],
        &["viewfinder", "still"],
        Duration::ZERO,
        None,
    );
    assert_eq!(outcome.started, ["viewfinder", "still"]);
    assert!(!outcome.summary.cancelled);
    let rows = &outcome.summary.rows;
    assert_eq!((rows[0].index, rows[0].total, rows[1].index), (1, 2, 2));
    assert_eq!(rows[0].measurement().unwrap().frames_per_second, Some(200.0));
    assert_eq!(rows[1].measurement().unwrap().frame_cost_ms, Some(40.0));
    // Тепло снимается в начале и в конце каждой работы: четыре разных чтения.
    let headrooms: Vec<f32> = rows
        .iter()
        .flat_map(|row| [row.thermal_start.as_ref().unwrap(), row.thermal_end.as_ref().unwrap()])
        .map(|snapshot| snapshot.headroom.unwrap())
        .collect();
    assert_eq!(headrooms, [1.0, 2.0, 3.0, 4.0]);
}

#[test]
fn a_failing_work_does_not_stop_the_others_and_is_visible_in_the_table() {
    let outcome = run(
        vec![
            ("viewfinder", Script::Frames(300.0)),
            ("headroom_v4", Script::Fail(FailureKind::BuildFailed)),
            ("still", Script::NoFrames),
            ("compositor", Script::Frames(900.0)),
        ],
        &["viewfinder", "headroom_v4", "still", "compositor"],
        Duration::ZERO,
        None,
    );
    assert_eq!(
        outcome.started,
        ["viewfinder", "headroom_v4", "still", "compositor"],
        "остальные не сорваны"
    );
    let words: Vec<&str> = outcome.summary.rows.iter().map(WorkloadRow::verdict_word).collect();
    assert_eq!(words, ["ok", "build_failed", "no_frames", "ok"]);
    assert!(matches!(
        &outcome.summary.rows[1].outcome,
        RowOutcome::Failed { failure } if failure.kind == FailureKind::BuildFailed
    ));
    assert!(
        outcome.summary.rows[1].thermal_start.is_some(),
        "тепло снято и у провалившейся"
    );
}

#[test]
fn cancel_in_the_middle_keeps_the_partial_row_and_marks_the_rest_not_run() {
    let outcome = run(
        vec![
            ("viewfinder", Script::Frames(300.0)),
            ("compositor", Script::CancelsRun),
            ("still", Script::Frames(10.0)),
        ],
        &["viewfinder", "compositor", "still", "headroom_v4"],
        Duration::from_secs(3),
        None,
    );
    assert_eq!(
        outcome.started,
        ["viewfinder", "compositor"],
        "после отмены новые работы не стартуют"
    );
    assert!(outcome.summary.cancelled);
    let rows = &outcome.summary.rows;
    assert_eq!(rows.len(), 4, "частичный итог отдан целиком");
    let partial = rows[1].measurement().expect("прерванная работа отдала частичный итог");
    assert_eq!(partial.end_reason, EndReasonName::Cancelled);
    assert_eq!(partial.frames_per_second, Some(7.0));
    for skipped in &rows[2..] {
        assert_eq!(
            skipped.outcome,
            RowOutcome::NotRun {
                reason: NotRunReason::Cancelled
            }
        );
        assert!(skipped.thermal_start.is_none() && skipped.started_at_ms.is_none());
    }
    assert_eq!(
        outcome.pauses,
        [Duration::from_secs(3)],
        "пауза была только до второй работы"
    );
}

#[test]
fn cooldown_is_kept_between_works_but_not_before_the_first_or_after_the_last() {
    let outcome = run(
        vec![
            ("viewfinder", Script::Frames(1.0)),
            ("compositor", Script::Frames(1.0)),
            ("still", Script::Frames(1.0)),
        ],
        &["viewfinder", "compositor", "still"],
        Duration::from_secs(3),
        None,
    );
    assert_eq!(outcome.pauses, [Duration::from_secs(3), Duration::from_secs(3)]);
    // Начала работ сдвинуты на паузы (часы подставные: 0, 3, 6 с).
    let starts: Vec<f64> = outcome
        .summary
        .rows
        .iter()
        .map(|row| row.started_at_ms.unwrap())
        .collect();
    assert_eq!(starts, [0.0, 3_000.0, 6_000.0]);

    let no_cooldown = run(
        vec![("viewfinder", Script::Frames(1.0)), ("still", Script::Frames(1.0))],
        &["viewfinder", "still"],
        Duration::ZERO,
        None,
    );
    assert!(no_cooldown.pauses.is_empty(), "нулевая пауза не вызывает часы");
}

#[test]
fn cancel_during_the_pause_skips_the_next_work() {
    let outcome = run(
        vec![("viewfinder", Script::Frames(1.0)), ("still", Script::Frames(1.0))],
        &["viewfinder", "still"],
        Duration::from_secs(3),
        Some(0),
    );
    assert_eq!(outcome.started, ["viewfinder"]);
    assert!(outcome.summary.cancelled);
    assert_eq!(outcome.summary.rows[1].verdict_word(), "not_run");
}

#[test]
fn start_and_finish_events_come_per_work_with_the_verdict_word() {
    let outcome = run(
        vec![
            ("viewfinder", Script::Frames(5.0)),
            ("still", Script::Fail(FailureKind::ObjectsNotReleased)),
        ],
        &["viewfinder", "still"],
        Duration::ZERO,
        None,
    );
    let shape: Vec<String> = outcome
        .events
        .iter()
        .map(|event| match event {
            RunEvent::WorkloadStarted { index, total, name, .. } => format!("start {index}/{total} {name}"),
            RunEvent::WorkloadFinished {
                index,
                name,
                verdict_word,
                ..
            } => format!("end {index} {name} {verdict_word}"),
        })
        .collect();
    assert_eq!(
        shape,
        [
            "start 1/2 viewfinder",
            "end 1 viewfinder ok",
            "start 2/2 still",
            "end 2 still leaked"
        ]
    );
    let RunEvent::WorkloadStarted { thermal, .. } = &outcome.events[0] else {
        panic!("первым идёт начало работы");
    };
    assert_eq!(
        thermal.headroom,
        Some(1.0),
        "в событии начала лежит тепловой снимок начала"
    );
}

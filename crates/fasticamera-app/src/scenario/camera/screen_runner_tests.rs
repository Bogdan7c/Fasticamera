//! Правила исполнителя плана `screen` без маршрутизатора: срок, выдержка, перерост, сквозные отказы.

use camera_contract::geometry::SensorOrientation;
use camera_contract::three_a::{AeAfLock, PointOfInterest};

use super::*;
use crate::camera_screen_source::{
    ModeKind, PanelKind, Presence, ScreenStateSnapshot, ShutterActivity, ShutterPlacement,
};
use crate::scenario::camera::screen_expect::{Count, Metric, StateCheck};
use crate::scenario::camera::screen_gesture::Gesture;

const TIMING: StepTiming = StepTiming {
    timeout: Duration::from_millis(1_000),
    settle: Duration::from_millis(100),
};

fn probe() -> ScreenProbe {
    ScreenProbe {
        taps: 0,
        long_presses: 0,
        pans: 0,
        button_presses: 0,
        gestures_cancelled: 0,
        camera_intents_sent: 0,
        camera_intents_lost: 0,
        focus_triggers_sent: 0,
        focus_trigger_failures: 0,
        results_with_unknown_origin: 0,
        capture_requests: 0,
        presence_live: true,
        lock: AeAfLock::Unlocked,
        point_of_interest: PointOfInterest::WholeFrame,
        sensor_orientation: Some(SensorOrientation::Degrees90),
        exposure_bias_ev: 0.0,
        thumbnail_has_last: false,
        screen: ScreenStateSnapshot {
            panel: PanelKind::Closed,
            lock_plaque: Presence::Hidden,
            focus_indicator: Presence::Hidden,
            exposure_steps: 0,
            mode: ModeKind::Photo,
            shutter_placement: ShutterPlacement::Visible,
            shutter_activity: ShutterActivity::Idle,
        },
        frames_produced: 100,
    }
}

fn tap_step(name: &'static str) -> PlannedStep {
    PlannedStep {
        name,
        purpose: "тап доходит до HAL",
        gesture: Gesture::Tap { x: 0.5, y: 0.5 },
        expect: Expectation::new()
            .count(Metric::Taps, Count::Exactly(1))
            .count(Metric::CameraIntents, Count::Exactly(1))
            .count(Metric::FocusTriggers, Count::Exactly(1)),
        inferred: None,
    }
}

/// Что видно после успешного тапа, считая от `before`.
fn after_tap(before: ScreenProbe) -> ScreenProbe {
    ScreenProbe {
        taps: before.taps + 1,
        camera_intents_sent: before.camera_intents_sent + 1,
        focus_triggers_sent: before.focus_triggers_sent + 1,
        ..before
    }
}

fn begin(runner: &mut StepRunner, now: Instant, seen: &ScreenProbe) -> &'static str {
    match runner.tick(now, seen) {
        RunnerTick::BeginStep(step) => step.name,
        other => panic!("ждали начало шага, получили {other:?}"),
    }
}

#[test]
fn a_plan_runs_each_step_once_and_finishes_with_ok_records() {
    let start = Instant::now();
    let mut seen = probe();
    let mut runner = StepRunner::new(vec![tap_step("first"), tap_step("second")], TIMING, seen);

    assert_eq!(begin(&mut runner, start, &seen), "first");
    // Пока ничего не случилось, стадия второй раз не выдаётся.
    assert_eq!(
        runner.tick(start + Duration::from_millis(50), &seen),
        RunnerTick::Waiting
    );
    seen = after_tap(seen);
    assert_eq!(
        runner.tick(start + Duration::from_millis(60), &seen),
        RunnerTick::Waiting
    );
    // Выдержка ещё не прошла.
    assert_eq!(
        runner.tick(start + Duration::from_millis(100), &seen),
        RunnerTick::Waiting
    );
    // Выдержка прошла: второй шаг начинается ровно один раз, с нового базового снимка.
    assert_eq!(begin(&mut runner, start + Duration::from_millis(200), &seen), "second");
    assert_eq!(
        runner.tick(start + Duration::from_millis(210), &seen),
        RunnerTick::Waiting
    );
    seen = after_tap(seen);
    runner.tick(start + Duration::from_millis(220), &seen);
    assert_eq!(
        runner.tick(start + Duration::from_millis(400), &seen),
        RunnerTick::AllDone
    );

    let names: Vec<_> = runner.records().iter().map(|record| (record.name, record.ok)).collect();
    assert_eq!(names, [("first", true), ("second", true)]);
    assert!(runner.records()[0].observed.contains("тапы 1") && runner.records()[0].observed.contains("триггеры 1"));
}

#[test]
fn a_tap_that_never_reached_the_hal_fails_at_the_deadline_with_the_reason() {
    let start = Instant::now();
    let mut seen = probe();
    let mut runner = StepRunner::new(vec![tap_step("lost_tap")], TIMING, seen);
    begin(&mut runner, start, &seen);
    // Экран распознал тап и отправил намерение, движок триггер не отправил.
    seen.taps = 1;
    seen.camera_intents_sent = 1;
    assert_eq!(
        runner.tick(start + Duration::from_millis(500), &seen),
        RunnerTick::Waiting
    );
    let RunnerTick::Failed(reason) = runner.tick(start + Duration::from_millis(1_000), &seen) else {
        panic!("шаг обязан провалиться по сроку");
    };
    assert!(reason.contains("lost_tap"), "{reason}");
    assert!(
        reason.contains("триггеры до HAL: ожидали ровно 1, увидели 0"),
        "{reason}"
    );
    assert!(reason.contains("триггер до HAL не дошёл"), "{reason}");
    assert!(!runner.records()[0].ok);
    // После провала автомат молчит.
    assert_eq!(
        runner.tick(start + Duration::from_millis(2_000), &seen),
        RunnerTick::Waiting
    );
}

#[test]
fn an_extra_camera_intent_fails_at_once_without_waiting_for_the_deadline() {
    let start = Instant::now();
    let mut seen = probe();
    let mut runner = StepRunner::new(vec![tap_step("double")], TIMING, seen);
    begin(&mut runner, start, &seen);
    seen = after_tap(seen);
    seen.camera_intents_sent += 1;
    let RunnerTick::Failed(reason) = runner.tick(start + Duration::from_millis(10), &seen) else {
        panic!("лишнее намерение обязано провалить шаг сразу");
    };
    assert!(
        reason.contains("намерения камере: ожидали ровно 1, увидели 2"),
        "{reason}"
    );
}

#[test]
fn something_extra_during_the_settle_window_fails_the_step() {
    let start = Instant::now();
    let mut seen = probe();
    let probe_tap_that_only_closes = PlannedStep {
        expect: Expectation::new()
            .count(Metric::Taps, Count::Exactly(1))
            .count(Metric::CameraIntents, Count::Exactly(0)),
        ..tap_step("closes_menu")
    };
    let mut runner = StepRunner::new(vec![probe_tap_that_only_closes], TIMING, seen);
    begin(&mut runner, start, &seen);
    seen.taps = 1;
    assert_eq!(
        runner.tick(start + Duration::from_millis(10), &seen),
        RunnerTick::Waiting
    );
    // В выдержку камере ушло намерение: тап на самом деле навёл фокус, меню не было открыто.
    seen.camera_intents_sent = 1;
    let RunnerTick::Failed(reason) = runner.tick(start + Duration::from_millis(50), &seen) else {
        panic!("лишнее намерение в выдержку обязано провалить шаг");
    };
    assert!(reason.contains("closes_menu"), "{reason}");
}

#[test]
fn a_hal_trigger_failure_fails_the_run_whatever_the_step() {
    let start = Instant::now();
    let mut seen = probe();
    let mut runner = StepRunner::new(vec![tap_step("any")], TIMING, seen);
    begin(&mut runner, start, &seen);
    seen.focus_trigger_failures = 1;
    let RunnerTick::Failed(reason) = runner.tick(start + Duration::from_millis(5), &seen) else {
        panic!("отказ HAL обязан провалить прогон");
    };
    assert!(reason.contains("HAL отверг 1 триггер"), "{reason}");
}

#[test]
fn a_lost_camera_intent_fails_the_run_with_its_own_reason() {
    let start = Instant::now();
    let mut seen = probe();
    let mut runner = StepRunner::new(vec![tap_step("any")], TIMING, seen);
    begin(&mut runner, start, &seen);
    seen.camera_intents_lost = 1;
    let RunnerTick::Failed(reason) = runner.tick(start + Duration::from_millis(5), &seen) else {
        panic!("потерянное намерение обязано провалить прогон");
    };
    assert!(reason.contains("очередь движка не приняла"), "{reason}");
}

#[test]
fn counters_from_before_the_plan_do_not_count_as_failures() {
    let start = Instant::now();
    let mut already_failed = probe();
    already_failed.focus_trigger_failures = 3;
    already_failed.camera_intents_lost = 2;
    let mut runner = StepRunner::new(vec![tap_step("only")], TIMING, already_failed);
    begin(&mut runner, start, &already_failed);
    let seen = after_tap(already_failed);
    runner.tick(start + Duration::from_millis(5), &seen);
    assert_eq!(
        runner.tick(start + Duration::from_millis(500), &seen),
        RunnerTick::AllDone
    );
}

#[test]
fn state_checks_gate_the_step_until_the_engine_confirms() {
    let start = Instant::now();
    let mut seen = probe();
    let lock = PlannedStep {
        expect: Expectation::new()
            .count(Metric::CameraIntents, Count::Exactly(1))
            .state(StateCheck::Lock(AeAfLock::Locked)),
        ..tap_step("lock")
    };
    let mut runner = StepRunner::new(vec![lock], TIMING, seen);
    begin(&mut runner, start, &seen);
    seen.camera_intents_sent = 1;
    // Намерение ушло, блокировки нет: не достигнуто, ждём.
    assert_eq!(
        runner.tick(start + Duration::from_millis(300), &seen),
        RunnerTick::Waiting
    );
    seen.lock = AeAfLock::Locked;
    runner.tick(start + Duration::from_millis(400), &seen);
    assert_eq!(
        runner.tick(start + Duration::from_millis(600), &seen),
        RunnerTick::AllDone
    );
}

#[test]
fn unfinished_names_lists_the_current_and_the_remaining_steps() {
    let start = Instant::now();
    let seen = probe();
    let mut runner = StepRunner::new(vec![tap_step("a"), tap_step("b"), tap_step("c")], TIMING, seen);
    begin(&mut runner, start, &seen);
    assert_eq!(runner.unfinished_names(), ["a", "b", "c"]);
}

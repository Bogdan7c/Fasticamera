use super::*;
use crate::screen_scenario_config::{ScreenPoint, ScreenTouchPlan};

const TIMEOUT: Duration = Duration::from_secs(10);
const WARMUP_FRAMES: u64 = 5;

fn swipes() -> SurfaceSwipes {
    let plan = ScreenTouchPlan::default();
    SurfaceSwipes {
        enter_calibrate: plan.modes_swipe_left,
        leave_calibrate: plan.modes_swipe_right,
    }
}

fn scopes(requested: bool, live: bool, passes: u64) -> Option<ScopesView> {
    Some(ScopesView {
        requested,
        resources_live: live,
        passes_recorded: passes,
    })
}

/// Живой экран в режиме `mode`, нарисовавший `frames` кадров.
fn view(mode: ModeKind, frames: u64, scopes: Option<ScopesView>) -> SurfaceView {
    SurfaceView {
        screen_installed: true,
        presence_live: true,
        mode: Some(mode),
        ui_overlay_active: true,
        ui_frames_drawn: frames,
        screen_frames: frames,
        scopes,
    }
}

fn entry(surface: StageBudgetSurface, now: Instant) -> SurfaceEntry {
    SurfaceEntry::begin(
        surface,
        swipes(),
        TIMEOUT,
        WARMUP_FRAMES,
        now,
        &view(ModeKind::Photo, 0, None),
    )
}

#[test]
fn bare_viewfinder_is_ready_at_once_and_asks_for_nothing() {
    let now = Instant::now();
    let mut entry = entry(StageBudgetSurface::BareViewfinder, now);
    assert_eq!(
        entry.tick(
            now,
            &SurfaceView {
                screen_installed: false,
                presence_live: false,
                mode: None,
                ui_overlay_active: false,
                ui_frames_drawn: 0,
                screen_frames: 0,
                scopes: None,
            }
        ),
        EntryTick::Ready
    );
}

#[test]
fn photo_screen_waits_for_a_live_warmed_screen_in_photo_mode_and_never_swipes() {
    let now = Instant::now();
    let mut entry = entry(StageBudgetSurface::PhotoScreen, now);
    // Экран нарисовал мало кадров.
    assert_eq!(entry.tick(now, &view(ModeKind::Photo, 2, None)), EntryTick::Waiting);
    // Достаточно кадров, но не Photo — не готов.
    assert_eq!(entry.tick(now, &view(ModeKind::Calibrate, 9, None)), EntryTick::Waiting);
    assert_eq!(entry.tick(now, &view(ModeKind::Photo, 9, None)), EntryTick::Ready);
}

#[test]
fn calibrate_asks_for_the_left_swipe_once_then_waits_for_the_mode_and_live_scopes() {
    let now = Instant::now();
    let mut entry = entry(StageBudgetSurface::CalibrateScreen, now);
    assert_eq!(entry.tick(now, &view(ModeKind::Photo, 1, None)), EntryTick::Waiting);
    let gesture = match entry.tick(now, &view(ModeKind::Photo, 9, scopes(false, false, 7))) {
        EntryTick::Gesture(gesture) => gesture,
        other => panic!("ждали жест, получили {other:?}"),
    };
    assert_eq!(gesture.step, "modes_swipe_left");
    assert_eq!(gesture.swipe, ScreenTouchPlan::default().modes_swipe_left);
    assert_eq!(
        gesture.to_stage_value()["kind"],
        "swipe",
        "жест уходит драйверу в том же формате, что у `screen`"
    );
    // Режим сменился, но прибор ещё не запрошен.
    assert_eq!(
        entry.tick(now, &view(ModeKind::Calibrate, 10, scopes(false, false, 7))),
        EntryTick::Waiting
    );
    // Запрошен и создан, но проходов после жеста ещё нет.
    assert_eq!(
        entry.tick(now, &view(ModeKind::Calibrate, 11, scopes(true, true, 7))),
        EntryTick::Waiting
    );
    // Проходы пошли — готово; второго жеста не было.
    assert_eq!(
        entry.tick(now, &view(ModeKind::Calibrate, 12, scopes(true, true, 9))),
        EntryTick::Ready
    );
}

#[test]
fn calibrate_that_never_gets_live_scopes_fails_naming_what_is_missing() {
    let start = Instant::now();
    let mut entry = entry(StageBudgetSurface::CalibrateScreen, start);
    let _gesture = entry.tick(start, &view(ModeKind::Photo, 9, scopes(false, false, 0)));
    let late = start + TIMEOUT + Duration::from_millis(1);
    match entry.tick(late, &view(ModeKind::Calibrate, 30, scopes(true, false, 0))) {
        EntryTick::Failed(reason) => {
            assert!(reason.contains("объекты GPU приборов не созданы"), "{reason}");
            assert!(reason.contains("проходы приборов не записываются"), "{reason}");
            assert!(!reason.contains("нужен Calibrate"), "режим уже верный: {reason}");
        }
        other => panic!("ждали провал, получили {other:?}"),
    }
}

#[test]
fn calibrate_swipe_that_never_arrives_fails_with_the_current_mode() {
    let start = Instant::now();
    let mut entry = entry(StageBudgetSurface::CalibrateScreen, start);
    let _gesture = entry.tick(start, &view(ModeKind::Photo, 9, scopes(false, false, 0)));
    let late = start + TIMEOUT;
    match entry.tick(late, &view(ModeKind::Photo, 40, scopes(false, false, 0))) {
        EntryTick::Failed(reason) => assert!(reason.contains("нужен Calibrate"), "{reason}"),
        other => panic!("ждали провал, получили {other:?}"),
    }
}

#[test]
fn a_screen_that_never_comes_up_fails_with_every_missing_piece() {
    let start = Instant::now();
    let mut entry = entry(StageBudgetSurface::PhotoScreen, start);
    let dead = SurfaceView {
        screen_installed: true,
        presence_live: false,
        mode: None,
        ui_overlay_active: false,
        ui_frames_drawn: 0,
        screen_frames: 1,
        scopes: None,
    };
    assert_eq!(entry.tick(start, &dead), EntryTick::Waiting);
    match entry.tick(start + TIMEOUT, &dead) {
        EntryTick::Failed(reason) => {
            for piece in [
                "не видит живую камеру",
                "источник UI не работает",
                "нарисовал 1 кадров из 5",
            ] {
                assert!(reason.contains(piece), "{piece}: {reason}");
            }
        }
        other => panic!("ждали провал, получили {other:?}"),
    }
}

#[test]
fn a_screen_left_in_calibrate_skips_the_swipe() {
    let now = Instant::now();
    let mut entry = entry(StageBudgetSurface::CalibrateScreen, now);
    // Раздел уже открыт с прошлого прогона: жеста не просим, ждём приборы.
    assert_eq!(
        entry.tick(now, &view(ModeKind::Calibrate, 9, scopes(true, true, 4))),
        EntryTick::Waiting
    );
    assert_eq!(
        entry.tick(now, &view(ModeKind::Calibrate, 10, scopes(true, true, 8))),
        EntryTick::Ready
    );
}

#[test]
fn exit_from_calibrate_swipes_right_once_and_waits_for_photo() {
    let now = Instant::now();
    let mut exit = SurfaceExit::begin(StageBudgetSurface::CalibrateScreen, swipes(), TIMEOUT, now, now);
    let gesture = match exit.tick(now, &view(ModeKind::Calibrate, 100, None)) {
        ExitTick::Gesture(gesture) => gesture,
        other => panic!("ждали жест, получили {other:?}"),
    };
    assert_eq!(gesture.step, "modes_swipe_right");
    assert_eq!(gesture.swipe, ScreenTouchPlan::default().modes_swipe_right);
    assert_eq!(exit.tick(now, &view(ModeKind::Calibrate, 101, None)), ExitTick::Waiting);
    assert_eq!(exit.tick(now, &view(ModeKind::Photo, 102, None)), ExitTick::Done);
}

#[test]
fn exit_that_never_reaches_photo_fails_and_other_places_have_nothing_to_return() {
    let now = Instant::now();
    let mut exit = SurfaceExit::begin(StageBudgetSurface::CalibrateScreen, swipes(), TIMEOUT, now, now);
    let _gesture = exit.tick(now, &view(ModeKind::Calibrate, 1, None));
    match exit.tick(now + TIMEOUT, &view(ModeKind::Calibrate, 99, None)) {
        ExitTick::Failed(reason) => assert!(reason.contains("нужен Photo"), "{reason}"),
        other => panic!("ждали провал, получили {other:?}"),
    }
    for surface in [StageBudgetSurface::BareViewfinder, StageBudgetSurface::PhotoScreen] {
        let mut exit = SurfaceExit::begin(surface, swipes(), TIMEOUT, now, now);
        assert_eq!(
            exit.tick(now, &view(ModeKind::Photo, 1, None)),
            ExitTick::Done,
            "{surface:?}"
        );
    }
    // Экран уже в Photo — свайпа не нужно и из CALIBRATE-замера.
    let mut exit = SurfaceExit::begin(StageBudgetSurface::CalibrateScreen, swipes(), TIMEOUT, now, now);
    assert_eq!(exit.tick(now, &view(ModeKind::Photo, 1, None)), ExitTick::Done);
    let _ = ScreenPoint::new(0.0, 0.0);
}

/// Исходник драйвера: таблица шагов сценария называет стадии, которые выдаёт Rust (общего механизма имён Rust <->
/// Python нет, сверяем текстом таблицы, как это делает `screen_gesture`).
const DRIVER_SOURCE: &str = include_str!("../../../../../scripts/smoke_driver.py");
/// Контракт жестов с драйвером: форму `long_press` и `swipe` читает Python.
const GESTURE_CONTRACT: &str = include_str!("../../../../../scripts/tests/fixtures/smoke/screen_gesture_contract.json");

#[test]
fn the_driver_table_waits_for_the_stages_this_scenario_emits() {
    use super::super::stage_budget_apply::HOLD_TOUCH_STAGE;
    let gesture_step = format!("\"{SURFACE_GESTURE_STAGE}\": (Step(StepKind.STAGE_GESTURE),)");
    assert!(
        DRIVER_SOURCE.contains(&gesture_step),
        "в SCENARIO_STEPS нет записи {gesture_step}"
    );
    let hold_step =
        format!("\"{HOLD_TOUCH_STAGE}\": (Step(StepKind.STAGE_GESTURE_BACKGROUND, only_if_true=\"needs_new_press\"),)");
    assert!(
        DRIVER_SOURCE.contains(&hold_step),
        "в SCENARIO_STEPS нет записи {hold_step}"
    );
}

#[test]
fn entry_and_hold_gestures_have_the_shape_the_driver_reads() {
    use crate::screen_scenario_config::HoldGesture;
    let contract: serde_json::Value = serde_json::from_str(GESTURE_CONTRACT).expect("фикстура — верный JSON");
    let gesture = SurfaceGesture {
        step: "modes_swipe_left",
        purpose: "проверка",
        swipe: ScreenTouchPlan::default().modes_swipe_left,
    };
    assert_eq!(
        gesture
            .to_stage_value()
            .as_object()
            .map(|map| map.keys().cloned().collect::<Vec<_>>()),
        contract["swipe"]
            .as_object()
            .map(|map| map.keys().cloned().collect::<Vec<_>>()),
        "поля свайпа совпадают с контрактом"
    );
    let hold = Gesture::long_press(HoldGesture {
        at: ScreenPoint::new(0.03, 0.85),
        hold_ms: 60_000,
    })
    .to_stage_value();
    assert_eq!(
        hold.as_object().map(|map| map.keys().cloned().collect::<Vec<_>>()),
        contract["long_press"]
            .as_object()
            .map(|map| map.keys().cloned().collect::<Vec<_>>())
    );
}

#[test]
fn exit_gesture_is_not_issued_before_the_held_touch_ends_plus_the_guard() {
    let now = Instant::now();
    let release = now + Duration::from_secs(8);
    let mut exit = SurfaceExit::begin(StageBudgetSurface::CalibrateScreen, swipes(), TIMEOUT, now, release);
    let in_calibrate = view(ModeKind::Calibrate, 1, None);
    assert_eq!(exit.tick(now, &in_calibrate), ExitTick::Waiting);
    assert_eq!(
        exit.tick(release - Duration::from_millis(1), &in_calibrate),
        ExitTick::Waiting
    );
    assert!(matches!(exit.tick(release, &in_calibrate), ExitTick::Gesture(_)));
    // Таймаут ожидания Photo считается от выданного жеста, а не от начала ожидания пальца.
    assert_eq!(
        exit.tick(release + TIMEOUT - Duration::from_millis(1), &in_calibrate),
        ExitTick::Waiting
    );
}

//! Функциональные тесты сценария `screen` (сессия 12): настоящий маршрутизатор, настоящий движок камеры на
//! fake-платформе, НАСТОЯЩИЙ `CameraScreen` с исполнителем эффектов, подставной «живой поток» (`StreamPump`) и
//! подставной рендер ([`RenderMirror`]). Тест играет двух актёров, которых на телефоне нет в процессе:
//! * **поток рендера** — каждый проход цикла рисует кадр установленным источником UI на синтетических часах
//!   (так экран разбирает вход, исполняет эффекты и публикует отчёт);
//! * **скрипт-драйвер** — читает стадии `screen_gesture` и подаёт в очередь оболочки касания и клавиши ТОЧНО из
//!   `gesture` стадии (как `adb input`), то есть по числам плана касаний из конфига. Неверная точка плана даёт
//!   промах по настоящему экрану и красный тест.
//!
//! Каждый тест доходит до строки результата. Помощники касаний и часов — копии минимальных из
//! `event_router_camera_screen_tests.rs` (тот модуль — не предок этого, его приватные функции отсюда не видны).

use camera_core::backend::{BackendFailure, RequestFailure};
use shell_contract::clock::MonotonicNanos;
use shell_contract::hardware_key::{HardwareKey, HardwareKeyEvent, KeyPhase, KeyRepeat};
use shell_contract::touch::{PointerId, SurfacePoint, TouchPhase, TouchSample};
use ui_draw::frame_source::UiFrameRequest;
use ui_draw::geometry::PxSize;

use super::*;
use crate::camera_screen_config::CameraScreenConfig;
use crate::camera_screen_service::{CameraScreenService, CameraScreenStatus};
use crate::gpu_test_support::RecordedUiOverlay;

const SURFACE_WIDTH_PX: u32 = 1440;
const SURFACE_HEIGHT_PX: u32 = 3216;
/// 1 pt = ширина / 440 px (решение 82).
const PX_PER_PT: f32 = SURFACE_WIDTH_PX as f32 / 440.0;
/// Период кадра синтетического рендера, мс.
const FRAME_MS: i64 = 16;
/// Шагов плана `screen` (счёт из `screen_steps`).
const STEP_COUNT: usize = 22;

/// Синтетические часы потока рендера: каждый кадр на 16 мс позже прошлого.
struct UiClock {
    now_ms: i64,
}

impl UiClock {
    fn new() -> Self {
        Self { now_ms: 0 }
    }

    /// Один кадр рендера установленным источником UI. `None` — источника сейчас нет (до установки экрана).
    fn frame(&mut self, harness: &Harness) {
        self.now_ms += FRAME_MS;
        let request = UiFrameRequest {
            present_time: MonotonicNanos::from_nanos(self.now_ms * 1_000_000),
            logical_size: PxSize::new(SURFACE_WIDTH_PX as f32, SURFACE_HEIGHT_PX as f32),
            px_per_pt: PX_PER_PT,
            backdrop: ui_draw::frame_source::BackdropContinuity::Live,
        };
        if let Some(produced) = harness.control.produce_ui_frame(&request) {
            produced.expect("кадр экрана построен");
        }
    }

    fn frames(&mut self, harness: &Harness, count: i64) {
        for _ in 0..count {
            self.frame(harness);
        }
    }
}

/// Касание в точке, заданной ДОЛЯМИ экрана (как `adb input`, через `wm size`).
fn touch(harness: &mut Harness, phase: TouchPhase, fraction_x: f64, fraction_y: f64, at_ms: i64) {
    harness.deliver(ShellEvent::Touch(TouchSample {
        pointer: PointerId::new(0),
        phase,
        position: SurfacePoint {
            x_px: (fraction_x * f64::from(SURFACE_WIDTH_PX - 1)) as f32,
            y_px: (fraction_y * f64::from(SURFACE_HEIGHT_PX - 1)) as f32,
        },
        time: MonotonicNanos::from_nanos(at_ms * 1_000_000),
    }));
}

fn key_event(harness: &mut Harness, phase: KeyPhase, at_ms: i64) {
    harness.deliver(ShellEvent::HardwareKey(HardwareKeyEvent {
        key: HardwareKey::VolumeUp,
        phase,
        repeat: KeyRepeat::Initial,
        time: MonotonicNanos::from_nanos(at_ms * 1_000_000),
    }));
}

fn number(gesture: &serde_json::Value, field: &str) -> f64 {
    gesture[field]
        .as_f64()
        .unwrap_or_else(|| panic!("в жесте стадии нет числа {field}: {gesture}"))
}

/// Выполняет жест из стадии `screen_gesture` так, как это сделал бы скрипт по `adb`. `stalled_render` — поток
/// рендера завис: образцы свайпа копятся в канале входа без единого кадра между ними.
fn perform(harness: &mut Harness, clock: &mut UiClock, gesture: &serde_json::Value, stalled_render: bool) {
    match gesture["kind"].as_str() {
        Some("tap") => {
            let (x, y) = (number(gesture, "x"), number(gesture, "y"));
            let start = clock.now_ms;
            touch(harness, TouchPhase::Began, x, y, start);
            clock.frame(harness);
            touch(harness, TouchPhase::Ended, x, y, start + 50);
        }
        Some("long_press") => {
            let (x, y) = (number(gesture, "x"), number(gesture, "y"));
            let hold_ms = number(gesture, "hold_ms") as i64;
            let start = clock.now_ms;
            touch(harness, TouchPhase::Began, x, y, start);
            // Палец стоит, пока рендер рисует кадры: долгий тап рождается по времени КАДРА.
            clock.frames(harness, hold_ms / FRAME_MS + 2);
            touch(harness, TouchPhase::Ended, x, y, start + hold_ms);
        }
        Some("swipe") => {
            let (x0, y0) = (number(gesture, "x"), number(gesture, "y"));
            let (x1, y1) = (number(gesture, "end_x"), number(gesture, "end_y"));
            let duration_ms = number(gesture, "duration_ms") as i64;
            let moves = (duration_ms / FRAME_MS).max(2);
            let start = clock.now_ms;
            touch(harness, TouchPhase::Began, x0, y0, start);
            for index in 1..=moves {
                let t = index as f64 / moves as f64;
                if !stalled_render {
                    clock.frame(harness);
                }
                touch(
                    harness,
                    TouchPhase::Moved,
                    x0 + (x1 - x0) * t,
                    y0 + (y1 - y0) * t,
                    start + duration_ms * index / moves,
                );
            }
            clock.frame(harness);
            touch(harness, TouchPhase::Ended, x1, y1, start + duration_ms + 10);
        }
        Some("volume_up") => {
            key_event(harness, KeyPhase::Pressed, clock.now_ms);
            clock.frames(harness, 3);
            key_event(harness, KeyPhase::Released, clock.now_ms);
        }
        other => panic!("неизвестный жест стадии {other:?}: {gesture}"),
    }
    clock.frames(harness, 4);
}

/// Конфиг движка с короткими сроками: тест идёт секунды, а не минуты.
fn screen_config(tune: impl FnOnce(&mut crate::screen_scenario_config::ScreenScenarioConfig)) -> CameraConfig {
    let mut config = short_config(|_, _| {});
    config.scenarios.screen.max_duration_ms = 60_000;
    config.scenarios.screen.step_timeout_ms = 3_000;
    config.scenarios.screen.step_settle_ms = 60;
    config.scenarios.screen.warmup_ui_frames = 8;
    config.scenarios.screen.warmup_timeout_ms = 2_000;
    tune(&mut config.scenarios.screen);
    config
}

/// Экран на маршрутизаторе, как в сквозных тестах экрана: свежий сервис и установка источника UI.
fn install_screen(harness: &mut Harness, config: CameraScreenConfig) {
    harness.router.camera_screen = CameraScreenService::new(config);
    harness.router.install_camera_screen();
}

/// Как ведёт себя «скрипт» в прогоне.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Driver {
    /// Выполняет каждый жест.
    Performs,
    /// Молчит: стадии идут в logcat, но никто не нажимает (скрипт упал или не читает logcat).
    Silent,
    /// Выполняет жесты, но поток рендера зависает на время свайпа (образцы копятся в канале входа).
    PerformsWhileRenderStalls,
}

/// Прогон `screen` до итога.
fn run_screen(
    config: CameraConfig,
    screen_config: CameraScreenConfig,
    faults: MirrorFaults,
    driver: Driver,
    tune_rig: impl FnOnce(&FakeCameraRig),
) -> (Harness, serde_json::Value) {
    let rig = FakeCameraRig::with_config(config);
    tune_rig(&rig);
    let mut harness = harness_for_rig(rig);
    install_screen(&mut harness, screen_config);
    harness.deliver(ShellEvent::Resumed);
    let paced = PumpFaults {
        paced_at_sensor_rate: true,
        ..PumpFaults::default()
    };
    let _pump = StreamPump::start(harness.rig(), Arc::clone(&harness.render), paced);
    let _mirror = RenderMirror::start(&harness, faults);
    harness.start(Scenario::parse("screen").expect("сценарий зарегистрирован"));
    let mut clock = UiClock::new();
    let mut gestures_done = 0;
    let deadline = Instant::now() + Duration::from_secs(60);
    while harness.output.results.lock().expect("замок").is_empty() && Instant::now() < deadline {
        harness.router.step(Duration::from_millis(2));
        clock.frame(&harness);
        let requested = stages_named(&harness, "screen_gesture");
        while driver != Driver::Silent && gestures_done < requested.len() {
            let stalled = driver == Driver::PerformsWhileRenderStalls;
            perform(&mut harness, &mut clock, &requested[gestures_done]["gesture"], stalled);
            gestures_done += 1;
        }
    }
    let result = harness.output.single_result();
    (harness, result)
}

fn run_healthy() -> (Harness, serde_json::Value) {
    run_screen(
        screen_config(|_| {}),
        instant_capture_config(),
        MirrorFaults::default(),
        Driver::Performs,
        |_| {},
    )
}

/// Экран с мгновенным ответом фейк-снимка (детерминированные тесты).
fn instant_capture_config() -> CameraScreenConfig {
    CameraScreenConfig {
        fake_capture_delay_ms: 0,
        // Отчёт каждый кадр: тесты читают его сразу после кадра (прореживание проверяет отдельный тест).
        report_min_interval_ms: 0,
        ..CameraScreenConfig::default()
    }
}

#[test]
fn the_script_drives_every_step_and_the_screen_proves_each_one_reached_the_camera() {
    let (harness, result) = run_healthy();

    assert_eq!(result["status"], "passed", "{result}");
    let report = &result["report"];
    let steps = report["steps"].as_array().expect("шаги");
    assert_eq!(steps.len(), STEP_COUNT, "{steps:?}");
    assert!(steps.iter().all(|step| step["ok"] == true), "{steps:?}");
    assert_eq!(steps[0]["name"], "focus_tap_center");
    assert_eq!(steps[STEP_COUNT - 1]["name"], "volume_up_shutter");
    assert_eq!(report["unfinished"], serde_json::json!([]));
    assert_eq!(report["plan_failure"], serde_json::Value::Null);

    // Доказательство «тап дошёл до HAL»: движок отправил триггер на каждый тап, блокировку и снятие блокировки
    // (центр, левый верх, правый низ, блокировка, снятие, две пробы закрытых панелей и проба после EV).
    let status = harness.rig().handle().status();
    assert_eq!(
        status.three_a.counters.focus_triggers_sent, 7,
        "{:?}",
        status.three_a.counters
    );
    assert_eq!(status.three_a.counters.focus_trigger_failures, 0);
    // План кончается нейтрально: блокировки нет, EV сброшен.
    assert_eq!(status.three_a.lock, camera_contract::three_a::AeAfLock::Unlocked);
    assert_eq!(status.three_a.controls.exposure_bias.ev(), 0.0);

    // Клавиша громкости сделала ровно один фейк-снимок.
    assert_eq!(harness.router.camera_screen().capture_journal().len(), 1);

    // Счётчики экрана за окно чистые, кадры рисовались.
    assert!(report["screen"]["frames_produced"].as_u64().expect("кадры") > 0);
    for zero in [
        "frame_failures",
        "effects_overflowed",
        "unhandled_gestures",
        "orphan_pan_continuations",
        "finger_focus_refusals",
        "input_overflowed",
        "executor_refusals",
        "input_dropped_without_screen",
    ] {
        assert_eq!(report["screen"][zero], 0, "{zero}: {report}");
    }
    assert_eq!(report["overlay"]["active_source"], "camera_screen");
    assert_eq!(report["ui_gpu_budget"]["budget_ms"], 8.33, "60 Гц × 0,5, решение 90");
    assert!(
        report["late_frames"].is_u64(),
        "late_frames — только цифра (решение 91)"
    );
    assert_fits_logcat(&harness, &result);
}

/// Шаги белого идут по НАСТОЯЩЕЙ карточке белого: точки плана (значок, кнопка света, долгое нажатие) попадают, а
/// не промахиваются мимо (иначе шаг краснеет «ожидали X, увидели Y»).
// Точки плана сверены с целями настоящего экрана отдельным тестом (`screen_scenario_touch_targets_tests.rs`).
#[test]
fn the_white_steps_hit_the_real_card_and_the_long_press_ends_with_an_honest_outcome() {
    let (harness, result) = run_healthy();
    assert_eq!(result["status"], "passed", "{result}");
    let steps = result["report"]["steps"].as_array().expect("шаги");
    for name in [
        "white_button_tap",
        "white_light_tap",
        "white_card_close_tap",
        "white_long_press",
    ] {
        let step = steps
            .iter()
            .find(|step| step["name"] == name)
            .unwrap_or_else(|| panic!("нет шага {name}"));
        assert_eq!(step["ok"], true, "{name}: {step}");
    }
    let white = harness.rig().handle().status().three_a.white;
    // Исход последней команды «взять» закрыт (не завис в ожидании) и различим: взято телефоном либо честный отказ.
    let take = white.last_take.expect("команда «взять» дошла до движка");
    assert!(
        !matches!(take.state, camera_core::white::WhiteTakeState::Waiting),
        "{take:?}"
    );
    match take.state {
        camera_core::white::WhiteTakeState::Finished(camera_core::white::WhiteRequestOutcome::Applied { .. }) => {
            assert_eq!(
                white.held.origin,
                camera_contract::white::WhiteOrigin::Source(camera_contract::white::WhiteSourceId::PHONE)
            );
        }
        // Отказ: белое осталось от кнопки света (рука).
        _ => assert_eq!(
            white.held.origin,
            camera_contract::white::WhiteOrigin::Manual,
            "{take:?}"
        ),
    }
    // Экран вернулся в исходное состояние: панели закрыты.
    assert_eq!(
        harness.router.camera_screen().report().expect("отчёт").state.panel,
        crate::camera_screen_source::PanelKind::Closed
    );
}

#[test]
fn each_step_stage_is_emitted_once_after_screen_ready_and_the_screen_is_never_reinstalled() {
    let (harness, _result) = run_healthy();

    let names = harness.output.stage_names();
    let position = |name: &str| names.iter().position(|stage| stage == name).expect(name);
    assert!(position("streaming") < position("screen_ready"), "{names:?}");
    assert!(position("screen_ready") < position("screen_gesture"), "{names:?}");
    assert!(
        names.iter().filter(|name| *name == "screen_ready").count() == 1,
        "{names:?}"
    );
    // Драйвер выполняет шаги стадии при КАЖДОЙ её выдаче, поэтому стадия каждого шага — ровно одна.
    let gestures = stages_named(&harness, "screen_gesture");
    let mut steps: Vec<_> = gestures
        .iter()
        .map(|stage| stage["step"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(steps.len(), STEP_COUNT);
    steps.sort_unstable();
    steps.dedup();
    assert_eq!(steps.len(), STEP_COUNT, "шаг запрошен дважды");
    assert!(position("screen_gesture") < position("detail_screen"), "{names:?}");
    // Экран остался тем же: не снимался на сценарий и не ставился заново (KeepRunning).
    assert_eq!(harness.router.camera_screen().installs(), 1);
    assert_eq!(
        harness.control.recorded_ui_overlay(),
        vec![RecordedUiOverlay::Install("camera_screen")]
    );
    assert_eq!(*harness.router.camera_screen().status(), CameraScreenStatus::Installed);
}

#[test]
fn a_trigger_the_hal_rejects_fails_the_run_with_the_reason() {
    let (_harness, result) = run_screen(
        screen_config(|_| {}),
        instant_capture_config(),
        MirrorFaults::default(),
        Driver::Performs,
        |rig| {
            // HAL отвергает первый разовый запрос: поток живёт, тап не доставлен (как в `3a`).
            rig.control
                .push_trigger_result(Err(RequestFailure::Backend(BackendFailure {
                    operation: "ACameraCaptureSession_capture",
                    code: -1,
                    detail: "тестовый отказ".to_owned(),
                })));
        },
    );

    assert_eq!(result["status"], "failed", "{result}");
    let error = result["error"].as_str().unwrap_or_default();
    assert!(
        error.contains("HAL отверг 1 триггер") && error.contains("не до HAL"),
        "{error}"
    );
}

#[test]
fn a_script_that_never_touches_the_screen_fails_the_first_step_by_its_deadline() {
    let config = screen_config(|screen| screen.step_timeout_ms = 400);
    let (_harness, result) = run_screen(
        config,
        instant_capture_config(),
        MirrorFaults::default(),
        Driver::Silent,
        |_| {},
    );

    assert_eq!(result["status"], "failed", "{result}");
    let error = result["error"].as_str().unwrap_or_default();
    assert!(
        error.contains("focus_tap_center")
            && error.contains("тапы: ожидали ровно 1, увидели 0")
            && error.contains("срок вышел"),
        "{error}"
    );
}

#[test]
fn a_menu_swipe_that_misses_fails_that_step_by_the_panel_not_a_later_probe() {
    // Свайп распознаётся как пан (счётчик свайпов вырастет), но идёт ГОРИЗОНТАЛЬНО: меню не открывается.
    // Раньше это вылезло бы только на следующей тап-пробе; теперь шаг сам видит, что панель осталась Closed.
    let config = screen_config(|screen| {
        screen.step_timeout_ms = 600;
        screen.touch.menu_open_swipe = crate::screen_scenario_config::SwipeGesture {
            from: crate::screen_scenario_config::ScreenPoint::new(0.7, 0.5),
            to: crate::screen_scenario_config::ScreenPoint::new(0.3, 0.5),
            duration_ms: 300,
        };
    });
    let (_harness, result) = run_screen(
        config,
        instant_capture_config(),
        MirrorFaults::default(),
        Driver::Performs,
        |_| {},
    );

    assert_eq!(result["status"], "failed", "{result}");
    let error = result["error"].as_str().unwrap_or_default();
    assert!(
        error.contains("menu_open_swipe")
            && error.contains("панель (panel) ControlsMenu, увидели Closed")
            && !error.contains("menu_probe_tap_opened"),
        "{error}"
    );
}

#[test]
fn a_screen_that_is_not_installed_fails_before_the_camera_is_touched() {
    let rig = FakeCameraRig::with_config(screen_config(|_| {}));
    let mut harness = harness_for_rig(rig);
    install_screen(
        &mut harness,
        CameraScreenConfig {
            enabled: false,
            ..CameraScreenConfig::default()
        },
    );
    harness.deliver(ShellEvent::Resumed);
    harness.start(Scenario::parse("screen").expect("сценарий зарегистрирован"));
    let deadline = Instant::now() + Duration::from_secs(5);
    while harness.output.results.lock().expect("замок").is_empty() && Instant::now() < deadline {
        harness.router.step(Duration::from_millis(2));
    }
    let result = harness.output.single_result();

    assert_eq!(result["status"], "failed", "{result}");
    let error = result["error"].as_str().unwrap_or_default();
    assert!(
        error.contains("экран камеры не установлен") && error.contains("Disabled"),
        "{error}"
    );
    assert!(harness.output.stage_names().iter().all(|name| name != "screen_gesture"));
}

#[test]
fn input_channel_overflow_is_carried_to_the_verdict() {
    // Очередь на 2 сообщения: свайп (десятки образцов) заведомо переполняет канал между кадрами рендера.
    let tiny_queue = CameraScreenConfig {
        input_queue_capacity: 2,
        ..instant_capture_config()
    };
    let (_harness, result) = run_screen(
        screen_config(|screen| screen.step_timeout_ms = 600),
        tiny_queue,
        MirrorFaults::default(),
        Driver::PerformsWhileRenderStalls,
        |_| {},
    );

    assert_eq!(result["status"], "failed", "{result}");
    let error = result["error"].as_str().unwrap_or_default();
    assert!(error.contains("потерь ввода из-за переполнения канала"), "{error}");
    assert!(
        result["report"]["screen"]["input_overflowed"]
            .as_u64()
            .expect("счётчик")
            > 0,
        "переполнение канала входа обязано дойти до отчёта: {result}"
    );
}

#[test]
fn ui_gpu_time_over_half_the_frame_period_fails_a_run_whose_steps_all_passed() {
    let faults = MirrorFaults {
        ui_overlay: UiOverlayFault::SlowGpu,
        ..MirrorFaults::default()
    };
    let (_harness, result) = run_screen(
        screen_config(|_| {}),
        instant_capture_config(),
        faults,
        Driver::Performs,
        |_| {},
    );

    assert_eq!(result["status"], "failed", "{result}");
    let error = result["error"].as_str().unwrap_or_default();
    assert!(
        error.contains("GPU-время кадра UI") && error.contains("8.33"),
        "{error}"
    );
    assert!(
        result["report"]["steps"]
            .as_array()
            .expect("шаги")
            .iter()
            .all(|step| step["ok"] == true),
        "все шаги прошли, провалена только плавность: {result}"
    );
}

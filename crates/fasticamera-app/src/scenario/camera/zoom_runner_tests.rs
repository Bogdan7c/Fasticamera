//! Исполнитель шагов `zoom` на подставных наблюдениях: время и кадры подаёт тест, так что каждое правило проверяется
//! значением в записи шага (а не «функция вызвалась»).

use std::time::Duration;

use camera_contract::fixtures::camera_id;
use camera_contract::geometry::{PixelRect, PixelSize};
use camera_contract::viewfinder_crop::{CropAspect, CropCenter, ViewfinderCrop, ZoomFactor};
use camera_contract::zoom::{FramedCrop, PreviewMirror, ZoomFit, ZoomRatio};
use camera_core::lens_status::LensSwitchFailure;
use render_loop::viewfinder::{BlackKind, FrameCropReport, FrameLabel, FrameRecord, ViewfinderCounters};
use ui_draw::frame_source::BackdropContinuity;

use super::*;
use crate::camera_screen_effects::RecentHapticTimes;
use crate::camera_screen_source::ZoomView;
use crate::camera_test_support::oneplus_zoom_catalog;
use crate::scenario::camera::zoom_views::ActiveLensView;

const MS: i64 = 1_000_000;

fn at_ms(ms: i64) -> MonotonicNanos {
    MonotonicNanos::from_nanos(ms * MS)
}

fn step(name: &str, action: ZoomAction) -> ZoomStep {
    ZoomStep {
        name: name.to_owned(),
        action,
    }
}

fn engine(role: LensRole, camera: &str, facing: LensFacing) -> EngineView {
    EngineView {
        streaming: true,
        active: Some(ActiveLensView {
            role,
            camera: camera_id(camera),
            facing,
        }),
        catalog: oneplus_zoom_catalog(),
        measured_fps: Some(59.7),
    }
}

fn live_frame(ms: i64, camera: &str, mirror: PreviewMirror) -> FrameRecord {
    FrameRecord {
        present_time: at_ms(ms),
        label: FrameLabel::Live,
        backdrop: BackdropContinuity::Live,
        crop: Some(FrameCropReport {
            camera: camera_id(camera),
            resolved: PixelRect {
                left: 0,
                top: 0,
                size: PixelSize::new(4096, 3072),
            },
            framed: FramedCrop {
                crop: ViewfinderCrop {
                    aspect: CropAspect::SensorNative,
                    zoom: ZoomFactor::NONE,
                    center: CropCenter::ValidAreaCenter,
                },
                mirror,
                shown_ratio: ZoomRatio::ONE,
                fit: ZoomFit::Exact,
            },
        }),
        held_magnification: 1.0,
    }
}

fn screen(ratio_milli: u32, facing: LensFacing, published: u64) -> ScreenView {
    ScreenView {
        zoom: ZoomView {
            ratio_milli,
            facing,
            wheel_open: false,
            gliding: false,
        },
        haptics: RecentHapticTimes::default(),
        lens_published: published,
    }
}

/// Стенд: часы маршрутизатора и рендера идут синхронно, наблюдение собирается из полей.
struct Rig {
    runner: ZoomRunner,
    config: ZoomScenarioConfig,
    base: Instant,
    elapsed_ms: i64,
    engine: EngineView,
    frames: Vec<FrameRecord>,
    counters: ContinuityCounters,
    isp_counters: ViewfinderCounters,
    screen: ScreenView,
}

impl Rig {
    fn new(steps: Vec<ZoomStep>, engine: EngineView, screen: ScreenView) -> Self {
        let config = ZoomScenarioConfig::default();
        Self {
            runner: ZoomRunner::new(config.clone(), steps),
            config,
            base: Instant::now(),
            elapsed_ms: 1_000,
            engine,
            frames: vec![live_frame(1_000, "2", PreviewMirror::None)],
            counters: ContinuityCounters::default(),
            isp_counters: ViewfinderCounters::default(),
            screen,
        }
    }

    /// Один тик: `after_ms` прошло с прошлого; `events` пришли в этом тике.
    fn tick(&mut self, after_ms: i64, events: &[CameraEvent]) -> ZoomTick {
        self.elapsed_ms += after_ms;
        let observation = ZoomObservation {
            now: self.base + Duration::from_millis(u64::try_from(self.elapsed_ms).expect("время растёт")),
            render_now: at_ms(self.elapsed_ms),
            engine: &self.engine,
            events,
            frames: &self.frames,
            counters: &self.counters,
            isp_counters: &self.isp_counters,
            screen: &self.screen,
        };
        self.runner.tick(&observation)
    }
}

fn switched(from: LensRole, to: LensRole) -> CameraEvent {
    CameraEvent::LensSwitched {
        from,
        to,
        elapsed: Duration::from_millis(380),
    }
}

fn only_action(tick: &ZoomTick) -> &ScreenAction {
    match tick.injections.as_slice() {
        [Injection::Action(action)] => action,
        other => panic!("ждали одно действие, получили {other:?}"),
    }
}

/// Доводит шаг до записи: тики по 100 мс, пока не появится запись или провал.
fn run_to_record(rig: &mut Rig) -> ZoomTick {
    for _ in 0..200 {
        let tick = rig.tick(100, &[]);
        if !rig.runner.records().is_empty() || matches!(tick.state, RunnerState::Failed(_)) {
            return tick;
        }
    }
    panic!("шаг не закончился: {:?}", rig.runner.unfinished_names());
}

#[test]
fn a_tap_to_ultra_wide_injects_the_stop_action_and_records_both_timings() {
    let mut rig = Rig::new(
        vec![step("01_tap_0.6", ZoomAction::TapStop { ratio: 0.6 })],
        engine(LensRole::Wide, "2", LensFacing::Back),
        screen(1_000, LensFacing::Back, 0),
    );
    let started = rig.tick(0, &[]);
    assert_eq!(
        only_action(&started),
        &ScreenAction::ZoomStopTapped {
            stop: 0,
            at: at_ms(1_000)
        },
        "кнопка .6 — первая в ленте, метка действия — время рендера"
    );
    // Экран принял тап и положил намерение; движок сменил объектив; новый кадр пошёл через 150 мс после действия.
    rig.screen = screen(590, LensFacing::Back, 1);
    rig.engine = engine(LensRole::UltraWide, "3", LensFacing::Back);
    rig.frames.push(live_frame(1_150, "3", PreviewMirror::None));
    rig.counters.held_frames_presented = 9;
    let tick = rig.tick(200, &[switched(LensRole::Wide, LensRole::UltraWide)]);
    assert_eq!(tick.state, RunnerState::Running, "после смены идёт наблюдение");
    assert!(rig.runner.records().is_empty(), "запись — только после наблюдения");

    let done = run_to_record(&mut rig);

    assert_eq!(done.state, RunnerState::Running);
    let record = &rig.runner.records()[0];
    assert_eq!(record.failure, None);
    assert_eq!(record.expected, "to_ultra_wide");
    let switch = record.switch.expect("смена записана");
    assert_eq!(
        (switch.from, switch.to, switch.engine_elapsed_ms),
        ("wide", "ultra_wide", 380)
    );
    assert_eq!(switch.observed_ms, 200, "от тика действия до тика, где пришло событие");
    assert_eq!(record.first_live_frame_ms, Some(150.0));
    assert_eq!(record.new_camera.as_deref(), Some("3"));
    assert_eq!(record.held_frames, 9);
    assert_eq!(record.black_frames_while_picture_expected, 0);
    assert_eq!(record.mirror, MirrorCheck::Correct);
    assert_eq!((record.measured_fps, record.expected_fps), (Some(59.7), Some(60)));
    assert_eq!(record.lens_intents_published, 1);
}

#[test]
fn black_frames_of_the_step_are_a_delta_from_the_step_start() {
    let mut rig = Rig::new(
        vec![step("01_tap_0.6", ZoomAction::TapStop { ratio: 0.6 })],
        engine(LensRole::Wide, "2", LensFacing::Back),
        screen(1_000, LensFacing::Back, 0),
    );
    rig.counters.black_frames_while_picture_expected = 5; // было до шага
    rig.tick(0, &[]);
    rig.screen = screen(590, LensFacing::Back, 1);
    rig.frames.push(live_frame(1_100, "3", PreviewMirror::None));
    rig.counters.black_frames_while_picture_expected = 7; // два новых за шаг
    rig.tick(150, &[switched(LensRole::Wide, LensRole::UltraWide)]);
    run_to_record(&mut rig);

    assert_eq!(rig.runner.records()[0].black_frames_while_picture_expected, 2);
}

#[test]
fn a_tap_on_2x_from_the_main_camera_expects_no_switch_and_passes_after_the_watch_period() {
    let mut rig = Rig::new(
        vec![step("03_tap_2", ZoomAction::TapStop { ratio: 2.0 })],
        engine(LensRole::Wide, "2", LensFacing::Back),
        screen(1_000, LensFacing::Back, 0),
    );
    let start = rig.tick(0, &[]);
    assert!(
        matches!(only_action(&start), ScreenAction::ZoomStopTapped { stop: 2, .. }),
        "кнопка 2 — третья"
    );
    rig.screen = screen(2_000, LensFacing::Back, 1);
    let done = run_to_record(&mut rig);

    assert_eq!(done.state, RunnerState::Running);
    let record = &rig.runner.records()[0];
    assert_eq!(record.expected, "no_switch");
    assert_eq!(record.switch, None);
    assert_eq!(record.failure, None);
    assert_eq!(record.mirror, MirrorCheck::NotApplicable);
}

#[test]
fn an_unexpected_lens_switch_during_a_no_switch_step_fails_the_run() {
    let mut rig = Rig::new(
        vec![step("03_tap_2", ZoomAction::TapStop { ratio: 2.0 })],
        engine(LensRole::Wide, "2", LensFacing::Back),
        screen(1_000, LensFacing::Back, 0),
    );
    rig.tick(0, &[]);
    let tick = rig.tick(100, &[switched(LensRole::Wide, LensRole::UltraWide)]);

    let RunnerState::Failed(reason) = tick.state else {
        panic!("ждали провал, получили {:?}", tick.state);
    };
    assert!(
        reason.contains("03_tap_2") && reason.contains("ждали no_switch"),
        "{reason}"
    );
    assert_eq!(rig.runner.records().len(), 1, "провалившийся шаг записан");
}

#[test]
fn a_failed_switch_event_fails_the_step_and_names_the_roles() {
    let mut rig = Rig::new(
        vec![step("01_tap_0.6", ZoomAction::TapStop { ratio: 0.6 })],
        engine(LensRole::Wide, "2", LensFacing::Back),
        screen(1_000, LensFacing::Back, 0),
    );
    rig.tick(0, &[]);
    let failure = CameraEvent::LensSwitchFailed {
        from: LensRole::Wide,
        to: LensRole::UltraWide,
        reason: LensSwitchFailure::Unavailable {
            reason: camera_contract::availability::CameraUnavailableReason::DeviceFailure,
        },
    };
    let tick = rig.tick(100, &[failure]);

    let RunnerState::Failed(reason) = tick.state else {
        panic!("ждали провал");
    };
    assert!(
        reason.contains("wide→ultra_wide") && reason.contains("не удалась"),
        "{reason}"
    );
    assert!(rig.runner.records()[0].failure.is_some());
}

#[test]
fn a_switch_that_never_happens_times_out_with_the_expectation_in_the_reason() {
    let mut rig = Rig::new(
        vec![step("01_tap_0.6", ZoomAction::TapStop { ratio: 0.6 })],
        engine(LensRole::Wide, "2", LensFacing::Back),
        screen(1_000, LensFacing::Back, 0),
    );
    rig.tick(0, &[]);
    let tick = rig.tick(i64::try_from(rig.config.step_timeout_ms).expect("мс"), &[]);

    let RunnerState::Failed(reason) = tick.state else {
        panic!("ждали провал по сроку");
    };
    assert!(
        reason.contains("не сообщил о смене объектива") && reason.contains("to_ultra_wide"),
        "{reason}"
    );
}

#[test]
fn a_reported_switch_without_a_live_frame_of_the_new_camera_times_out_with_its_own_reason() {
    let mut rig = Rig::new(
        vec![step("01_tap_0.6", ZoomAction::TapStop { ratio: 0.6 })],
        engine(LensRole::Wide, "2", LensFacing::Back),
        screen(1_000, LensFacing::Back, 0),
    );
    rig.tick(0, &[]);
    rig.tick(100, &[switched(LensRole::Wide, LensRole::UltraWide)]);
    let tick = rig.tick(i64::try_from(rig.config.step_timeout_ms).expect("мс"), &[]);

    let RunnerState::Failed(reason) = tick.state else {
        panic!("ждали провал по сроку");
    };
    assert!(reason.contains("живого кадра новой камеры"), "{reason}");
}

#[test]
fn flipping_to_the_front_expects_a_front_switch_and_checks_the_mirror_of_its_frames() {
    let mut rig = Rig::new(
        vec![step("06_flip_to_front", ZoomAction::Flip)],
        engine(LensRole::Wide, "2", LensFacing::Back),
        screen(1_000, LensFacing::Back, 0),
    );
    let start = rig.tick(0, &[]);
    assert!(matches!(only_action(&start), ScreenAction::FlipCameraRequested { .. }));
    rig.screen = screen(1_000, LensFacing::Front, 1);
    rig.engine = engine(LensRole::Front, "1", LensFacing::Front);
    rig.engine.measured_fps = Some(29.8);
    rig.frames.push(live_frame(1_300, "1", PreviewMirror::Horizontal));
    rig.tick(320, &[switched(LensRole::Wide, LensRole::Front)]);
    run_to_record(&mut rig);

    let record = &rig.runner.records()[0];
    assert_eq!(record.expected, "to_front");
    assert_eq!(
        record.mirror,
        MirrorCheck::Correct,
        "у фронтальной зеркало по горизонтали"
    );
    assert_eq!(record.expected_fps, Some(30), "фронтальная держит 30 к/с");
    assert_eq!(record.failure, None);
}

#[test]
fn front_frames_without_the_mirror_are_recorded_as_wrong() {
    let mut rig = Rig::new(
        vec![step("06_flip_to_front", ZoomAction::Flip)],
        engine(LensRole::Wide, "2", LensFacing::Back),
        screen(1_000, LensFacing::Back, 0),
    );
    rig.tick(0, &[]);
    rig.screen = screen(1_000, LensFacing::Front, 1);
    rig.engine = engine(LensRole::Front, "1", LensFacing::Front);
    rig.frames.push(live_frame(1_300, "1", PreviewMirror::None));
    rig.tick(320, &[switched(LensRole::Wide, LensRole::Front)]);
    run_to_record(&mut rig);

    assert_eq!(rig.runner.records()[0].mirror, MirrorCheck::Wrong { frames: 1 });
}

#[test]
fn flipping_back_from_the_front_accepts_any_back_lens() {
    let mut rig = Rig::new(
        vec![step("07_flip_to_back", ZoomAction::Flip)],
        engine(LensRole::Front, "1", LensFacing::Front),
        screen(1_000, LensFacing::Front, 0),
    );
    rig.tick(0, &[]);
    rig.screen = screen(590, LensFacing::Back, 1);
    rig.engine = engine(LensRole::UltraWide, "3", LensFacing::Back);
    rig.frames.push(live_frame(1_250, "3", PreviewMirror::None));
    rig.tick(300, &[switched(LensRole::Front, LensRole::UltraWide)]);
    run_to_record(&mut rig);

    let record = &rig.runner.records()[0];
    assert_eq!(
        (record.expected.as_str(), record.failure.clone()),
        ("to_any_back", None)
    );
    assert_eq!(record.new_camera.as_deref(), Some("3"));
}

#[test]
fn the_wheel_drag_sends_began_then_evenly_spaced_moved_samples_then_ended() {
    let mut rig = Rig::new(
        vec![step("09_wheel_drag", ZoomAction::WheelDrag)],
        engine(LensRole::Wide, "2", LensFacing::Back),
        screen(1_000, LensFacing::Back, 0),
    );
    let plan = rig.config.wheel_drag;
    let first = rig.tick(0, &[]);
    let [Injection::Touch(began)] = first.injections.as_slice() else {
        panic!("ждали один образец Began, получили {:?}", first.injections);
    };
    assert_eq!((began.phase, began.point), (TouchPhase::Began, plan.start));

    let mut samples = Vec::new();
    for _ in 0..200 {
        let tick = rig.tick(i64::try_from(plan.sample_interval_ms).expect("мс"), &[]);
        for injection in tick.injections {
            let Injection::Touch(touch) = injection else {
                panic!("в протяжке только касания");
            };
            samples.push(touch);
        }
        if samples.iter().any(|touch| touch.phase == TouchPhase::Ended) {
            break;
        }
    }

    let moved: Vec<&TouchInjection> = samples
        .iter()
        .filter(|touch| touch.phase == TouchPhase::Moved)
        .collect();
    assert_eq!(moved.len() as u32, plan.samples);
    assert_eq!(samples.last().map(|touch| touch.phase), Some(TouchPhase::Ended));
    let gaps: Vec<i64> = samples
        .windows(2)
        .map(|pair| (pair[1].time.as_nanos() - pair[0].time.as_nanos()) / MS)
        .collect();
    assert!(
        gaps.iter()
            .all(|gap| *gap == i64::try_from(plan.sample_interval_ms).expect("мс")),
        "метки идут ровным шагом: {gaps:?}"
    );
    assert!(
        moved.windows(2).all(|pair| pair[1].point.x < pair[0].point.x),
        "палец идёт влево"
    );
    assert!(
        (moved[0].point.y - plan.start.y).abs() < f64::EPSILON,
        "высота не меняется"
    );
}

#[test]
fn a_drag_record_carries_the_haptics_of_its_window_only() {
    let mut rig = Rig::new(
        vec![step("09_wheel_drag", ZoomAction::WheelDrag)],
        engine(LensRole::Wide, "2", LensFacing::Back),
        screen(1_000, LensFacing::Back, 0),
    );
    // До шага уже играла одна вибрация: в окно она не входит.
    let before = [at_ms(500)];
    rig.screen.haptics = RecentHapticTimes::from_times(&before);
    rig.tick(0, &[]);
    let in_window: Vec<MonotonicNanos> = [500, 1_100, 1_140, 1_172, 1_300].into_iter().map(at_ms).collect();
    rig.screen = ScreenView {
        haptics: RecentHapticTimes::from_times(&in_window),
        ..screen(5_400, LensFacing::Back, 12)
    };
    for _ in 0..200 {
        rig.tick(16, &[]);
        if !rig.runner.records().is_empty() {
            break;
        }
    }

    let record = &rig.runner.records()[0];
    assert_eq!(record.failure, None);
    let haptics = record.haptics.expect("у протяжки есть окно вибраций");
    assert_eq!(haptics.count, 4, "вибрация до шага не считается");
    assert!(haptics.window_ms > 0.0, "окно меряется от действия до конца наблюдения");
    assert_eq!(record.screen_ratio, 5.4);
}

#[test]
fn a_step_without_a_published_lens_intent_fails() {
    let mut rig = Rig::new(
        vec![step("01_tap_0.6", ZoomAction::TapStop { ratio: 0.6 })],
        engine(LensRole::Wide, "2", LensFacing::Back),
        screen(1_000, LensFacing::Back, 4),
    );
    rig.tick(0, &[]);
    rig.screen = screen(590, LensFacing::Back, 4); // намерение не прибавилось
    rig.frames.push(live_frame(1_100, "3", PreviewMirror::None));
    rig.tick(150, &[switched(LensRole::Wide, LensRole::UltraWide)]);
    let tick = run_to_record(&mut rig);

    let RunnerState::Failed(reason) = tick.state else {
        panic!("ждали провал");
    };
    assert!(reason.contains("не положил намерение объектива"), "{reason}");
}

#[test]
fn the_screen_showing_another_ratio_than_the_chosen_stop_fails_the_step() {
    let mut rig = Rig::new(
        vec![step("03_tap_2", ZoomAction::TapStop { ratio: 2.0 })],
        engine(LensRole::Wide, "2", LensFacing::Back),
        screen(1_000, LensFacing::Back, 0),
    );
    rig.tick(0, &[]);
    rig.screen = screen(1_400, LensFacing::Back, 1); // глайд застрял на 1,4×
    let tick = run_to_record(&mut rig);

    let RunnerState::Failed(reason) = tick.state else {
        panic!("ждали провал");
    };
    assert!(reason.contains("1400"), "{reason}");
}

#[test]
fn a_tap_for_a_ratio_without_a_button_fails_before_touching_the_screen() {
    let mut rig = Rig::new(
        vec![step("01_tap_5", ZoomAction::TapStop { ratio: 5.0 })],
        engine(LensRole::Wide, "2", LensFacing::Back),
        screen(1_000, LensFacing::Back, 0),
    );
    let tick = rig.tick(0, &[]);

    assert!(tick.injections.is_empty(), "действие не вложено");
    let RunnerState::Failed(reason) = tick.state else {
        panic!("ждали провал");
    };
    assert!(reason.contains("нет кнопки"), "{reason}");
}

#[test]
fn a_step_does_not_start_while_the_engine_is_not_streaming() {
    let mut view = engine(LensRole::Wide, "2", LensFacing::Back);
    view.streaming = false;
    let mut rig = Rig::new(
        vec![step("01_tap_0.6", ZoomAction::TapStop { ratio: 0.6 })],
        view,
        screen(1_000, LensFacing::Back, 0),
    );
    let tick = rig.tick(0, &[]);

    assert!(tick.injections.is_empty());
    assert!(matches!(tick.state, RunnerState::Failed(_)));
}

#[test]
fn a_ring_that_lost_records_lets_the_step_finish_with_the_gap_marked_instead_of_hanging() {
    let mut rig = Rig::new(
        vec![step("01_tap_0.6", ZoomAction::TapStop { ratio: 0.6 })],
        engine(LensRole::Wide, "2", LensFacing::Back),
        screen(1_000, LensFacing::Back, 0),
    );
    rig.tick(0, &[]);
    rig.screen = screen(590, LensFacing::Back, 1);
    // Кольцо целиком обновилось за один тик: между старым и новым снимком — дыра, нужного кадра в нём нет.
    rig.frames = vec![
        live_frame(1_900, "2", PreviewMirror::None),
        live_frame(1_916, "2", PreviewMirror::None),
    ];
    rig.tick(100, &[switched(LensRole::Wide, LensRole::UltraWide)]);
    run_to_record(&mut rig);

    let record = &rig.runner.records()[0];
    assert!(record.ring_overran);
    assert_eq!(record.first_live_frame_ms, None, "кадр не измерен, а не «нулевой»");
    assert_eq!(record.mirror, MirrorCheck::NoFrameObserved);
}

#[test]
fn unfinished_names_list_the_running_step_and_the_pending_ones() {
    let mut rig = Rig::new(
        vec![
            step("01_tap_2", ZoomAction::TapStop { ratio: 2.0 }),
            step("02_tap_1", ZoomAction::TapStop { ratio: 1.0 }),
        ],
        engine(LensRole::Wide, "2", LensFacing::Back),
        screen(1_000, LensFacing::Back, 0),
    );
    assert_eq!(rig.runner.unfinished_names(), vec!["01_tap_2", "02_tap_1"]);
    rig.tick(0, &[]);
    assert_eq!(
        rig.runner.unfinished_names(),
        vec!["01_tap_2", "02_tap_1"],
        "идущий шаг ещё не закончен"
    );
}

#[test]
fn after_the_last_step_the_runner_reports_all_done_and_stays_idle() {
    let mut rig = Rig::new(
        vec![step("01_tap_2", ZoomAction::TapStop { ratio: 2.0 })],
        engine(LensRole::Wide, "2", LensFacing::Back),
        screen(1_000, LensFacing::Back, 0),
    );
    rig.tick(0, &[]);
    rig.screen = screen(2_000, LensFacing::Back, 1);
    run_to_record(&mut rig);

    let done = rig.tick(16, &[]);
    assert_eq!(done.state, RunnerState::AllDone);
    assert!(done.injections.is_empty());
    assert!(rig.runner.unfinished_names().is_empty());
}

#[test]
fn a_drag_step_waits_for_the_inertia_to_stop_and_the_wheel_to_close_before_it_is_recorded() {
    let mut rig = Rig::new(
        vec![step("09_wheel_drag", ZoomAction::WheelDrag)],
        engine(LensRole::Wide, "2", LensFacing::Back),
        screen(1_000, LensFacing::Back, 0),
    );
    rig.tick(0, &[]);
    // Колесо открыто и катится по инерции: до конца протяжки и после.
    rig.screen = ScreenView {
        zoom: ZoomView {
            wheel_open: true,
            ..rig.screen.zoom
        },
        lens_published: 5,
        ..rig.screen
    };
    for _ in 0..80 {
        rig.tick(16, &[]);
    }
    assert!(
        rig.runner.records().is_empty(),
        "колесо открыто — шаг не записан, тап следующего шага перебила бы инерция"
    );

    rig.screen = ScreenView {
        zoom: ZoomView {
            wheel_open: false,
            ratio_milli: 7_000,
            ..rig.screen.zoom
        },
        ..rig.screen
    };
    for _ in 0..40 {
        rig.tick(16, &[]);
    }
    assert_eq!(rig.runner.records().len(), 1, "колесо ушло — шаг записан");
    assert_eq!(rig.runner.records()[0].screen_ratio, 7.0);
}

#[test]
fn a_tap_step_waits_while_the_screen_is_still_gliding_to_the_stop() {
    let mut rig = Rig::new(
        vec![step("03_tap_2", ZoomAction::TapStop { ratio: 2.0 })],
        engine(LensRole::Wide, "2", LensFacing::Back),
        screen(1_000, LensFacing::Back, 0),
    );
    rig.tick(0, &[]);
    rig.screen = ScreenView {
        zoom: ZoomView {
            ratio_milli: 1_400,
            gliding: true,
            ..rig.screen.zoom
        },
        lens_published: 3,
        ..rig.screen
    };
    for _ in 0..30 {
        rig.tick(100, &[]);
    }
    assert!(
        rig.runner.records().is_empty(),
        "переход ещё идёт: судить значение рано"
    );

    rig.screen = ScreenView {
        zoom: ZoomView {
            ratio_milli: 2_000,
            gliding: false,
            ..rig.screen.zoom
        },
        ..rig.screen
    };
    rig.tick(100, &[]);
    assert_eq!(rig.runner.records().len(), 1);
    assert_eq!(rig.runner.records()[0].failure, None);
}

#[path = "zoom_runner_home_tests.rs"]
mod home;

#[test]
fn the_step_record_tells_what_the_screen_showed_and_which_continuity_counters_moved() {
    let mut rig = Rig::new(
        vec![step("01_tap_0.6", ZoomAction::TapStop { ratio: 0.6 })],
        engine(LensRole::Wide, "2", LensFacing::Back),
        screen(1_000, LensFacing::Back, 0),
    );
    rig.tick(0, &[]);
    rig.screen = screen(590, LensFacing::Back, 1);
    // После действия экран 300 мс был чёрным «нет картинки», потом пошли живые кадры новой камеры.
    rig.frames = vec![
        FrameRecord {
            present_time: at_ms(rig.elapsed_ms + 16),
            label: FrameLabel::Black(BlackKind::NoPicture),
            backdrop: BackdropContinuity::Live,
            crop: None,
            held_magnification: 1.0,
        },
        live_frame(rig.elapsed_ms + 316, "3", PreviewMirror::None),
    ];
    rig.counters.black_frames_before_first_picture = 1;
    rig.counters.live_frames_presented = 1;
    rig.isp_counters.frames_hidden_cold_start = 2;
    rig.tick(100, &[switched(LensRole::Wide, LensRole::UltraWide)]);
    run_to_record(&mut rig);

    let record = &rig.runner.records()[0];
    assert_eq!(record.continuity.black_frames_before_first_picture, 1);
    assert_eq!(record.continuity.live_frames_presented, 1);
    assert_eq!(record.continuity.frames_hidden_cold_start, 2);
    assert_eq!(record.held_frames, 0);
    let first_black = record
        .presented_frames
        .iter()
        .find(|bucket| bucket.label == "black_no_picture");
    assert_eq!(
        first_black.map(|bucket| (bucket.camera.clone(), bucket.frames)),
        Some((None, 1))
    );
    assert!(
        record
            .presented_frames
            .iter()
            .any(|bucket| bucket.label == "live" && bucket.camera.as_deref() == Some("3"))
    );
}

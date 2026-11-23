//! Сквозные тесты экрана камеры (сессия 12): настоящий маршрутизатор на fake-оболочке, настоящий движок камеры на
//! fake-бэкенде, настоящий `CameraScreen`. Подставлен только поток рендера: [`RecordingRenderControl`] хранит
//! установленный источник UI, а тест сам «рисует кадры» им, как рисовал бы поток рендера.
//!
//! Что доказывают тесты: касание из очереди оболочки доходит до команды ДВИЖКУ камеры с верной точкой сенсора;
//! клавиша громкости доходит до сервиса снимка, а итог возвращается в экран; экран снимается на время сценария и
//! возвращается свежим (в том числе после `glass`); без GPU экрана нет, итог явный; переполнение канала не молчит.

use camera_contract::ids::{CameraTarget, LensRole};
use camera_contract::raw_format::RawFormat;
use camera_contract::stream::{ReadoutAspect, StreamMode, TargetFrameRate};
use camera_contract::three_a::PointOfInterest;
use camera_screen::settings::AspectRatio;
use camera_screen::shutter::ThumbnailState;
use camera_screen::{LayoutConfig, ScreenLayout};
use render_loop::viewfinder::{FramingDestination, ViewfinderCommand};
use shell_contract::clock::MonotonicNanos;
use shell_contract::hardware_key::{HardwareKey, HardwareKeyEvent, KeyPhase, KeyRepeat};
use shell_contract::touch::{PointerId, SurfacePoint, TouchPhase, TouchSample};
use ui_draw::draw_list::DrawList;
use ui_draw::frame_source::{UiFrameError, UiFrameRequest, UiFrameSource};
use ui_draw::geometry::PxSize;

#[path = "event_router_camera_screen_still_tests.rs"]
mod real_still;
#[path = "event_router_camera_screen_thumbnail_tests.rs"]
mod real_thumbnail;
#[path = "event_router_camera_screen_text_tests.rs"]
mod text_dialog;

use super::*;
use crate::camera_config::CameraConfig;
use crate::camera_screen_config::CameraScreenConfig;
use crate::camera_screen_service::{CameraScreenPolicy, CameraScreenService, CameraScreenStatus};
use crate::camera_test_support::WAIT;
use crate::gpu_test_support::RecordedUiOverlay;
use crate::scenario::observing::{ObservedEvent, ObservingScenario, ScenarioContext, ScenarioStep};
use crate::viewfinder_control::{RenderAdjustment, UnavailableRenderControl};

const SURFACE_WIDTH_PX: u32 = 1440;
const SURFACE_HEIGHT_PX: u32 = 3216;
/// 1 pt = ширина / 440 px (решение 82).
const PX_PER_PT: f64 = SURFACE_WIDTH_PX as f64 / 440.0;
/// Период кадра рендера в тестах, мс.
const FRAME_MS: i64 = 16;

/// Экран, включённый, с мгновенным ответом фейк-снимка (детерминированные тесты).
fn instant_capture_config() -> CameraScreenConfig {
    CameraScreenConfig {
        fake_capture_delay_ms: 0,
        // Отчёт каждый кадр: тесты читают его сразу после кадра (прореживание проверяет отдельный тест).
        report_min_interval_ms: 0,
        ..CameraScreenConfig::default()
    }
}

/// Маршрутизатор с движком на fake-бэкенде и поставленным экраном.
fn screen_harness(config: CameraScreenConfig) -> Harness {
    let mut harness = harness_for_rig(crate::camera_test_support::FakeCameraRig::with_config(
        CameraConfig::default(),
    ));
    harness.router.camera_screen = CameraScreenService::new(config);
    harness.router.install_camera_screen();
    harness
}

/// Запускает поток основной камеры 4:3 и ждёт `Streaming`.
fn start_streaming(harness: &mut Harness) {
    // Android отдаёт камеру только приложению на переднем плане.
    harness.deliver(ShellEvent::Resumed);
    let handle = harness.rig().handle();
    handle.open(CameraTarget::Role(LensRole::Wide)).expect("open");
    handle
        .configure(StreamMode {
            raw_format: RawFormat::Raw10,
            readout: ReadoutAspect::FourByThree,
            frame_rate: TargetFrameRate::new(30).expect("30 к/с"),
        })
        .expect("configure");
    handle
        .start_streaming(camera_contract::control::CaptureIntent::Preview)
        .expect("start");
    harness.rig().wait_streaming();
}

/// Часы потока рендера: каждый кадр на 16 мс позже предыдущего.
struct RenderClock {
    now_ms: i64,
}

impl RenderClock {
    fn new() -> Self {
        Self { now_ms: 0 }
    }

    fn time(&self) -> MonotonicNanos {
        MonotonicNanos::from_nanos(self.now_ms * 1_000_000)
    }

    /// Один кадр рендера установленным источником UI.
    fn frame(&mut self, harness: &Harness) {
        let _list = self.frame_list(harness);
    }

    /// Один кадр, и список рисования, который экран отдал рендеру (для проверок «что нарисовано»).
    fn frame_list(&mut self, harness: &Harness) -> DrawList {
        self.now_ms += FRAME_MS;
        let request = UiFrameRequest {
            present_time: self.time(),
            logical_size: PxSize::new(SURFACE_WIDTH_PX as f32, SURFACE_HEIGHT_PX as f32),
            px_per_pt: PX_PER_PT as f32,
            backdrop: ui_draw::frame_source::BackdropContinuity::Live,
        };
        let produced = harness
            .control
            .produce_ui_frame(&request)
            .expect("источник UI установлен");
        produced.expect("кадр экрана построен")
    }

    fn frames(&mut self, harness: &Harness, count: usize) {
        for _ in 0..count {
            self.frame(harness);
        }
    }
}

fn touch(harness: &mut Harness, phase: TouchPhase, x_pt: f64, y_pt: f64, at_ms: i64) {
    harness.deliver(ShellEvent::Touch(TouchSample {
        pointer: PointerId::new(0),
        phase,
        position: SurfacePoint {
            x_px: (x_pt * PX_PER_PT) as f32,
            y_px: (y_pt * PX_PER_PT) as f32,
        },
        time: MonotonicNanos::from_nanos(at_ms * 1_000_000),
    }));
}

fn volume_key(harness: &mut Harness, phase: KeyPhase, repeat: KeyRepeat, at_ms: i64) {
    harness.deliver(ShellEvent::HardwareKey(HardwareKeyEvent {
        key: HardwareKey::VolumeDown,
        phase,
        repeat,
        time: MonotonicNanos::from_nanos(at_ms * 1_000_000),
    }));
}

/// Точка кадра 4:3 на экране в pt: доли `(u, v)` внутри прямоугольника кадра по раскладке из конфига.
fn frame_point_pt(u: f64, v: f64) -> (f64, f64) {
    let layout = ScreenLayout::new(
        &LayoutConfig::default(),
        shell_contract::surface::SurfaceSize {
            width_px: SURFACE_WIDTH_PX,
            height_px: SURFACE_HEIGHT_PX,
        },
    )
    .expect("раскладка");
    let rect = layout.frame_rect(AspectRatio::FourByThree);
    (rect.min_x() + u * rect.size.width, rect.min_y() + v * rect.size.height)
}

/// Установленный экран, камера стримит, два кадра отрисованы (сцена построена, факты Live доставлены).
fn ready_screen() -> (Harness, RenderClock) {
    let mut harness = screen_harness(instant_capture_config());
    start_streaming(&mut harness);
    let mut clock = RenderClock::new();
    clock.frames(&harness, 2);
    (harness, clock)
}

#[test]
fn a_tap_on_the_viewfinder_reaches_the_camera_engine_with_the_right_sensor_point() {
    let (mut harness, mut clock) = ready_screen();
    assert_eq!(
        harness.control.recorded_ui_overlay(),
        vec![RecordedUiOverlay::Install("camera_screen")]
    );

    // Тап в точке (u = 0,25; v = 0,75) кадра 4:3. Сенсор 90°: экран (u, v) → сенсор (v, 1 − u) = (0,75; 0,75);
    // кадрирование 4:3 по полному буферу 4096×3072 и активная область такого же размера — доли не меняются.
    let (x_pt, y_pt) = frame_point_pt(0.25, 0.75);
    let start_ms = clock.now_ms;
    touch(&mut harness, TouchPhase::Began, x_pt, y_pt, start_ms);
    clock.frame(&harness);
    touch(&mut harness, TouchPhase::Ended, x_pt, y_pt, start_ms + 50);
    clock.frames(&harness, 30);

    let status = harness
        .rig()
        .handle()
        .wait_until_status(WAIT, |status| {
            matches!(status.three_a.point, PointOfInterest::Point { .. })
        })
        .expect("команда экрана дошла до движка: точка интереса появилась в статусе");
    let PointOfInterest::Point { region, .. } = status.three_a.point else {
        unreachable!("условие ожидания гарантирует точку");
    };
    assert!(
        (region.center().x() - 0.75).abs() < 0.002 && (region.center().y() - 0.75).abs() < 0.002,
        "точка сенсора {:?}",
        (region.center().x(), region.center().y())
    );
    let report = harness.router.camera_screen().report().expect("отчёт экрана");
    assert_eq!(
        report.effects.camera.sent, 1,
        "ровно одно намерение камере: {:?}",
        report.effects
    );
    assert_eq!(report.diagnostics.gestures.taps, 1);
    assert_eq!(harness.router.camera_screen().input_stats().overflowed_touches, 0);
}

#[test]
fn the_screen_places_the_viewfinder_by_its_own_layout_through_render_control() {
    let (harness, _clock) = ready_screen();

    let framings: Vec<_> = harness
        .control
        .recorded()
        .into_iter()
        .filter_map(|adjustment| match adjustment {
            RenderAdjustment::Viewfinder(ViewfinderCommand::SetFraming(framing)) => Some(framing),
            _ => None,
        })
        .collect();

    assert!(
        !framings.is_empty(),
        "экран положил место видоискателя командой SetFraming"
    );
    let FramingDestination::ScreenRect(rect) = framings[0].destination else {
        panic!(
            "место кадра должно быть прямоугольником экрана: {:?}",
            framings[0].destination
        );
    };
    let (left_pt, top_pt) = frame_point_pt(0.0, 0.0);
    let (right_pt, bottom_pt) = frame_point_pt(1.0, 1.0);
    let screen_width_pt = f64::from(SURFACE_WIDTH_PX) / PX_PER_PT;
    let screen_height_pt = f64::from(SURFACE_HEIGHT_PX) / PX_PER_PT;
    assert!((f64::from(rect.left()) - left_pt / screen_width_pt).abs() < 1e-4);
    assert!((f64::from(rect.top()) - top_pt / screen_height_pt).abs() < 1e-4);
    assert!((f64::from(rect.width()) - (right_pt - left_pt) / screen_width_pt).abs() < 1e-4);
    assert!((f64::from(rect.height()) - (bottom_pt - top_pt) / screen_height_pt).abs() < 1e-4);
    assert_eq!(
        harness
            .router
            .camera_screen()
            .report()
            .expect("отчёт")
            .effects
            .framing
            .delivered,
        framings.len() as u64
    );
}

#[test]
fn the_volume_key_takes_a_shot_and_the_result_comes_back_to_the_screen() {
    let (mut harness, mut clock) = ready_screen();
    assert_eq!(
        harness.router.camera_screen().report().expect("отчёт").thumbnail,
        ThumbnailState::Empty
    );

    volume_key(&mut harness, KeyPhase::Pressed, KeyRepeat::Initial, clock.now_ms);
    clock.frames(&harness, 3);
    volume_key(&mut harness, KeyPhase::Released, KeyRepeat::Initial, clock.now_ms);
    clock.frames(&harness, 3);

    let journal = harness.router.camera_screen().capture_journal();
    assert_eq!(
        journal.len(),
        1,
        "одно нажатие — один запрос снимка (отпускание его не повторяет): {journal:?}"
    );
    let report = harness.router.camera_screen().report().expect("отчёт");
    assert_eq!(
        report.thumbnail,
        ThumbnailState::Last {
            shot: journal[0].request,
            // Фейк-сервис отвечает без картинки: в круге серый диск (14g, осознанно).
            picture: None,
        },
        "CaptureFinished(Saved) вернулся в экран: миниатюра указывает на этот снимок"
    );
    assert_eq!(report.effects.capture.accepted, 1);
    assert_eq!(
        harness
            .router
            .camera_screen()
            .capture_diagnostics()
            .fake
            .expect("в этом тесте стоит фейк")
            .counters
            .completions_delivered,
        1
    );
}

#[test]
fn auto_repeat_of_the_volume_key_does_not_take_extra_shots() {
    let (mut harness, mut clock) = ready_screen();

    volume_key(&mut harness, KeyPhase::Pressed, KeyRepeat::Initial, clock.now_ms);
    for count in 1..=4 {
        volume_key(
            &mut harness,
            KeyPhase::Pressed,
            KeyRepeat::AutoRepeat { count },
            clock.now_ms,
        );
    }
    clock.frames(&harness, 5);

    assert_eq!(harness.router.camera_screen().capture_journal().len(), 1);
}

#[test]
fn camera_facts_and_orientation_and_gravity_reach_the_screen() {
    let (mut harness, mut clock) = ready_screen();
    let report = harness.router.camera_screen().report().expect("отчёт");
    assert_eq!(
        report.presence,
        camera_screen::CameraPresence::Live,
        "факты Live из статуса движка"
    );
    let facts_after_start = report.facts_sent;
    assert!(facts_after_start >= 1);

    // Без изменений в камере факты экрану не шлются.
    clock.frames(&harness, 5);
    assert_eq!(
        harness.router.camera_screen().report().expect("отчёт").facts_sent,
        facts_after_start
    );

    harness.deliver(ShellEvent::DeviceOrientation(DeviceOrientation::LandscapeLeft));
    harness.shell.gravity.publish(shell_contract::gravity::GravitySample {
        time: shell_contract::clock::BoottimeNanos::from_nanos(1),
        vector: shell_contract::gravity::GravityVector {
            x: 0.0,
            y: 9.81,
            z: 0.0,
        },
    });
    clock.frames(&harness, 8);

    let report = harness.router.camera_screen().report().expect("отчёт");
    assert_ne!(
        report.icon_rotation_clockwise_degrees, 0.0,
        "значки повёрнуты по ориентации"
    );
    assert!(report.gravity_samples_sent >= 1, "гравитация дошла: {report:?}");
}

#[test]
fn channel_overflow_is_counted_and_resets_gestures_with_a_gap() {
    let config = CameraScreenConfig {
        input_queue_capacity: 4,
        ..instant_capture_config()
    };
    let mut harness = screen_harness(config);
    start_streaming(&mut harness);
    let mut clock = RenderClock::new();
    clock.frames(&harness, 2);
    let (x_pt, y_pt) = frame_point_pt(0.5, 0.5);

    // Поток рендера «завис»: 12 касаний при очереди на 4.
    for index in 0..12 {
        let phase = if index == 0 {
            TouchPhase::Began
        } else {
            TouchPhase::Moved
        };
        touch(&mut harness, phase, x_pt, y_pt, clock.now_ms + index);
    }
    let stats = harness.router.camera_screen().input_stats();
    assert_eq!(stats.queued_touches, 4);
    assert_eq!(stats.overflowed_touches, 8, "потеря не молчит: {stats:?}");

    // Рендер ожил и разгрёб очередь; следующее касание сначала приносит разрыв потока.
    clock.frame(&harness);
    touch(&mut harness, TouchPhase::Began, x_pt, y_pt, clock.now_ms);
    clock.frame(&harness);

    let stats = harness.router.camera_screen().input_stats();
    assert_eq!(stats.gaps_queued, 1);
    let diagnostics = harness.router.camera_screen().report().expect("отчёт").diagnostics;
    assert_eq!(
        diagnostics.touch_gaps, 1,
        "экран получил разрыв и сбросил жесты: {diagnostics:?}"
    );
}

#[test]
fn without_a_gpu_there_is_no_screen_and_the_outcome_is_explicit() {
    let mut harness = harness_for_rig(crate::camera_test_support::FakeCameraRig::with_config(
        CameraConfig::default(),
    ));
    harness.router.gpu = crate::gpu_services::GpuServices::new(
        std::sync::Arc::clone(&harness.render) as _,
        crate::gpu_test_support::FakeGpuDiagnostics::new(Vec::new(), None),
        render_loop::config::RenderLoopConfig::default(),
        std::sync::Arc::new(UnavailableRenderControl::new("GPU не поднялся")),
    );
    harness.router.camera_screen = CameraScreenService::new(instant_capture_config());

    harness.router.install_camera_screen();
    touch(&mut harness, TouchPhase::Began, 100.0, 300.0, 1);
    volume_key(&mut harness, KeyPhase::Pressed, KeyRepeat::Initial, 2);

    assert_eq!(
        *harness.router.camera_screen().status(),
        CameraScreenStatus::RenderUnavailable {
            reason: "GPU не поднялся".to_owned()
        }
    );
    let dropped = harness.router.camera_screen().dropped_input();
    assert_eq!(
        (dropped.touches, dropped.actions),
        (1, 1),
        "вход отброшен со счётчиком, не молча"
    );
    assert!(harness.router.camera_screen().report().is_none());
}

#[test]
fn a_disabled_screen_is_never_installed_and_drops_input_with_a_counter() {
    let mut harness = screen_harness(CameraScreenConfig {
        enabled: false,
        ..instant_capture_config()
    });

    touch(&mut harness, TouchPhase::Began, 100.0, 300.0, 1);

    assert_eq!(*harness.router.camera_screen().status(), CameraScreenStatus::Disabled);
    assert!(harness.control.recorded_ui_overlay().is_empty(), "команд рендеру нет");
    assert_eq!(harness.router.camera_screen().dropped_input().touches, 1);
}

/// Сценарий-проба: ничего не делает, пока тест не скажет закончить; политика экрана — по параметру.
struct ProbeScenario {
    policy: CameraScreenPolicy,
    finish: Arc<Mutex<bool>>,
    /// Ставит ли проба свой источник UI (как `glass`) и меняет ли рисовальщик.
    installs_own_ui: bool,
}

/// Источник UI пробы, имитирующий демо-сцену `glass`.
struct OwnUi;

impl UiFrameSource for OwnUi {
    fn name(&self) -> &'static str {
        "probe_ui"
    }

    fn produce_frame(&mut self, _request: &UiFrameRequest) -> Result<DrawList, UiFrameError> {
        Err(UiFrameError::SourceFailed {
            reason: "источник пробы ничего не рисует".to_owned(),
        })
    }
}

impl ObservingScenario for ProbeScenario {
    fn name(&self) -> &'static str {
        "screen-probe"
    }

    fn camera_screen_policy(&self) -> CameraScreenPolicy {
        self.policy
    }

    fn start(&mut self, context: &mut ScenarioContext<'_>) -> ScenarioStep {
        if self.installs_own_ui {
            context.gpu.install_ui_overlay_for_scenario(Box::new(OwnUi));
            context
                .gpu
                .set_viewfinder_painter_for_scenario(render_loop::viewfinder::ViewfinderPainter::Isp);
        }
        ScenarioStep::Continue
    }

    fn on_event(&mut self, _event: ObservedEvent, _context: &mut ScenarioContext<'_>) -> ScenarioStep {
        ScenarioStep::Continue
    }

    fn on_tick(&mut self, _context: &mut ScenarioContext<'_>) -> ScenarioStep {
        if *self.finish.lock().expect("замок") {
            return ScenarioStep::Finished(crate::scenario::ScenarioOutcome::Passed {
                report: serde_json::json!({}),
            });
        }
        ScenarioStep::Continue
    }
}

fn start_probe(harness: &mut Harness, policy: CameraScreenPolicy, installs_own_ui: bool) -> Arc<Mutex<bool>> {
    let finish = Arc::new(Mutex::new(false));
    harness
        .starter
        .start(Box::new(ProbeScenario {
            policy,
            finish: Arc::clone(&finish),
            installs_own_ui,
        }))
        .expect("старт");
    harness.router.step(Duration::from_millis(1));
    finish
}

fn finish_probe(harness: &mut Harness, finish: &Arc<Mutex<bool>>) {
    *finish.lock().expect("замок") = true;
    harness.router.step(Duration::from_millis(1));
    assert_eq!(
        harness.output.results.lock().expect("замок").len(),
        1,
        "сценарий закончился"
    );
}

#[test]
fn the_screen_is_removed_for_a_scenario_and_comes_back_fresh_after_it() {
    let (mut harness, mut clock) = ready_screen();
    let frames_before = harness
        .router
        .camera_screen()
        .report()
        .expect("отчёт")
        .diagnostics
        .frames_produced;
    assert!(frames_before >= 2);

    let finish = start_probe(&mut harness, CameraScreenPolicy::SuspendForScenario, false);
    assert_eq!(
        *harness.router.camera_screen().status(),
        CameraScreenStatus::SuspendedForScenario {
            scenario: "screen-probe"
        }
    );
    assert!(
        harness.control.produce_ui_frame(&dummy_request()).is_none(),
        "источника на время сценария нет"
    );
    // Вход во время сценария отбрасывается со счётчиком, а не копится для следующего экрана.
    touch(&mut harness, TouchPhase::Began, 100.0, 300.0, 1);
    assert_eq!(harness.router.camera_screen().dropped_input().touches, 1);

    finish_probe(&mut harness, &finish);

    assert_eq!(*harness.router.camera_screen().status(), CameraScreenStatus::Installed);
    assert_eq!(harness.router.camera_screen().installs(), 2);
    assert_eq!(
        harness.control.recorded_ui_overlay(),
        vec![
            RecordedUiOverlay::Install("camera_screen"),
            RecordedUiOverlay::Remove,
            RecordedUiOverlay::Install("camera_screen"),
        ]
    );
    clock.frame(&harness);
    let fresh = harness.router.camera_screen().report().expect("отчёт нового экрана");
    assert_eq!(
        fresh.diagnostics.frames_produced, 1,
        "экран свежий: состояние между сценариями не сохраняется"
    );
}

#[test]
fn a_scenario_that_installs_its_own_ui_gets_the_screen_back_after_it_ends() {
    let (mut harness, _clock) = ready_screen();

    let finish = start_probe(&mut harness, CameraScreenPolicy::SuspendForScenario, true);
    assert_eq!(harness.control.installed_ui_source_name(), Some("probe_ui"));
    finish_probe(&mut harness, &finish);

    assert_eq!(
        harness.control.recorded_ui_overlay(),
        vec![
            RecordedUiOverlay::Install("camera_screen"),
            RecordedUiOverlay::Remove,
            RecordedUiOverlay::Install("probe_ui"),
            RecordedUiOverlay::Remove,
            RecordedUiOverlay::Install("camera_screen"),
        ],
        "источник сценария снят возвратом рендера, затем поставлен свежий экран"
    );
    assert_eq!(harness.control.installed_ui_source_name(), Some("camera_screen"));
    // Возврат рендера откатил только то, что тронул сценарий (рисовальщик), и не откатил место видоискателя от экрана.
    let framing_resets = harness
        .control
        .recorded()
        .into_iter()
        .filter(|adjustment| {
            matches!(
                adjustment,
                RenderAdjustment::Viewfinder(ViewfinderCommand::SetFraming(framing))
                    if framing.destination == FramingDestination::Configured
            )
        })
        .count();
    assert_eq!(
        framing_resets, 0,
        "сценарий не откатывает место кадра, заданное экраном"
    );
}

#[test]
fn a_scenario_can_ask_to_keep_the_screen_running() {
    let (mut harness, _clock) = ready_screen();

    let finish = start_probe(&mut harness, CameraScreenPolicy::KeepRunning, false);

    assert_eq!(*harness.router.camera_screen().status(), CameraScreenStatus::Installed);
    touch(&mut harness, TouchPhase::Began, 100.0, 300.0, 1);
    assert_eq!(
        harness.router.camera_screen().dropped_input().touches,
        0,
        "экран продолжает получать вход"
    );
    finish_probe(&mut harness, &finish);
    assert_eq!(harness.router.camera_screen().installs(), 1, "экран не пересоздавался");
    assert_eq!(
        harness.control.recorded_ui_overlay(),
        vec![RecordedUiOverlay::Install("camera_screen")]
    );
}

fn dummy_request() -> UiFrameRequest {
    UiFrameRequest {
        present_time: MonotonicNanos::from_nanos(0),
        logical_size: PxSize::new(SURFACE_WIDTH_PX as f32, SURFACE_HEIGHT_PX as f32),
        px_per_pt: PX_PER_PT as f32,
        backdrop: ui_draw::frame_source::BackdropContinuity::Live,
    }
}

// Восстановление экрана после снятия рендером, долг разрыва по тику и прореживание отчёта (ревью P6e).
#[path = "event_router_camera_screen_recovery_tests.rs"]
mod recovery;
#[path = "event_router_camera_screen_system_gesture_tests.rs"]
mod system_gesture;

/// Дефект 12b: в продукте датчик ориентации не запускал никто (только отладочный `motion`), и значки экрана не
/// поворачивались вслед за телефоном. Теперь датчик держит установленный экран: стоит экран — датчик работает;
/// сценарий снял экран — датчик свободен (его может занять сам сценарий); экран вернулся — датчик снова запущен.
#[test]
fn the_installed_screen_keeps_the_orientation_sensor_running_and_releases_it_for_a_scenario() {
    let (mut harness, _clock) = ready_screen();
    assert_eq!(
        harness.shell.orientation_sensor.alive(),
        1,
        "экран стоит — датчик ориентации работает"
    );
    assert_eq!(harness.shell.orientation_sensor.starts(), 1);

    let finish = start_probe(&mut harness, CameraScreenPolicy::SuspendForScenario, false);
    assert_eq!(
        harness.shell.orientation_sensor.alive(),
        0,
        "экран снят на время сценария — датчик отпущен"
    );

    finish_probe(&mut harness, &finish);
    assert_eq!(*harness.router.camera_screen().status(), CameraScreenStatus::Installed);
    assert_eq!(
        harness.shell.orientation_sensor.alive(),
        1,
        "свежий экран снова держит датчик"
    );
    assert_eq!(harness.shell.orientation_sensor.starts(), 2);
}

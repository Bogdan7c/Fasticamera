//! Функциональные тесты сценария `3a` (сессия 08): настоящий маршрутизатор, настоящий движок камеры на
//! fake-платформе, подставной HAL 3A ([`SimulatedHal`]: единственный источник результатов съёмки) и
//! подставной рендер ([`ThreeAMirror`]: счётчики AWB и пропуск кадров серии). Каждый тест доходит до
//! строки результата, а не проверяет отдельный шаг.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, PoisonError};
use std::thread::{self, JoinHandle};

use camera_contract::auto_exposure::{ExposureAuthority, MeteredArea};
use camera_contract::control::ManualControl;
use camera_contract::three_a::AeAfLock;
use camera_core::backend::{BackendFailure, RequestFailure};
use camera_core::three_a_status::FocusProgress;
use isp_vulkan::viewfinder::IspStageSet;
use render_loop::config::WhiteBalanceConfig;
use render_loop::viewfinder::{
    AutoExposureCounters, AutoExposureDiagnostics, HeadroomBenchDiagnostics, ViewfinderColorSpace, ViewfinderCounters,
    ViewfinderDiagnostics, ViewfinderExtent, ViewfinderGpuTimes, ViewfinderPainter, ViewfinderStorage,
    WhiteBalanceDiagnostics, WhiteDelta, WhiteReading,
};

use super::*;
use crate::auto_exposure_link::ExposureControllerKind;
use crate::camera_config::CameraConfig;
use crate::camera_scenario_config::CameraScenarioConfig;
use crate::camera_test_support::WAIT;
use crate::fake_render_ae::{DEFAULT_SCENE_SECONDS_ISO, FakeRenderAe, TapPointScene};
use crate::raw_stream_test_support::{PumpFaults, StreamPump};
use crate::three_a_scenario_config::ThreeAScenarioConfig;
use crate::three_a_test_hal::{HalFaults, HalStats, SimulatedHal};
use crate::viewfinder_scenario_config::ViewfinderScenarioConfig;

/// Сколько кадров рендер прячет на холодном старте AWB (оценка готова на 3-м кадре, решение 65).
const HIDDEN_AT_START: u64 = 3;

/// Подставной поток рендера: счётчики видоискателя по числу показанных кадров камеры и по кадрам серии,
/// которые HAL выдал (как настоящий рендер, он возвращает их камере без показа).
struct ThreeAMirror {
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

/// Диагностика нашего AE рендера: из неё сценарий берёт сырой замер сцены.
fn ae_diagnostics(scene_seconds_iso: f64) -> AutoExposureDiagnostics {
    AutoExposureDiagnostics {
        directive_generation: None,
        authority: ExposureAuthority::Automatic,
        metered_area: MeteredArea::WholeFrame,
        target_log2_seconds_iso: None,
        observed_scene_log2_seconds_iso: Some(scene_seconds_iso.log2()),
        display_gain_ev: 0.0,
        displayed_frame_log2_seconds_iso: None,
        convergence: None,
        quality: None,
        last_proposal: None,
        counters: AutoExposureCounters::default(),
    }
}

fn viewfinder_diagnostics() -> ViewfinderDiagnostics {
    ViewfinderDiagnostics {
        painter: ViewfinderPainter::Isp,
        // Место видоискателя — из конфига рендера (решение 93: экран камеры задаёт его командой `SetFraming`).
        placement: render_loop::viewfinder::EffectivePlacement::Configured(
            render_loop::config::ViewfinderPlacement::default(),
        ),
        stages: IspStageSet::ALL,
        look: std::sync::Arc::new(look_contract::LookSettings::NEUTRAL),
        display_color_space: Some(ViewfinderColorSpace::DisplayP3),
        storage: Some(ViewfinderStorage::TenBitUnorm),
        output_extent: Some(ViewfinderExtent {
            width: 1440,
            height: 1080,
        }),
        gpu_time: ViewfinderGpuTimes::default(),
        counters: ViewfinderCounters::default(),
        headroom: HeadroomBenchDiagnostics::default(),
        white_balance: WhiteBalanceDiagnostics::idle(WhiteBalanceConfig::default().statistics_every_n_frames),
        auto_exposure: None,
        ui_overlay: render_loop::ui_overlay::UiOverlayDiagnostics::default(),
        continuity: Default::default(),
    }
}

impl ThreeAMirror {
    /// `scene` — сцена подставного AE рендера (с·ISO): если она есть, рендер отдаёт её как сырой замер сцены.
    fn start(harness: &Harness, hal_stats: Arc<HalStats>, scene: Option<Arc<Mutex<f64>>>) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let render = Arc::clone(&harness.render);
        let join = thread::spawn(move || {
            while !thread_stop.load(Ordering::SeqCst) {
                let skipped = hal_stats.burst_frames_emitted.load(Ordering::SeqCst);
                render.update(|status| {
                    let presented = status
                        .backend
                        .camera
                        .as_ref()
                        .map_or(0, |camera| camera.camera_frames_presented);
                    let viewfinder = status.backend.viewfinder.get_or_insert_with(viewfinder_diagnostics);
                    let hidden = if presented > 0 { HIDDEN_AT_START } else { 0 };
                    viewfinder.auto_exposure = scene
                        .as_ref()
                        .map(|scene| ae_diagnostics(*scene.lock().unwrap_or_else(PoisonError::into_inner)));
                    viewfinder.counters.frames_processed = presented + hidden;
                    viewfinder.counters.frames_hidden_cold_start = hidden;
                    viewfinder.counters.bracket_frames_skipped = skipped;
                    let period = u64::from(viewfinder.white_balance.statistics_every_n_frames.max(1));
                    viewfinder.counters.statistics_published = (presented + hidden) / period;
                    let white_balance = &mut viewfinder.white_balance;
                    white_balance.estimates = (presented + hidden) / period;
                    white_balance.hidden_frames_cold_start = hidden;
                    white_balance.scene_white = Some(WhiteReading {
                        temperature_kelvin: 5_200.0,
                        duv: 0.002,
                    });
                    white_balance.hal_delta = Some(WhiteDelta { mired: 4.0, duv: 0.001 });
                });
                thread::sleep(Duration::from_millis(1));
            }
        });
        Self { stop, join: Some(join) }
    }
}

impl Drop for ThreeAMirror {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

/// Короткие сроки всех шагов, **вендорский AE** (поведение сессии 08: маркеры `AE_STATE`). Тесты нашего AE —
/// на [`short_own_config`].
fn short_config(tune: impl FnOnce(&mut ThreeAScenarioConfig)) -> CameraConfig {
    CameraConfig {
        exposure_controller: ExposureControllerKind::Vendor,
        ..short_own_config(tune)
    }
}

/// Короткие сроки всех шагов: тест идёт секунды, а не минуты. Запуск видоискателя — как в тестах `viewfinder`.
/// Экспозицию ведёт наш AE (умолчание продукта); роль рендера играет `FakeRenderAe`.
fn short_own_config(tune: impl FnOnce(&mut ThreeAScenarioConfig)) -> CameraConfig {
    let viewfinder = ViewfinderScenarioConfig {
        streaming_timeout_ms: 2_000,
        render_live_timeout_ms: 1_000,
        viewfinder_settle_ms: 50,
        ..ViewfinderScenarioConfig::default()
    };
    let mut three_a = ThreeAScenarioConfig {
        live_results_timeout_ms: 500,
        ..ThreeAScenarioConfig::default()
    };
    three_a.awb.ready_timeout_ms = 500;
    three_a.tap.settle_timeout_ms = 1_500;
    three_a.manual.apply_timeout_ms = 800;
    three_a.ev.settle_ms = 100;
    three_a.ev.samples = 5;
    three_a.ev.collect_timeout_ms = 800;
    three_a.lock.settle_timeout_ms = 800;
    // Отсчёт «приготовься» короткий (2 шага по 100 мс), чтобы тест шёл секунды.
    three_a.lock.prepare_countdown_ms = 200;
    three_a.lock.prepare_step_ms = 100;
    three_a.lock.hold_ms = 200;
    three_a.lock.unlock_timeout_ms = 800;
    three_a.bracket.result_timeout_ms = 1_500;
    three_a.bracket.render_settle_ms = 100;
    tune(&mut three_a);
    let mut config = CameraConfig {
        scenarios: CameraScenarioConfig {
            viewfinder,
            three_a,
            ..CameraScenarioConfig::default()
        },
        ..CameraConfig::default()
    };
    // Движок не ждёт итога тапа дольше, чем сценарий (иначе `TimedOut` не успел бы прийти событием).
    config.engine.three_a.focus_settle_timeout_ms = 300;
    config
}

/// Сценарий `3a` до итога на здоровом потоке; `tune_rig` готовит стенд, `during` вызывается после каждого шага
/// маршрутизатора (тест может вмешаться в прогон).
fn run_three_a_with(
    config: CameraConfig,
    faults: HalFaults,
    tune_rig: impl FnOnce(&FakeCameraRig),
    during: impl FnMut(&Harness, Option<&FakeRenderAe>),
) -> (Harness, serde_json::Value) {
    let rig = FakeCameraRig::with_config(config);
    tune_rig(&rig);
    run_three_a_on(rig, faults, during)
}

/// То же на готовом стенде. Подставной рендер AE запускается, только если композиция поставила наш AE.
fn run_three_a_on(
    rig: FakeCameraRig,
    faults: HalFaults,
    mut during: impl FnMut(&Harness, Option<&FakeRenderAe>),
) -> (Harness, serde_json::Value) {
    let own_ae = rig.services.exposure_controller().effective == ExposureControllerKind::Own;
    let mut harness = harness_for_rig(rig);
    harness.deliver(ShellEvent::Resumed);
    let paced = PumpFaults {
        paced_at_sensor_rate: true,
        results_from_external_hal: true,
        ..PumpFaults::default()
    };
    let _pump = StreamPump::start(harness.rig(), Arc::clone(&harness.render), paced);
    let hal = SimulatedHal::start(harness.rig(), faults);
    // Наш AE: роль рендера играет подставной помощник (в тестах приложения настоящего рендера нет).
    let render_ae = (own_ae).then(|| FakeRenderAe::start(harness.rig()));
    let _mirror = ThreeAMirror::start(
        &harness,
        Arc::clone(&hal.stats),
        render_ae.as_ref().map(FakeRenderAe::scene_probe),
    );
    harness.start(Scenario::parse("3a").expect("сценарий зарегистрирован"));
    let deadline = Instant::now() + Duration::from_secs(30);
    while harness.output.results.lock().expect("замок").is_empty() && Instant::now() < deadline {
        harness.router.step(Duration::from_millis(2));
        during(&harness, render_ae.as_ref());
    }
    let result = harness.output.single_result();
    (harness, result)
}

fn run_three_a(faults: HalFaults) -> (Harness, serde_json::Value) {
    run_three_a_with(short_config(|_| {}), faults, |_| {}, |_, _| {})
}

fn error_of(result: &serde_json::Value) -> String {
    result["error"].as_str().unwrap_or_default().to_owned()
}

fn assert_failed_with(result: &serde_json::Value, needle: &str) {
    assert_eq!(result["status"], "failed", "{result}");
    let error = error_of(result);
    assert!(error.contains(needle), "нет «{needle}» в «{error}»");
}

/// Движок после сценария снова на автомате: ручных значений, точки и блокировки нет.
fn assert_engine_restored(harness: &Harness) {
    let status = harness
        .rig()
        .handle()
        .wait_until_status(WAIT, |status| {
            status.three_a.controls == ManualControl::AUTOMATIC
                && matches!(
                    status.three_a.point,
                    camera_contract::three_a::PointOfInterest::WholeFrame
                )
                && status.three_a.lock == AeAfLock::Unlocked
        })
        .unwrap_or_else(|| panic!("автоматика не вернулась: {:?}", harness.rig().handle().status().three_a));
    assert!(matches!(
        status.three_a.focus,
        FocusProgress::Continuous { .. } | FocusProgress::FixedLens
    ));
}

#[test]
fn healthy_run_passes_every_step_and_restores_automatic_controls() {
    let (harness, result) = run_three_a(HalFaults::default());

    assert_eq!(result["status"], "passed", "{result}");
    let report = &result["report"];
    assert_eq!(report["live"]["companion"]["state"], "active", "{report}");
    assert_eq!(report["live"]["af_state"], "passive_focused");
    assert_eq!(report["live"]["ae_state"], "converged");
    assert_eq!(report["awb"]["ready"], true);
    assert_eq!(report["awb"]["awb_vs_hal"], "ok");
    let taps = report["focus"]["taps"].as_array().expect("тапы");
    assert_eq!(taps.len(), 5, "центр и четыре угла");
    assert!(taps.iter().all(|tap| tap["result"] == "focused"), "{taps:?}");
    assert_eq!(report["focus"]["triggers_sent"], 5);
    assert!(
        report["focus"]["region_effect_diopters"]
            .as_f64()
            .is_some_and(|effect| effect > 0.5),
        "разные точки — разные дистанции: {report}"
    );
    assert_eq!(report["manual"]["applied"]["exposure_ns"], 1_000_000);
    assert_eq!(report["manual"]["applied"]["iso"], 400);
    assert_eq!(report["manual"]["adjustments_for_in_range"], 0);
    assert_eq!(report["manual"]["long_shutter"]["limited_event_seen"], true);
    assert!(
        report["manual"]["long_shutter"]["applied_ns"]
            .as_u64()
            .is_some_and(|ns| ns <= 16_700_000)
    );
    assert_eq!(report["ev"]["ev_unverifiable_at_limit"], false, "{report}");
    let ratios: Vec<f64> = report["ev"]["points"]
        .as_array()
        .expect("точки EV")
        .iter()
        .filter_map(|point| point["ratio_to_baseline"].as_f64())
        .collect();
    assert_eq!(ratios.len(), 3);
    assert!(
        (ratios[1] - 2.0).abs() < 0.05 && (ratios[2] - 0.5).abs() < 0.05,
        "{ratios:?}"
    );
    assert_eq!(report["lock"]["locked"], true);
    assert_eq!(report["lock"]["back_to_continuous"], true);
    assert_eq!(report["bracket"]["captured"], true);
    let frames = report["bracket"]["frames"].as_array().expect("кадры серии");
    let exposures: Vec<u64> = frames
        .iter()
        .filter_map(|frame| frame["exposure_ns"].as_u64())
        .collect();
    assert_eq!(exposures, [2_000_000, 4_000_000, 8_000_000, 16_000_000]);
    assert_eq!(report["bracket"]["frame_gaps"], 0);
    assert_eq!(report["bracket"]["skipped_delta"], 4);
    assert!(result.to_string().len() < 4_000, "строка результата в бюджете");

    let stages = harness.output.stage_names();
    for expected in ["three_a_live", "awb_ready", "three_a_tap", "detail_three_a"] {
        assert!(
            stages.iter().any(|stage| stage == expected),
            "нет стадии {expected}: {stages:?}"
        );
    }
    assert_eq!(stages.iter().filter(|stage| *stage == "three_a_tap").count(), 5);
    assert_engine_restored(&harness);
}

#[test]
fn dead_companion_fails_the_scenario_and_skips_the_remaining_steps() {
    use camera_android::companion_stream::{CompanionStreamStatus, CompanionUnavailableReason};
    let (harness, result) = run_three_a_with(
        short_config(|_| {}),
        HalFaults::default(),
        |rig| {
            rig.diagnostics
                .record_companion_status(CompanionStreamStatus::Unavailable {
                    reason: CompanionUnavailableReason::RejectedByHal,
                });
        },
        |_, _| {},
    );

    assert_failed_with(&result, "вспомогательный поток 3A не активен");
    assert!(
        result["report"]["focus"].is_null(),
        "шаги после мёртвой автоматики не шли: {result}"
    );
    assert_engine_restored(&harness);
}

#[test]
fn hal_without_ae_state_fails_the_live_rule() {
    let (_harness, result) = run_three_a(HalFaults {
        no_ae_state: true,
        ..HalFaults::default()
    });
    assert_failed_with(&result, "нет AE_STATE");
}

#[test]
fn hal_ignoring_manual_exposure_fails_the_exposure_rule() {
    let (_harness, result) = run_three_a(HalFaults {
        ignores_manual_exposure: true,
        ..HalFaults::default()
    });
    assert_failed_with(&result, "не появилась в результатах");
}

#[test]
fn focus_that_never_settles_fails_the_center_tap_rule() {
    let (_harness, result) = run_three_a(HalFaults {
        focus_never_settles: true,
        ..HalFaults::default()
    });
    assert_failed_with(&result, "тап в центр: HAL не сообщил итог");
    assert_eq!(result["report"]["focus"]["taps"][0]["result"], "timed_out", "{result}");
}

#[test]
fn not_focused_is_an_honest_hal_answer_and_does_not_fail() {
    let (_harness, result) = run_three_a(HalFaults {
        focus_reports_not_focused: true,
        ..HalFaults::default()
    });
    assert_eq!(result["status"], "passed", "{result}");
    assert_eq!(result["report"]["focus"]["taps"][0]["result"], "not_focused");
}

#[test]
fn backend_rejecting_the_focus_trigger_is_reported_as_not_delivered() {
    let (_harness, result) = run_three_a_with(
        short_config(|_| {}),
        HalFaults::default(),
        |rig| {
            // Отказ HAL на одном разовом запросе: поток живёт, тап не доставлен.
            rig.control
                .push_trigger_result(Err(RequestFailure::Backend(BackendFailure {
                    operation: "ACameraCaptureSession_capture",
                    code: -1,
                    detail: "тестовый отказ".to_owned(),
                })));
        },
        |_, _| {},
    );
    assert_failed_with(&result, "бэкенд отверг разовый запрос");
    assert_eq!(
        result["report"]["focus"]["taps"][0]["result"], "trigger_not_delivered",
        "{result}"
    );
}

#[test]
fn exposure_bias_that_does_not_act_fails_the_ev_rule() {
    let (_harness, result) = run_three_a(HalFaults {
        ignores_exposure_bias: true,
        ..HalFaults::default()
    });
    assert_failed_with(&result, "EV +1");
}

#[test]
fn ae_that_does_not_lock_fails_the_lock_rule() {
    let (_harness, result) = run_three_a(HalFaults {
        ae_never_locks: true,
        ..HalFaults::default()
    });
    assert_failed_with(&result, "AE_STATE не стал Locked");
}

#[test]
fn burst_with_a_lost_frame_number_fails_the_consecutive_frames_rule() {
    let (_harness, result) = run_three_a(HalFaults {
        burst_loses_a_frame_number: true,
        ..HalFaults::default()
    });
    assert_failed_with(&result, "номера кадров серии не подряд");
}

#[test]
fn interrupted_stream_during_manual_exposure_still_restores_automatic_controls() {
    let mut disconnected = false;
    let (harness, result) = run_three_a_with(
        short_config(|_| {}),
        HalFaults::default(),
        |_| {},
        |harness, _| {
            // Как только движок держит ручные значения (шаг d), у камеры отнимают устройство.
            let manual = harness.rig().handle().status().three_a.controls != ManualControl::AUTOMATIC;
            if manual && !disconnected {
                disconnected = true;
                harness.rig().control.disconnect_current_device();
            }
        },
    );

    assert!(disconnected, "ручные значения не появились — вмешаться было не во что");
    assert_failed_with(&result, "прервался");
    assert_engine_restored(&harness);
}

// ---------------------------------------------------------------------------------------------
// Наш AE (08a): экспозицию ведёт движок по предложениям подставного рендера, HAL получает `AE_MODE_OFF`.
// ---------------------------------------------------------------------------------------------

fn run_three_a_own(faults: HalFaults) -> (Harness, serde_json::Value) {
    run_three_a_with(short_own_config(|_| {}), faults, |_| {}, |_, _| {})
}

/// Запросы повторяющегося потока, ушедшие в бэкенд с вендорским AE.
fn vendor_auto_requests(harness: &Harness) -> usize {
    harness
        .rig()
        .control
        .calls()
        .iter()
        .filter(|call| {
            matches!(call, camera_core::fake::BackendCall::SetRepeatingRequest { controls, .. }
                if matches!(controls.exposure, camera_core::request_controls::ExposurePlan::VendorAuto { .. }))
        })
        .count()
}

#[test]
fn run_with_our_ae_passes_and_never_sends_a_vendor_ae_request() {
    let (harness, result) = run_three_a_own(HalFaults::default());

    assert_eq!(result["status"], "passed", "{result}");
    let report = &result["report"];
    assert_eq!(report["live"]["ae_source"], "own", "{report}");
    assert_eq!(report["live"]["ae_state"], "inactive", "HAL при AE_MODE_OFF: {report}");
    assert_eq!(
        vendor_auto_requests(&harness),
        0,
        "ни одного повторяющегося запроса с вендорским AE: {:?}",
        harness.rig().control.calls()
    );
    let own = &report["own_ae"];
    assert_eq!(own["vendor_ae_results"], 0, "{own}");
    let checked = &own["results_checked"];
    assert!(checked["repeating"].as_u64().is_some_and(|n| n > 10), "{own}");
    assert!(
        checked["focus_trigger"].as_u64().is_some_and(|n| n > 0),
        "тапы проверены на вендорский AE: {own}"
    );
    assert!(
        checked["burst"].as_u64().is_some_and(|n| n > 0),
        "кадры серии проверены на вендорский AE: {own}"
    );
    assert_eq!(own["settled_seen"], true, "{own}");
    assert_eq!(own["holding_seen"], true, "блокировка дошла до Holding: {own}");
    assert_engine_restored(&harness);
}

#[test]
fn ev_points_are_metered_on_the_whole_frame_even_after_the_corner_taps() {
    // Решение 135 (13c): тапы шага c оставляли точку замера в углу кадра, и точки EV мерились по нему.
    // Здесь угол «тёмный»: точечному замеру нужно в 8 раз больше света (+3 EV), чем всему кадру. Шаг EV обязан
    // сбросить точку и мерить по всему кадру — опорная точка встаёт на свет кадра, а не угла.
    let mut tap_point_set = false;
    let (harness, result) = run_three_a_with(
        short_own_config(|_| {}),
        HalFaults::default(),
        |_| {},
        |_, render_ae| {
            if let (false, Some(render_ae)) = (tap_point_set, render_ae) {
                render_ae.set_tap_point_scene(TapPointScene::Needs(8.0 * DEFAULT_SCENE_SECONDS_ISO));
                tap_point_set = true;
            }
        },
    );

    assert_eq!(result["status"], "passed", "{result}");
    let points = result["report"]["ev"]["points"].as_array().expect("точки EV");
    assert_eq!(points.len(), 3, "{points:?}");
    assert!(
        points.iter().all(|point| point["metered_area"] == "whole_frame"),
        "все точки EV — по всему кадру: {points:?}"
    );
    // Опорная точка: свет всего кадра (2 с·ISO = 2·10⁹ нс·ISO), а не угла (16 с·ISO).
    let baseline_ns_iso = points[0]["median_product"].as_f64().expect("медиана опорной точки");
    let baseline_ev_from_frame = (baseline_ns_iso / (DEFAULT_SCENE_SECONDS_ISO * 1.0e9)).log2();
    assert!(
        baseline_ev_from_frame.abs() < 0.3,
        "опора EV на свету кадра, а не тёмного угла: {baseline_ev_from_frame:.2} EV ({points:?})"
    );
    // Перед шагом EV были все пять тапов (последний — в углу): сброс делает именно шаг EV.
    let taps = result["report"]["focus"]["taps"].as_array().expect("тапы");
    assert_eq!(taps.len(), 5, "{taps:?}");
    assert_engine_restored(&harness);
}

#[test]
fn ev_steps_scale_the_exposure_product_by_two_and_one_half_with_our_ae() {
    let (_harness, result) = run_three_a_own(HalFaults::default());

    assert_eq!(result["status"], "passed", "{result}");
    let points = result["report"]["ev"]["points"].as_array().expect("точки EV");
    let ratios: Vec<f64> = points
        .iter()
        .filter_map(|point| point["ratio_to_baseline"].as_f64())
        .collect();
    assert_eq!(ratios.len(), 3);
    assert!(
        (ratios[1] - 2.0).abs() < 0.05 && (ratios[2] - 0.5).abs() < 0.05,
        "+1 EV вдвое, -1 EV вполовину: {ratios:?}"
    );
    assert!(
        points.iter().all(|point| point["ae_converged"] == true),
        "маркер сходимости — Settled нашего AE, а не Converged HAL: {points:?}"
    );
}

#[test]
fn lock_keeps_the_product_constant_while_the_scene_changes_under_our_ae() {
    let mut scene_changed = false;
    let (_harness, result) = run_three_a_with(
        short_own_config(|three_a| three_a.lock.hold_ms = 600),
        HalFaults::default(),
        |_| {},
        |harness, render_ae| {
            // Как только движок держит блокировку (Holding), «сцена» резко светлеет в 4 раза.
            let holding = harness.rig().handle().status().three_a.auto_exposure
                == Some(camera_core::three_a_status::AutoExposureProgress::Holding);
            if let (true, false, Some(render_ae)) = (holding, scene_changed, render_ae) {
                scene_changed = true;
                render_ae.change_scene(8.0);
            }
        },
    );

    assert!(scene_changed, "блокировка не наступила — менять сцену было не во что");
    assert_eq!(result["status"], "passed", "{result}");
    let lock = &result["report"]["lock"];
    assert_eq!(lock["locked"], true, "{lock}");
    assert_eq!(lock["ae_source"], "own", "{lock}");
    assert!(
        lock["max_product_deviation"]
            .as_f64()
            .is_some_and(|deviation| deviation <= 0.01),
        "под блокировкой произведение постоянно, несмотря на смену сцены: {lock}"
    );
    assert!(lock["samples_held"].as_u64().is_some_and(|n| n > 10), "{lock}");
}

#[test]
fn our_ae_without_proposals_from_the_renderer_fails_the_scenario_by_its_own_rule() {
    // Рендера нет (подставной не запущен): предложений нет, AE остаётся на стартовом значении.
    let mut config = short_own_config(|_| {});
    config.exposure_controller = ExposureControllerKind::Own;
    let rig = FakeCameraRig::with_config(config);
    let mut harness = harness_for_rig(rig);
    harness.deliver(ShellEvent::Resumed);
    let paced = PumpFaults {
        paced_at_sensor_rate: true,
        results_from_external_hal: true,
        ..PumpFaults::default()
    };
    let _pump = StreamPump::start(harness.rig(), Arc::clone(&harness.render), paced);
    let hal = SimulatedHal::start(harness.rig(), HalFaults::default());
    let _mirror = ThreeAMirror::start(&harness, Arc::clone(&hal.stats), None);
    harness.start(Scenario::parse("3a").expect("сценарий зарегистрирован"));
    let deadline = Instant::now() + Duration::from_secs(30);
    while harness.output.results.lock().expect("замок").is_empty() && Instant::now() < deadline {
        harness.router.step(Duration::from_millis(2));
    }
    let result = harness.output.single_result();

    assert_eq!(result["status"], "failed", "{result}");
    let error = error_of(&result);
    assert!(error.contains("наш AE не дошёл до Holding"), "{error}");
    assert_eq!(result["report"]["own_ae"]["settled_seen"], false, "{result}");
}

#[test]
fn unavailable_renderer_gives_the_camera_vendor_ae_visibly_and_the_scenario_passes_as_vendor() {
    use crate::auto_exposure_link::RenderAutoExposure;

    let mut config = short_own_config(|_| {});
    config.viewfinder_autostart.enabled = false;
    assert_eq!(config.exposure_controller, ExposureControllerKind::Own);
    let rig = FakeCameraRig::with_product_config_and_render(
        config,
        RenderAutoExposure::Unavailable {
            reason: "GPU не поднялся".to_owned(),
        },
    );
    let choice = rig.services.exposure_controller().clone();
    assert_eq!(choice.configured, ExposureControllerKind::Own);
    assert_eq!(choice.effective, ExposureControllerKind::Vendor);
    assert!(
        choice
            .fallback_reason
            .as_deref()
            .is_some_and(|reason| reason.contains("GPU не поднялся"))
    );

    let (harness, result) = run_three_a_on(rig, HalFaults::default(), |_, _| {});

    assert_eq!(result["status"], "passed", "{result}");
    assert_eq!(result["report"]["live"]["ae_source"], "vendor", "{result}");
    assert_eq!(result["report"]["expected_ae_source"], "vendor");
    assert!(
        result["report"]["ae_fallback_reason"]
            .as_str()
            .is_some_and(|reason| reason.contains("GPU не поднялся")),
        "причина замены видна в отчёте: {result}"
    );
    assert!(vendor_auto_requests(&harness) > 0, "камера работала на вендорском AE");
}

#[test]
fn scenario_fails_when_the_engine_runs_another_ae_than_the_composition_chose() {
    // Композиция заявляет наш AE, а в движке (по конфигу vendor) стоит вендорский.
    let rig = FakeCameraRig::with_config(short_config(|_| {}));
    let rig = FakeCameraRig {
        services: rig.services.clone().with_exposure_controller(
            crate::auto_exposure_link::ExposureControllerChoice::resolve(
                ExposureControllerKind::Own,
                &crate::auto_exposure_link::RenderAutoExposure::Connected,
            ),
        ),
        ..rig
    };
    let (_harness, result) = run_three_a_on(rig, HalFaults::default(), |_, _| {});
    assert_failed_with(&result, "а композиция поставила Own");
}

#[path = "event_router_three_a_lock_tests.rs"]
mod lock_window;

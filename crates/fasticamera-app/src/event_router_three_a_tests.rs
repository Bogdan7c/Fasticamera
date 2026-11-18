//! Функциональные тесты сценария `3a` (сессия 08): настоящий маршрутизатор, настоящий движок камеры на
//! fake-платформе, подставной HAL 3A ([`SimulatedHal`]: единственный источник результатов съёмки) и
//! подставной рендер ([`ThreeAMirror`]: счётчики AWB и пропуск кадров серии). Каждый тест доходит до
//! строки результата, а не проверяет отдельный шаг.

use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};

use camera_contract::control::ManualControl;
use camera_contract::three_a::AeAfLock;
use camera_core::backend::{BackendFailure, RequestFailure};
use camera_core::three_a_status::FocusProgress;
use isp_vulkan::viewfinder::IspStageSet;
use render_loop::config::WhiteBalanceConfig;
use render_loop::viewfinder::{
    HeadroomBenchDiagnostics, ViewfinderColorSpace, ViewfinderCounters, ViewfinderDiagnostics, ViewfinderExtent,
    ViewfinderGpuTimes, ViewfinderPainter, ViewfinderStorage, WhiteBalanceDiagnostics, WhiteDelta, WhiteReading,
};

use super::*;
use crate::camera_config::CameraConfig;
use crate::camera_scenario_config::CameraScenarioConfig;
use crate::camera_test_support::WAIT;
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

fn viewfinder_diagnostics() -> ViewfinderDiagnostics {
    ViewfinderDiagnostics {
        painter: ViewfinderPainter::Isp,
        stages: IspStageSet::ALL,
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
    }
}

impl ThreeAMirror {
    fn start(harness: &Harness, hal_stats: Arc<HalStats>) -> Self {
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

/// Короткие сроки всех шагов: тест идёт секунды, а не минуты. Запуск видоискателя — как в тестах `viewfinder`.
fn short_config(tune: impl FnOnce(&mut ThreeAScenarioConfig)) -> CameraConfig {
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
    mut during: impl FnMut(&Harness),
) -> (Harness, serde_json::Value) {
    let rig = FakeCameraRig::with_config(config);
    tune_rig(&rig);
    let mut harness = harness_for_rig(rig);
    harness.deliver(ShellEvent::Resumed);
    let paced = PumpFaults {
        paced_at_sensor_rate: true,
        results_from_external_hal: true,
        ..PumpFaults::default()
    };
    let _pump = StreamPump::start(harness.rig(), Arc::clone(&harness.render), paced);
    let hal = SimulatedHal::start(harness.rig(), faults);
    let _mirror = ThreeAMirror::start(&harness, Arc::clone(&hal.stats));
    harness.start(Scenario::parse("3a").expect("сценарий зарегистрирован"));
    let deadline = Instant::now() + Duration::from_secs(30);
    while harness.output.results.lock().expect("замок").is_empty() && Instant::now() < deadline {
        harness.router.step(Duration::from_millis(2));
        during(&harness);
    }
    let result = harness.output.single_result();
    (harness, result)
}

fn run_three_a(faults: HalFaults) -> (Harness, serde_json::Value) {
    run_three_a_with(short_config(|_| {}), faults, |_| {}, |_| {})
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
        |_| {},
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
        |_| {},
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
        |harness| {
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

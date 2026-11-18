//! Функциональные тесты сценариев видоискателя сессии 07 (`viewfinder`, `viewfinder-latency`,
//! `viewfinder-stage-budget`, `viewfinder-endurance`, `video-headroom-bench`): настоящий
//! маршрутизатор, настоящий движок камеры на fake-платформе, подставной «живой поток»
//! (`StreamPump`) и подставной поток рендера ([`RenderMirror`]), который, как настоящий, применяет
//! регулировки сценария и ведёт счётчики ISP. Каждый тест доходит до строки результата.

use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};

use isp_vulkan::viewfinder::{IspStage, IspStageSet};
use render_loop::config::{AdpfMode, PresentTargetConfig, WhiteBalanceConfig};
use render_loop::thread::{AdpfSessionState, DurationSummary, PresentTargetSelection};
use render_loop::viewfinder::{
    HeadroomBench, HeadroomBenchDiagnostics, HeadroomPassTime, ViewfinderColorSpace, ViewfinderCommand,
    ViewfinderCounters, ViewfinderDiagnostics, ViewfinderExtent, ViewfinderGpuTimes, ViewfinderPainter,
    ViewfinderStorage, WhiteBalanceDiagnostics,
};
use shell_contract::power::fake::FakePowerSource;
use shell_contract::power::{BatteryCurrent, BatterySample, ChargingState, PowerPlug};

use super::*;
use crate::camera_config::CameraConfig;
use crate::camera_scenario_config::CameraScenarioConfig;
use crate::clock_offset::monotonic_now;
use crate::power_measurement::PowerMeasurementConfig;
use crate::raw_stream_test_support::{PumpFaults, StreamPump};
use crate::viewfinder_control::RenderAdjustment;
use crate::viewfinder_scenario_config::{IspStageSetting, StageBudgetRowSetting, ViewfinderScenarioConfig};

/// Ток телефона в окнах «со стадией» и «без неё», мА (разница — 20 мА).
const CURRENT_WITH_STAGE_MA: i64 = 520;
const CURRENT_WITHOUT_STAGE_MA: i64 = 500;

/// Что умеет подставной рендер помимо здорового показа.
#[derive(Clone, Copy, Default)]
struct MirrorFaults {
    /// ISP обрабатывает только каждый второй показанный кадр.
    isp_skips_half: bool,
    /// Поток рендера принимает команды ADPF, но режим не меняет (в статусе он остаётся выключенным).
    ignores_adpf_command: bool,
}

/// Подставной поток рендера: как настоящий, применяет регулировки сценария (последнюю по виду) и
/// ведёт счётчики видоискателя по числу показанных кадров камеры.
struct RenderMirror {
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

/// Действующие настройки по журналу регулировок.
struct Effective {
    painter: ViewfinderPainter,
    stages: IspStageSet,
    bench: HeadroomBench,
    selection: PresentTargetSelection,
    adpf: AdpfMode,
}

fn effective(adjustments: &[RenderAdjustment]) -> Effective {
    let mut current = Effective {
        painter: ViewfinderPainter::Isp,
        stages: IspStageSet::ALL,
        bench: HeadroomBench::Disabled,
        // Штатный выбор рендера — из его конфига по умолчанию (продуктовые значения решения 54),
        // а не догадка: возврат после сценария обязан привести ровно к нему.
        selection: PresentTargetSelection::of_config(&PresentTargetConfig::default()),
        adpf: AdpfMode::Disabled,
    };
    for adjustment in adjustments {
        match adjustment {
            RenderAdjustment::Viewfinder(ViewfinderCommand::SetPainter(painter)) => current.painter = *painter,
            RenderAdjustment::Viewfinder(ViewfinderCommand::SetIspStages(stages)) => current.stages = *stages,
            RenderAdjustment::Viewfinder(ViewfinderCommand::SetHeadroomBench(bench)) => current.bench = *bench,
            RenderAdjustment::Viewfinder(ViewfinderCommand::SetCrop(_)) => {}
            RenderAdjustment::PresentTarget(setting) => {
                current.selection = PresentTargetSelection {
                    policy: setting.policy,
                    late_latch: setting.late_latch,
                    depth_reduction: setting.depth_reduction,
                }
            }
            RenderAdjustment::Adpf(mode) => current.adpf = *mode,
        }
    }
    current
}

fn micros(value: u64) -> DurationSummary {
    DurationSummary {
        samples: 600,
        p50: Duration::from_micros(value),
        p95: Duration::from_micros(value + 300),
        max: Duration::from_micros(value + 900),
    }
}

/// Проход ISP: 1 мс + 0,1 мс за каждую включённую стадию.
fn isp_micros(stages: IspStageSet) -> u64 {
    1_000 + 100 * IspStage::ALL.iter().filter(|stage| stages.contains(**stage)).count() as u64
}

fn fresh_viewfinder() -> ViewfinderDiagnostics {
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

impl RenderMirror {
    fn start(harness: &Harness, faults: MirrorFaults) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let render = Arc::clone(&harness.render);
        let control = Arc::clone(&harness.control);
        let shell = Arc::clone(&harness.shell);
        let join = thread::spawn(move || {
            let mut bench_started_at: Option<u64> = None;
            // Отчёты ADPF копятся по показанным кадрам, пока режим включён (как у настоящего рендера).
            let mut adpf_reports = 0_u64;
            let mut last_presented = 0_u64;
            while !thread_stop.load(Ordering::SeqCst) {
                let now = effective(&control.recorded());
                let refresh_hz = shell.frame_rates.lock().expect("замок").last().copied().unwrap_or(60.0);
                render.update(|status| {
                    let presented = status
                        .backend
                        .camera
                        .as_ref()
                        .map_or(0, |camera| camera.camera_frames_presented);
                    let viewfinder = status.backend.viewfinder.get_or_insert_with(fresh_viewfinder);
                    viewfinder.painter = now.painter;
                    viewfinder.stages = now.stages;
                    let processed = if faults.isp_skips_half {
                        presented / 2
                    } else {
                        presented
                    };
                    viewfinder.counters.frames_processed = processed;
                    // Как настоящий рендер: параметры разрешаются один раз на поток, а не на кадр.
                    viewfinder.counters.params_resolved = u64::from(processed > 0);
                    // Как настоящий рендер: статистика AWB считается на каждом N-м обработанном кадре.
                    let period = u64::from(viewfinder.white_balance.statistics_every_n_frames.max(1));
                    viewfinder.counters.statistics_published = processed / period;
                    viewfinder.white_balance.estimates = processed / period;
                    viewfinder.gpu_time.isp_dispatch =
                        (now.painter == ViewfinderPainter::Isp).then(|| micros(isp_micros(now.stages)));
                    viewfinder.gpu_time.frame_total = Some(match now.painter {
                        ViewfinderPainter::Isp => micros(isp_micros(now.stages) + 600),
                        ViewfinderPainter::DebugMosaic => micros(700),
                    });
                    viewfinder.headroom.mode = now.bench;
                    if now.bench == HeadroomBench::Enabled {
                        let start = *bench_started_at.get_or_insert(presented);
                        viewfinder.headroom.frames_recorded = presented - start;
                        viewfinder.headroom.total = Some(micros(12_000));
                        viewfinder.headroom.pass_times = vec![HeadroomPassTime {
                            pass: "temporal_denoise",
                            gpu_time: micros(4_000),
                        }];
                    }
                    let adpf_mode = if faults.ignores_adpf_command {
                        AdpfMode::Disabled
                    } else {
                        now.adpf
                    };
                    if adpf_mode == AdpfMode::Enabled {
                        adpf_reports += presented.saturating_sub(last_presented);
                    }
                    last_presented = presented;
                    status.adpf.mode = adpf_mode;
                    status.adpf.session = match adpf_mode {
                        AdpfMode::Enabled => AdpfSessionState::Active,
                        AdpfMode::Disabled => AdpfSessionState::Disabled,
                    };
                    status.adpf.reports_sent = adpf_reports;
                    status.present_target.frame_gpu_time = Some(micros(9_000));
                    status.present_target.selection = now.selection;
                    status.present_target.frames_planned = presented;
                    status.present_target.vsync_shift.late_by_periods[0] = presented;
                    if let Some(display) = &mut status.display {
                        display.refresh_cycle = Some(Duration::from_secs_f64(1.0 / f64::from(refresh_hz)));
                    }
                });
                thread::sleep(Duration::from_millis(1));
            }
        });
        Self { stop, join: Some(join) }
    }
}

impl Drop for RenderMirror {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

/// Подставная батарея: каждые 10 мс показание с током по действующей настройке видоискателя.
struct BatteryPump {
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

impl BatteryPump {
    fn start(harness: &Harness, plug: PowerPlug) -> Self {
        let battery = Arc::new(FakePowerSource::new());
        *harness.shell.battery.lock().expect("замок") = Some(Arc::clone(&battery));
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let control = Arc::clone(&harness.control);
        let join = thread::spawn(move || {
            while !thread_stop.load(Ordering::SeqCst) {
                let now = effective(&control.recorded());
                let product = now.painter == ViewfinderPainter::Isp && now.stages == IspStageSet::ALL;
                let discharge_ma = if product {
                    CURRENT_WITH_STAGE_MA
                } else {
                    CURRENT_WITHOUT_STAGE_MA
                };
                if let Ok(at) = monotonic_now() {
                    // Опрос ещё не запущен — показание некуда класть, это нормально.
                    let _ = battery.push_sample(BatterySample {
                        at,
                        current: Some(BatteryCurrent::from_raw_microamps(-discharge_ma * 1_000)),
                        charge_counter: None,
                        charging: if plug == PowerPlug::Unplugged {
                            ChargingState::Discharging
                        } else {
                            ChargingState::Charging
                        },
                        plug,
                        temperature: None,
                        level_percent: Some(80),
                        voltage: None,
                    });
                }
                thread::sleep(Duration::from_millis(10));
            }
        });
        Self { stop, join: Some(join) }
    }
}

impl Drop for BatteryPump {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

/// Короткие сроки всех сценариев видоискателя.
fn short_config(tune: impl FnOnce(&mut ViewfinderScenarioConfig, &mut PowerMeasurementConfig)) -> CameraConfig {
    let mut viewfinder = ViewfinderScenarioConfig {
        streaming_timeout_ms: 2_000,
        render_live_timeout_ms: 1_000,
        viewfinder_settle_ms: 50,
        progress_interval_ms: 100,
        ..ViewfinderScenarioConfig::default()
    };
    viewfinder.smoke.measure_ms = 300;
    // Короткое окно: кадров ~18, ±1 кадр на краях окна — до 6 %.
    viewfinder.smoke.frame_rate_tolerance = 0.2;
    viewfinder.latency.window_ms = 200;
    viewfinder.latency.warmup_ms = 50;
    viewfinder.latency.min_samples_per_window = 3;
    viewfinder.headroom.duration_ms = 400;
    viewfinder.headroom.snapshot_interval_ms = 200;
    let mut power = PowerMeasurementConfig {
        sampling_period_ms: 10,
        ring_capacity: 4_096,
        sign_detection_min_samples: 4,
        max_missing_fraction: 0.6,
        ..PowerMeasurementConfig::default()
    };
    power.ab.window_ms = 300;
    power.ab.warmup_ms = 100;
    power.ab.window_close_grace_ms = 50;
    power.endurance.duration_ms = 600;
    power.endurance.snapshot_interval_ms = 200;
    tune(&mut viewfinder, &mut power);
    CameraConfig {
        scenarios: CameraScenarioConfig {
            viewfinder,
            power,
            ..CameraScenarioConfig::default()
        },
        ..CameraConfig::default()
    }
}

/// Сценарий до итога на здоровом потоке; батарея — по запросу.
fn run_viewfinder(
    config: CameraConfig,
    name: &str,
    faults: MirrorFaults,
    battery: Option<PowerPlug>,
) -> (Harness, serde_json::Value) {
    run_viewfinder_on(FakeCameraRig::with_config(config), name, faults, battery)
}

/// То же на готовом стенде камеры (тест мог подготовить диагностику бэкенда).
fn run_viewfinder_on(
    rig: FakeCameraRig,
    name: &str,
    faults: MirrorFaults,
    battery: Option<PowerPlug>,
) -> (Harness, serde_json::Value) {
    let mut harness = harness_for_rig(rig);
    harness.deliver(ShellEvent::Resumed);
    let paced = PumpFaults {
        paced_at_sensor_rate: true,
        ..PumpFaults::default()
    };
    let _pump = StreamPump::start(harness.rig(), Arc::clone(&harness.render), paced);
    let _mirror = RenderMirror::start(&harness, faults);
    let _battery = battery.map(|plug| BatteryPump::start(&harness, plug));
    harness.start(Scenario::parse(name).expect("сценарий зарегистрирован"));
    let deadline = Instant::now() + Duration::from_secs(30);
    while harness.output.results.lock().expect("замок").is_empty() && Instant::now() < deadline {
        harness.router.step(Duration::from_millis(2));
    }
    let result = harness.output.single_result();
    (harness, result)
}

#[path = "event_router_viewfinder_latency_tests.rs"]
mod latency;

fn stages_named(harness: &Harness, name: &str) -> Vec<serde_json::Value> {
    harness
        .output
        .stages
        .lock()
        .expect("замок")
        .iter()
        .filter(|line| line["stage"] == name)
        .cloned()
        .collect()
}

/// Строка результата и каждая строка стадии помещаются в запись logcat.
fn assert_fits_logcat(harness: &Harness, result: &serde_json::Value) {
    use crate::logging::record::LOGCAT_MAX_TEXT_BYTES;
    use crate::result_line::{RESULT_LINE_PREFIX, STAGE_LINE_PREFIX};
    let bytes = RESULT_LINE_PREFIX.len() + result.to_string().len();
    assert!(bytes <= LOGCAT_MAX_TEXT_BYTES, "строка результата {bytes} байт");
    for line in harness.output.stages.lock().expect("замок").iter() {
        let bytes = STAGE_LINE_PREFIX.len() + line.to_string().len();
        assert!(bytes <= LOGCAT_MAX_TEXT_BYTES, "строка стадии {bytes} байт: {line}");
    }
}

#[test]
fn viewfinder_smoke_passes_on_a_healthy_isp_and_reports_gpu_time_color_space_awb_and_latency() {
    let (harness, result) = run_viewfinder(short_config(|_, _| {}), "viewfinder", MirrorFaults::default(), None);

    assert_eq!(result["status"], "passed", "{result}");
    let report = &result["report"];
    assert_eq!(report["configured"]["camera"], "2");
    assert_eq!(report["look"]["display_color_space"], "display_p3");
    assert_eq!(report["look"]["painter"], "isp");
    assert_eq!(
        report["gpu_time"]["isp_dispatch"]["p50_ms"], 1.8,
        "8 стадий: 1 + 8 × 0,1 мс"
    );
    assert!(report["awb_published"].as_u64().expect("AWB") > 0);
    assert_eq!(report["cpu_copies"], 0);
    let p50 = report["latency"]["p50_ms"].as_f64().expect("p50");
    assert!((39.5..=40.5).contains(&p50), "задержка {p50}");
    assert_eq!(report["frames_processed"], report["frames_presented"]);
    // Стадии: скриншоты делаются на viewfinder_live, подробности — до результата.
    let names = harness.output.stage_names();
    let position = |name: &str| names.iter().position(|stage| stage == name).expect(name);
    assert!(position("streaming") < position("viewfinder_live"), "{names:?}");
    assert!(position("viewfinder_live") < position("detail_viewfinder"), "{names:?}");
    // `viewfinder` рендер не регулирует: возвращать нечего.
    assert!(
        harness.control.recorded().is_empty(),
        "{:?}",
        harness.control.recorded()
    );
    assert_fits_logcat(&harness, &result);
}

#[test]
fn viewfinder_smoke_fails_when_isp_misses_presented_frames() {
    let faults = MirrorFaults {
        isp_skips_half: true,
        ..MirrorFaults::default()
    };
    let (_harness, result) = run_viewfinder(short_config(|_, _| {}), "viewfinder", faults, None);

    assert_eq!(result["status"], "failed", "{result}");
    let error = result["error"].as_str().unwrap_or_default();
    assert!(error.contains("каждый показанный"), "{error}");
}

#[test]
fn viewfinder_smoke_fails_when_the_three_a_companion_stream_is_dead() {
    use camera_android::companion_stream::{CompanionStreamStatus, CompanionUnavailableReason};
    let rig = FakeCameraRig::with_config(short_config(|_, _| {}));
    rig.diagnostics
        .record_companion_status(CompanionStreamStatus::Unavailable {
            reason: CompanionUnavailableReason::RejectedByHal,
        });
    let (_harness, result) = run_viewfinder_on(rig, "viewfinder", MirrorFaults::default(), None);

    assert_eq!(result["status"], "failed", "{result}");
    let error = result["error"].as_str().unwrap_or_default();
    assert!(error.contains("вспомогательный поток 3A мёртв"), "{error}");
    assert_eq!(
        result["report"]["three_a_companion"]["state"], "unavailable",
        "{result}"
    );
}

#[test]
fn viewfinder_smoke_reports_decimated_awb_statistics_and_the_white_balance_section() {
    let (_harness, result) = run_viewfinder(short_config(|_, _| {}), "viewfinder", MirrorFaults::default(), None);

    assert_eq!(result["status"], "passed", "{result}");
    let report = &result["report"];
    let processed = report["frames_processed"].as_u64().expect("кадры");
    let published = report["awb_published"].as_u64().expect("статистика");
    assert_eq!(
        published,
        processed / 4,
        "статистика идёт на каждом 4-м кадре: {report}"
    );
    assert!(
        report["white_balance"]["estimates"].as_u64().is_some_and(|n| n > 0),
        "{report}"
    );
    assert_eq!(report["three_a_companion"]["state"], "active");
    assert!(result.to_string().len() < 4_000, "строка результата в бюджете");
}

#[test]
fn viewfinder_smoke_over_isp_target_passes_with_a_note_in_the_result_line() {
    // Решение 56: превышение цели прохода ISP — пометка в строке результата, а не провал.
    let config = short_config(|viewfinder, _| viewfinder.smoke.isp_dispatch_budget_ms = 1.5);
    let (_harness, result) = run_viewfinder(config, "viewfinder", MirrorFaults::default(), None);

    assert_eq!(result["status"], "passed", "{result}");
    let target = &result["report"]["isp_dispatch_target"];
    assert_eq!(target["verdict"], "above_target", "{result}");
    assert_eq!(target["target_ms"], 1.5);
    assert!(target["p95_ms"].as_f64().is_some_and(|p95| p95 > 1.5), "{result}");
}

#[test]
fn stage_budget_measures_each_row_applies_stage_sets_and_restores_the_product() {
    let config = short_config(|viewfinder, _| {
        viewfinder.stage_budget.rows = vec![
            StageBudgetRowSetting::Stage(IspStageSetting::Tone),
            StageBudgetRowSetting::WholeViewfinder,
        ];
    });
    let (harness, result) = run_viewfinder(
        config,
        "viewfinder-stage-budget",
        MirrorFaults::default(),
        Some(PowerPlug::Unplugged),
    );

    assert_eq!(result["status"], "passed", "{result}");
    let table: Vec<String> = result["report"]["table"]
        .as_array()
        .expect("таблица")
        .iter()
        .map(|line| line.as_str().unwrap_or_default().to_owned())
        .collect();
    assert_eq!(table.len(), 3, "две строки + «камера без ISP»: {table:?}");
    assert!(table[0].starts_with("tone | 0.10 / 0.10 | 20.0"), "{table:?}");
    assert!(table[0].ends_with("| measured"), "{table:?}");
    // Весь видоискатель: кадр ISP 1,8 + 0,6 мс против мозаики 0,7 мс.
    assert!(
        table[1].starts_with("whole_viewfinder | 1.70 / 1.70 | 20.0"),
        "{table:?}"
    );
    assert!(
        table[2].starts_with("camera_without_isp | 0.70 / 1.00 | 500.0"),
        "{table:?}"
    );

    let recorded = harness.control.recorded();
    let without_tone = IspStageSet::ALL.without(IspStage::Tone);
    assert!(
        recorded.contains(&RenderAdjustment::Viewfinder(ViewfinderCommand::SetIspStages(
            without_tone
        )))
    );
    assert!(
        recorded.contains(&RenderAdjustment::Viewfinder(ViewfinderCommand::SetPainter(
            ViewfinderPainter::DebugMosaic
        )))
    );
    // Конец сценария возвращает продукт: рисовальщик ISP и все стадии — последними.
    let tail = &recorded[recorded.len() - 2..];
    assert_eq!(
        tail,
        [
            RenderAdjustment::Viewfinder(ViewfinderCommand::SetPainter(ViewfinderPainter::Isp)),
            RenderAdjustment::Viewfinder(ViewfinderCommand::SetIspStages(IspStageSet::ALL)),
        ]
    );
    let rows: Vec<serde_json::Value> = stages_named(&harness, "row_started")
        .iter()
        .map(|stage| stage["row"].clone())
        .collect();
    assert_eq!(rows, [serde_json::json!("tone"), serde_json::json!("whole_viewfinder")]);
    assert_fits_logcat(&harness, &result);
}

#[test]
fn stage_budget_refuses_on_external_power_without_measuring_further_rows() {
    let config = short_config(|viewfinder, _| {
        viewfinder.stage_budget.rows = vec![
            StageBudgetRowSetting::Stage(IspStageSetting::Levels),
            StageBudgetRowSetting::Stage(IspStageSetting::Tone),
        ];
    });
    let (harness, result) = run_viewfinder(
        config,
        "viewfinder-stage-budget",
        MirrorFaults::default(),
        Some(PowerPlug::Usb),
    );

    assert_eq!(result["status"], "failed", "{result}");
    let error = result["error"].as_str().unwrap_or_default();
    assert!(error.contains("ток не измерен"), "{error}");
    assert!(error.contains("ExternalPower"), "{error}");
    assert_eq!(
        stages_named(&harness, "row_started").len(),
        1,
        "вторую строку не начинали"
    );
    // Первое же показание на зарядке отвергает замер: продукт остаётся (или возвращён) продуктом.
    let left = effective(&harness.control.recorded());
    assert_eq!((left.painter, left.stages), (ViewfinderPainter::Isp, IspStageSet::ALL));
}

#[test]
fn stage_budget_refuses_without_a_battery_source() {
    let (_harness, result) = run_viewfinder(
        short_config(|_, _| {}),
        "viewfinder-stage-budget",
        MirrorFaults::default(),
        None,
    );

    assert_eq!(result["status"], "failed", "{result}");
    assert!(
        result["error"]
            .as_str()
            .unwrap_or_default()
            .contains("опрос батареи не запущен"),
        "{result}"
    );
}

#[test]
fn endurance_records_every_minute_and_reports_no_throttling_on_a_calm_run() {
    let (harness, result) = run_viewfinder(
        short_config(|_, _| {}),
        "viewfinder-endurance",
        MirrorFaults::default(),
        None,
    );

    assert_eq!(result["status"], "passed", "{result}");
    let report = &result["report"];
    assert_eq!(report["expected_minutes"], 3);
    assert_eq!(report["throttling"]["found"], false);
    assert!(
        report["power_unavailable"].is_string(),
        "без батареи — честно: {report}"
    );
    assert_eq!(stages_named(&harness, "minute").len(), 3);
    let isp = stages_named(&harness, "minute")[0]["isp_median_ms"]
        .as_f64()
        .expect("ISP");
    assert!((isp - 1.8).abs() < 1e-9, "{isp}");
    assert_fits_logcat(&harness, &result);
}

#[test]
fn endurance_finds_throttling_when_the_sensor_is_slower_than_the_threshold() {
    let config = short_config(|viewfinder, _| viewfinder.endurance.min_sensor_fps = 61.0);
    let (_harness, result) = run_viewfinder(config, "viewfinder-endurance", MirrorFaults::default(), None);

    assert_eq!(result["status"], "passed", "троттлинг — находка, не провал: {result}");
    assert_eq!(result["report"]["throttling"]["found"], true);
    assert_eq!(result["report"]["throttling"]["signs"][0]["sign"], "sensor_fps_drop");
}

#[test]
fn headroom_bench_runs_at_120_hz_and_end_restores_bench_off_and_60_hz() {
    let (harness, result) = run_viewfinder(
        short_config(|_, _| {}),
        "video-headroom-bench",
        MirrorFaults::default(),
        None,
    );

    assert_eq!(result["status"], "passed", "{result}");
    let report = &result["report"];
    // 16:9@60 основной камеры идёт публичным диапазоном AE (4:3@60 — вендорским ключом, факт 00).
    assert_eq!(report["configured"]["lever"], "public_ae_target_range", "{report}");
    assert_eq!(report["bench"]["total"]["p95_ms"], 12.3);
    assert_eq!(report["fits_frame_budget"], true);
    assert_eq!(report["display"]["refresh_hz"], 120.0);
    assert!(report["bench"]["frames_recorded"].as_u64().expect("кадры") > 0);
    let started = stages_named(&harness, "bench_started");
    assert_eq!(started.len(), 1);
    assert_eq!(
        started[0]["tap_for_refresh"], true,
        "драйвер касается экрана ради 120 Гц"
    );
    assert_eq!(*harness.shell.frame_rates.lock().expect("замок"), [120.0, 60.0]);
    assert_eq!(
        harness.control.recorded(),
        [
            RenderAdjustment::Viewfinder(ViewfinderCommand::SetHeadroomBench(HeadroomBench::Enabled)),
            RenderAdjustment::Viewfinder(ViewfinderCommand::SetHeadroomBench(HeadroomBench::Disabled)),
        ]
    );
    assert_fits_logcat(&harness, &result);
}

#[test]
fn camera_failure_during_a_viewfinder_scenario_still_restores_the_render() {
    let (harness, result) = {
        let mut harness = harness_for_rig(FakeCameraRig::with_config(short_config(|_, _| {})));
        harness.deliver(ShellEvent::Resumed);
        // Рендер камеру не показывает вовсе: сценарий упадёт на ожидании первого кадра.
        let _pump = StreamPump::start(
            harness.rig(),
            Arc::clone(&harness.render),
            PumpFaults {
                render_shows_nothing: true,
                ..PumpFaults::default()
            },
        );
        harness.start(Scenario::parse("video-headroom-bench").expect("сценарий"));
        let deadline = Instant::now() + Duration::from_secs(10);
        while harness.output.results.lock().expect("замок").is_empty() && Instant::now() < deadline {
            harness.router.step(Duration::from_millis(2));
        }
        let result = harness.output.single_result();
        (harness, result)
    };

    assert_eq!(result["status"], "failed", "{result}");
    // Стенд не включался и частоту не трогали — возвращать нечего.
    assert!(harness.control.recorded().is_empty());
    assert!(harness.shell.frame_rates.lock().expect("замок").is_empty());
}

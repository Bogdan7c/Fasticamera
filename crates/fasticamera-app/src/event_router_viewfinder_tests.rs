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
    /// Как ведёт себя слой UI, пока сценарий держит источник (см. [`UiOverlayFault`]).
    ui_overlay: UiOverlayFault,
}

/// Поведение подставного слоя UI (сценарий `glass`).
#[derive(Clone, Copy, Default)]
enum UiOverlayFault {
    /// Здоров: кадр UI на каждый показанный кадр камеры, пирамида строится, GPU p95 1,8 мс.
    #[default]
    Healthy,
    /// GPU-время UI p95 9,3 мс при бюджете 4,17 мс.
    SlowGpu,
    /// Каждый кадр UI не рисуется (отказ рендера).
    DrawFails,
    /// Первые `WARMUP_FAILED_FRAMES` кадров UI после установки отказывают (разгон), дальше здоров.
    FailsWhileWarmingUp,
    /// Источник стоит, но UI не нарисовал ни одного кадра.
    NeverDraws,
}

/// Сколько первых кадров UI отказывает при [`UiOverlayFault::FailsWhileWarmingUp`].
const WARMUP_FAILED_FRAMES: u64 = 4;

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
    /// Маска подавленной работы GPU (16a): последняя команда `SuppressGpuWork`.
    suppression: render_loop::viewfinder::GpuWorkSuppression,
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
        suppression: render_loop::viewfinder::GpuWorkSuppression::NONE,
    };
    for adjustment in adjustments {
        match adjustment {
            RenderAdjustment::Viewfinder(ViewfinderCommand::SetPainter(painter)) => current.painter = *painter,
            RenderAdjustment::Viewfinder(ViewfinderCommand::SetIspStages(stages)) => current.stages = *stages,
            RenderAdjustment::Viewfinder(ViewfinderCommand::SetHeadroomBench(bench)) => current.bench = *bench,
            // Место, вид (ручки и основа) и приборы — настройки продукта, сценарий их не трогает и не откатывает.
            RenderAdjustment::Viewfinder(
                ViewfinderCommand::SetCrop(_)
                | ViewfinderCommand::SetFraming(_)
                | ViewfinderCommand::SetLook(_)
                | ViewfinderCommand::SetToneBase(_)
                | ViewfinderCommand::Scopes(_),
            ) => {}
            RenderAdjustment::Viewfinder(ViewfinderCommand::SuppressGpuWork(mask)) => current.suppression = *mask,
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
        // Место видоискателя — из конфига рендера (решение 93: экран камеры задаёт его командой `SetFraming`).
        placement: render_loop::viewfinder::EffectivePlacement::Configured(
            render_loop::config::ViewfinderPlacement::default(),
        ),
        stages: IspStageSet::ALL,
        look: std::sync::Arc::new(look_contract::LookSettings::NEUTRAL),
        tone_base: look_contract::ToneBaseModel::Agx,
        // Приборы закрыты: эти тесты их не касаются.
        scopes: Default::default(),
        gpu_work_suppression: render_loop::viewfinder::GpuWorkSuppression::NONE,
        pipeline_variants: Default::default(),
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

/// Как настоящий рендер ведёт диагностику UI-прохода по записанным командам: пока последняя команда —
/// `Install`, на каждый показанный кадр камеры рисуется кадр UI (или отказывает по `fault`).
fn mirror_ui_overlay(
    control: &crate::gpu_test_support::RecordingRenderControl,
    viewfinder: &mut ViewfinderDiagnostics,
    presented: u64,
    fault: UiOverlayFault,
    started_at: &mut Option<u64>,
) {
    use crate::gpu_test_support::RecordedUiOverlay;
    let commands = control.recorded_ui_overlay();
    let overlay = &mut viewfinder.ui_overlay;
    overlay.installs = commands
        .iter()
        .filter(|command| matches!(command, RecordedUiOverlay::Install(_)))
        .count() as u64;
    overlay.removals = commands
        .iter()
        .filter(|command| matches!(command, RecordedUiOverlay::Remove))
        .count() as u64;
    let Some(RecordedUiOverlay::Install(name)) = commands.last() else {
        overlay.active_source = None;
        *started_at = None;
        return;
    };
    overlay.active_source = Some((*name).to_owned());
    let drawn_since_install = presented.saturating_sub(*started_at.get_or_insert(presented));
    match fault {
        UiOverlayFault::Healthy | UiOverlayFault::SlowGpu => {
            overlay.frames_drawn = drawn_since_install;
            overlay.pyramid_rebuilds = drawn_since_install;
            overlay.gpu_time = Some(micros(if matches!(fault, UiOverlayFault::SlowGpu) {
                9_000
            } else {
                1_500
            }));
        }
        UiOverlayFault::FailsWhileWarmingUp => {
            let failed = drawn_since_install.min(WARMUP_FAILED_FRAMES);
            overlay.frames_failed = failed;
            overlay.frames_drawn = drawn_since_install - failed;
            overlay.last_error = (failed > 0).then(|| "первый кадр UI: рендер ещё создаётся".to_owned());
            overlay.gpu_time = Some(micros(1_500));
            overlay.pyramid_rebuilds = overlay.frames_drawn;
        }
        UiOverlayFault::NeverDraws => {
            overlay.last_error = Some("рендер UI не создан".to_owned());
        }
        UiOverlayFault::DrawFails => {
            overlay.frames_failed = drawn_since_install;
            overlay.last_error = Some("запись кадра UI: превышен предел кадра".to_owned());
            overlay.gpu_time = Some(micros(1_500));
            overlay.pyramid_rebuilds = drawn_since_install;
        }
    }
}

impl RenderMirror {
    fn start(harness: &Harness, faults: MirrorFaults) -> Self {
        Self::start_with_surface(harness, faults, None)
    }

    /// То же, плюс модель продуктового экрана 16a (подавление работы GPU, приборы, частота от касания); `None` —
    /// прежнее поведение зеркала.
    fn start_with_surface(
        harness: &Harness,
        faults: MirrorFaults,
        mut surface: Option<surface_support::SurfaceMirror>,
    ) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let render = Arc::clone(&harness.render);
        let control = Arc::clone(&harness.control);
        let shell = Arc::clone(&harness.shell);
        let join = thread::spawn(move || {
            let mut bench_started_at: Option<u64> = None;
            // Сколько кадров камеры было показано к моменту установки источника UI.
            let mut overlay_started_at: Option<u64> = None;
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
                    if let Some(viewfinder) = status.backend.viewfinder.as_mut() {
                        mirror_ui_overlay(
                            &control,
                            viewfinder,
                            presented,
                            faults.ui_overlay,
                            &mut overlay_started_at,
                        );
                    }
                    status.present_target.frame_gpu_time = Some(micros(9_000));
                    status.present_target.selection = now.selection;
                    status.present_target.frames_planned = presented;
                    status.present_target.vsync_shift.late_by_periods[0] = presented;
                    if let Some(display) = &mut status.display {
                        display.refresh_cycle = Some(Duration::from_secs_f64(1.0 / f64::from(refresh_hz)));
                    }
                    if let (Some(surface), Some(viewfinder)) = (surface.as_mut(), status.backend.viewfinder.as_mut()) {
                        let presented = status
                            .backend
                            .camera
                            .as_ref()
                            .map_or(0, |camera| camera.camera_frames_presented);
                        surface.apply(&control, viewfinder, presented);
                        if let Some(display) = &mut status.display {
                            display.refresh_cycle = Some(Duration::from_secs_f64(
                                1.0 / surface.effective_refresh_hz(f64::from(refresh_hz)),
                            ));
                        }
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
        let shell = Arc::clone(&harness.shell);
        let join = thread::spawn(move || {
            while !thread_stop.load(Ordering::SeqCst) {
                let now = effective(&control.recorded());
                // Запрошенные 120 Гц стоят телефону ещё 10 мА (строка `display_120hz`, 16a); без запроса — как раньше.
                let high_refresh = shell
                    .frame_rates
                    .lock()
                    .expect("замок")
                    .last()
                    .is_some_and(|hertz| *hertz >= 120.0);
                let product = now.painter == ViewfinderPainter::Isp
                    && now.stages == IspStageSet::ALL
                    && now.suppression.is_none();
                let discharge_ma = if product {
                    CURRENT_WITH_STAGE_MA
                } else {
                    CURRENT_WITHOUT_STAGE_MA
                } + if high_refresh { 10 } else { 0 };
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
    // Порог «профили различаются по к/с» обязан быть больше кванта измерения: в окне 200 мс один кадр на краю
    // окна — это 1 / 0,2 с = 5 к/с. С продуктовым порогом 0,5 к/с (он рассчитан на окна в десятки секунд)
    // зеркальный источник на 60 к/с давал ±0,8 к/с от дрожания границы окна и вердикт «лучше/хуже» вместо
    // «разницы нет» (~10 % прогонов под нагрузкой). Порог = один кадр на окно.
    // 5 к/с = один кадр на окно 200 мс (выше); меняешь окно — меняй и порог.
    viewfinder.latency.min_fps_delta = 5.0;
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

#[path = "event_router_capture_tests.rs"]
mod capture;
#[path = "event_router_capture_support.rs"]
mod capture_support;
#[path = "event_router_glass_tests.rs"]
mod glass;
#[path = "event_router_viewfinder_latency_tests.rs"]
mod latency;
#[path = "event_router_viewfinder_surface_support.rs"]
mod surface_support;
#[path = "event_router_zoom_tests.rs"]
mod zoom;

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
        report["gpu_time"]["isp_dispatch"]["p50_ms"],
        isp_micros(IspStageSet::ALL) as f64 / 1_000.0,
        "все {} стадий: 1 + N × 0,1 мс",
        IspStage::ALL.len()
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
    // Статистика — на каждом 4-м кадре от начала потока, а окно отчёта начинается в произвольной фазе этого
    // счёта: на 18 кадров честно выходит 4 или 5 публикаций (пол или потолок от деления на 4).
    assert!(
        (processed / 4..=processed.div_ceil(4)).contains(&published),
        "статистика идёт на каждом 4-м кадре: {published} из {processed}: {report}"
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

#[path = "event_router_viewfinder_budget_tests.rs"]
mod budget;
#[path = "event_router_viewfinder_surface_tests.rs"]
mod surface;

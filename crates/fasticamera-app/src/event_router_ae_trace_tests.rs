//! Функциональные тесты сценария `ae-trace` (сессия 08c): настоящий маршрутизатор, настоящий движок камеры на
//! fake-платформе, подставной HAL ([`SimulatedHal`]) и подставной рендер нашего AE ([`FakeRenderAe`] ведёт цель,
//! [`AeDiagnosticsMirror`] публикует диагностику AE в статус рендера). «Владелец» — цикл теста: на каждую подсказку
//! «ЩЁЛКНИ ЛАМПОЙ» (вид `Act` в журнале fake-экрана подсказок) он меняет яркость сцены на ±1 EV.
//!
//! Каждый тест доходит до строки результата: ряд записан, ступеньки найдены, профиль переключался и вернулся.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::{self, JoinHandle};

use camera_contract::three_a::ThreeAProfile;
use isp_vulkan::viewfinder::IspStageSet;
use render_loop::config::WhiteBalanceConfig;
use render_loop::viewfinder::{
    AutoExposureCounters, AutoExposureDiagnostics, HeadroomBenchDiagnostics, ViewfinderColorSpace, ViewfinderCounters,
    ViewfinderDiagnostics, ViewfinderExtent, ViewfinderGpuTimes, ViewfinderPainter, ViewfinderStorage,
    WhiteBalanceDiagnostics,
};
use shell_contract::commands::CommandOutcome;
use shell_contract::operator_prompt::OperatorPromptKind;

use crate::operator_prompt_fake::PromptEvent;

use super::*;
use crate::ae_trace_scenario_config::AeTraceScenarioConfig;
use crate::auto_exposure_link::ExposureControllerKind;
use crate::camera_config::CameraConfig;
use crate::camera_scenario_config::CameraScenarioConfig;
use crate::camera_test_support::WAIT;
use crate::fake_render_ae::FakeRenderAe;
use crate::raw_stream_test_support::{PumpFaults, StreamPump};
use crate::three_a_test_hal::{HalFaults, SimulatedHal};
use crate::viewfinder_scenario_config::ViewfinderScenarioConfig;

/// Яркость сцены до подсказки и после неё, с·ISO: ровно +1 EV (комнатный свет, внутри пределов камеры).
const SCENE_BEFORE: f64 = 2.0;
const SCENE_AFTER: f64 = 4.0;

/// Как подставной рендер публикует диагностику AE.
#[derive(Clone, Copy)]
enum Publishing {
    /// Диагностика есть; `gain_ev` — подтяжка ISP, которую рендер «показывает» на каждом кадре.
    Diagnostics { gain_ev: f32 },
    /// Диагностики нет вовсе (рендер без нашего AE).
    Nothing,
}

/// Подставной поток рендера: показывает кадры камеры (счётчики пишет `StreamPump`) и публикует диагностику AE
/// по яркости сцены, которую меняет «владелец».
struct AeDiagnosticsMirror {
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
    /// Яркость сцены, с·ISO, битами `f64`.
    scene_bits: Arc<AtomicU64>,
}

fn viewfinder_without_ae() -> ViewfinderDiagnostics {
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

fn ae_diagnostics(scene_seconds_iso: f64, gain_ev: f32, published: u64) -> AutoExposureDiagnostics {
    let scene_ev = scene_seconds_iso.log2();
    AutoExposureDiagnostics {
        directive_generation: Some(published),
        authority: camera_contract::auto_exposure::ExposureAuthority::Automatic,
        metered_area: camera_contract::auto_exposure::MeteredArea::WholeFrame,
        target_log2_seconds_iso: Some(scene_ev),
        observed_scene_log2_seconds_iso: Some(scene_ev),
        display_gain_ev: gain_ev,
        // Показанный кадр в этом зеркале всегда на уровне сцены: экран = сцена + подтяжка.
        displayed_frame_log2_seconds_iso: Some(scene_ev),
        convergence: Some(camera_contract::auto_exposure::ProposalConvergence::Settled),
        quality: None,
        last_proposal: None,
        counters: AutoExposureCounters {
            proposals_published: published,
            ..AutoExposureCounters::default()
        },
    }
}

impl AeDiagnosticsMirror {
    fn start(harness: &Harness, publishing: Publishing) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let scene_bits = Arc::new(AtomicU64::new(SCENE_BEFORE.to_bits()));
        let (thread_stop, thread_scene) = (Arc::clone(&stop), Arc::clone(&scene_bits));
        let render = Arc::clone(&harness.render);
        let join = thread::spawn(move || {
            let mut published = 0_u64;
            while !thread_stop.load(Ordering::SeqCst) {
                let scene = f64::from_bits(thread_scene.load(Ordering::SeqCst));
                render.update(|status| {
                    let viewfinder = status.backend.viewfinder.get_or_insert_with(viewfinder_without_ae);
                    viewfinder.auto_exposure = match publishing {
                        Publishing::Diagnostics { gain_ev } => {
                            published += 1;
                            Some(ae_diagnostics(scene, gain_ev, published))
                        }
                        Publishing::Nothing => None,
                    };
                });
                thread::sleep(Duration::from_millis(1));
            }
        });
        Self {
            stop,
            join: Some(join),
            scene_bits,
        }
    }

    fn set_scene(&self, scene_seconds_iso: f64) {
        self.scene_bits.store(scene_seconds_iso.to_bits(), Ordering::SeqCst);
    }
}

impl Drop for AeDiagnosticsMirror {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

/// Короткое расписание: фазы по 1,6 / 1,6 / 1,2 с, подсказка на 0,8 с каждой фазы с подсказками.
fn short_ae_trace() -> AeTraceScenarioConfig {
    AeTraceScenarioConfig {
        photo_phase_ms: 1_600,
        video_phase_ms: 1_600,
        static_phase_ms: 1_200,
        cue_interval_ms: 800,
        // Отсчёт 2 x 100 мс + надпись 200 мс помещаются в период 800 мс.
        cue_countdown_steps: 2,
        cue_countdown_step_ms: 100,
        cue_display_ms: 200,
        static_warmup_ms: 400,
        sample_interval_ms: 5,
        step_window_ms: 300,
        min_window_samples: 5,
        ae_diagnostics_timeout_ms: 300,
        detail_decimation_ms: 100,
        series_capacity: 4_096,
        ..AeTraceScenarioConfig::default()
    }
}

fn config_with(controller: ExposureControllerKind, tune: impl FnOnce(&mut AeTraceScenarioConfig)) -> CameraConfig {
    let viewfinder = ViewfinderScenarioConfig {
        streaming_timeout_ms: 2_000,
        render_live_timeout_ms: 1_000,
        viewfinder_settle_ms: 50,
        ..ViewfinderScenarioConfig::default()
    };
    let mut ae_trace = short_ae_trace();
    tune(&mut ae_trace);
    CameraConfig {
        exposure_controller: controller,
        scenarios: CameraScenarioConfig {
            viewfinder,
            ae_trace,
            ..CameraScenarioConfig::default()
        },
        ..CameraConfig::default()
    }
}

/// Сколько подсказок «ЩЁЛКНИ ЛАМПОЙ» (вид `Act`) уже показано на экране.
fn cues_shown(harness: &Harness) -> usize {
    harness
        .shell
        .prompts
        .shown_kinds()
        .into_iter()
        .filter(|kind| *kind == OperatorPromptKind::Act)
        .count()
}

/// Прогон `ae-trace` до итога. «Владелец» меняет сцену на каждую подсказку: чётные подсказки светлее, нечётные темнее.
fn run_ae_trace(
    config: CameraConfig,
    publishing: Publishing,
    owner_follows_cues: bool,
) -> (Harness, serde_json::Value) {
    run_ae_trace_with_prompt_outcome(config, publishing, owner_follows_cues, None)
}

/// То же, но экран подсказок отвечает заданным исходом (`None` — «поставлено в очередь»).
fn run_ae_trace_with_prompt_outcome(
    config: CameraConfig,
    publishing: Publishing,
    owner_follows_cues: bool,
    prompt_outcome: Option<CommandOutcome>,
) -> (Harness, serde_json::Value) {
    let rig = FakeCameraRig::with_config(config);
    let own_ae = rig.services.exposure_controller().effective == ExposureControllerKind::Own;
    let mut harness = harness_for_rig(rig);
    harness.shell.prompts.set_outcome(prompt_outcome);
    harness.deliver(ShellEvent::Resumed);
    let paced = PumpFaults {
        paced_at_sensor_rate: true,
        results_from_external_hal: true,
        ..PumpFaults::default()
    };
    let _pump = StreamPump::start(harness.rig(), Arc::clone(&harness.render), paced);
    let _hal = SimulatedHal::start(harness.rig(), HalFaults::default());
    let mirror = AeDiagnosticsMirror::start(&harness, publishing);
    let render_ae = own_ae.then(|| FakeRenderAe::start(harness.rig()));
    harness.start(Scenario::parse("ae-trace").expect("сценарий зарегистрирован"));
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut last_cues = 0;
    while harness.output.results.lock().expect("замок").is_empty() && Instant::now() < deadline {
        harness.router.step(Duration::from_millis(2));
        let cues = cues_shown(&harness);
        if owner_follows_cues && cues != last_cues {
            last_cues = cues;
            let scene = if cues % 2 == 1 { SCENE_AFTER } else { SCENE_BEFORE };
            mirror.set_scene(scene);
            if let Some(render_ae) = &render_ae {
                render_ae.change_scene(scene);
            }
        }
    }
    let result = harness.output.single_result();
    (harness, result)
}

fn error_of(result: &serde_json::Value) -> String {
    result["error"].as_str().unwrap_or_default().to_owned()
}

/// Движок вернулся на профиль «Фото».
fn assert_profile_restored(harness: &Harness) {
    let status = harness
        .rig()
        .handle()
        .wait_until_status(WAIT, |status| status.three_a.profile == ThreeAProfile::Photo);
    assert!(
        status.is_some(),
        "профиль не вернулся: {:?}",
        harness.rig().handle().status().three_a.profile
    );
}

#[test]
fn full_run_finds_the_real_steps_of_both_profiles_and_returns_the_photo_profile() {
    let (harness, result) = run_ae_trace(
        config_with(ExposureControllerKind::Own, |_| {}),
        Publishing::Diagnostics { gain_ev: 0.0 },
        true,
    );

    assert_eq!(result["status"], "passed", "{result}");
    let report = &result["report"];
    assert_eq!(
        report["real_steps"], 2,
        "по одной настоящей ступеньке в фото и видео: {report}"
    );
    let phases = report["phases"].as_array().expect("фазы");
    let names: Vec<&str> = phases.iter().filter_map(|phase| phase["phase"].as_str()).collect();
    assert_eq!(names, vec!["photo", "video", "static"]);
    // Ступенька фото: сцена темнее на 1 EV (нужно больше света), видео: обратно на -1 EV.
    let photo = &phases[0]["steps"][0];
    let video = &phases[1]["steps"][0];
    assert_eq!(photo[1], "measured", "{photo}");
    assert_eq!(video[1], "measured", "{video}");
    assert!((photo[2].as_f64().expect("размер") - 1.0).abs() < 0.05, "{photo}");
    assert!((video[2].as_f64().expect("размер") + 1.0).abs() < 0.05, "{video}");
    // Сенсор действительно пошёл за сценой: время установки известно и укладывается в отрезок 800 мс.
    for step in [photo, video] {
        let settle = step[4]
            .as_u64()
            .unwrap_or_else(|| panic!("сенсор не установился: {step}"));
        assert!(settle < 800, "установка {settle} мс: {step}");
    }
    // Движок держал нужные профили: иначе числа видео были бы числами фото.
    assert!(
        phases.iter().all(|phase| phase["profile_confirmed"] == true),
        "{phases:?}"
    );
    assert_eq!(phases[1]["profile"], "video");
    // Экран подсказок: на каждую фазу своя надпись, перед каждой подсказкой отсчёт, после неё «жди»; в конце
    // сценария маршрутизатор снимает подсказку, и она снимается ПОСЛЕ последней показанной.
    let headlines: Vec<String> = harness
        .shell
        .prompts
        .shown()
        .iter()
        .map(|prompt| prompt.headline().to_owned())
        .collect();
    assert_eq!(
        headlines,
        vec![
            "ФАЗА 1 ИЗ 3: ФОТО",
            "ПРИГОТОВЬСЯ… 2",
            "ПРИГОТОВЬСЯ… 1",
            "ЩЁЛКНИ ЛАМПОЙ",
            "ЖДИ СЛЕДУЮЩЕЙ СТУПЕНЬКИ",
            "ФАЗА 2 ИЗ 3: ВИДЕО",
            "ПРИГОТОВЬСЯ… 2",
            "ПРИГОТОВЬСЯ… 1",
            "ЩЁЛКНИ ЛАМПОЙ",
            "ЖДИ СЛЕДУЮЩЕЙ СТУПЕНЬКИ",
            "ФАЗА 3 ИЗ 3: СТАТИКА",
        ]
    );
    assert_eq!(harness.shell.prompts.events().last(), Some(&PromptEvent::Cleared));
    assert!(
        harness.shell.haptics_played.lock().expect("замок").is_empty(),
        "вибрация как сигнал владельцу больше не используется"
    );
    assert_eq!(report["prompts"]["queued"], 11);
    assert_eq!(report["prompts"]["failed"]["count"], 0);
    assert_eq!(phases[2]["quiet"]["analyzed_ms"], 800);
    assert_profile_restored(&harness);
}

#[test]
fn full_run_writes_the_series_into_detail_lines_before_the_result() {
    let (harness, _result) = run_ae_trace(
        config_with(ExposureControllerKind::Own, |_| {}),
        Publishing::Diagnostics { gain_ev: 0.0 },
        true,
    );

    let stages = harness.output.stages.lock().expect("замок").clone();
    let details: Vec<&serde_json::Value> = stages
        .iter()
        .filter(|stage| stage["stage"] == "detail_ae_trace" && stage["scenario"] == "ae-trace")
        .collect();
    assert!(
        !details.is_empty(),
        "подробностей нет: {:?}",
        harness.output.stage_names()
    );
    let items: Vec<&serde_json::Value> = details
        .iter()
        .flat_map(|stage| stage["items"].as_array().expect("элементы"))
        .collect();
    assert!(items[0].get("legend").is_some(), "первая запись — легенда");
    // 4,4 с с шагом 100 мс — порядка 44 строк ряда.
    assert!(items.len() > 30, "строк ряда мало: {}", items.len());
    let started = harness
        .output
        .stage_names()
        .iter()
        .position(|name| name == "trace_started");
    assert!(
        started.is_some(),
        "стадия начала записи: {:?}",
        harness.output.stage_names()
    );
}

#[test]
fn owner_who_misses_every_cue_gets_no_step_rows_and_fails_as_not_accepted() {
    // Каждая пропущенная подсказка — `no_step` без суда, но прогон без единой ступеньки — не приёмка.
    let (_harness, result) = run_ae_trace(
        config_with(ExposureControllerKind::Own, |_| {}),
        Publishing::Diagnostics { gain_ev: 0.0 },
        false,
    );

    assert_eq!(result["status"], "failed", "{result}");
    assert!(error_of(&result).contains("ни одной настоящей ступеньки"), "{result}");
    assert_eq!(result["report"]["real_steps"], 0, "{result}");
    assert_eq!(result["report"]["phases"][0]["steps"][0][1], "no_step", "{result}");
}

#[test]
fn residual_gain_above_the_controller_limit_fails_the_run() {
    // Рендер «показывает» подтяжку 0,9 EV при пределе контроллера 0,5 EV.
    let (harness, result) = run_ae_trace(
        config_with(ExposureControllerKind::Own, |_| {}),
        Publishing::Diagnostics { gain_ev: 0.9 },
        true,
    );

    assert_eq!(result["status"], "failed", "{result}");
    assert!(error_of(&result).contains("подтяжка ISP"), "{result}");
    assert!(
        result["report"]["max_abs_gain_ev"]
            .as_f64()
            .is_some_and(|gain| (gain - 0.9).abs() < 1e-6),
        "{result}"
    );
    assert_profile_restored(&harness);
}

#[test]
fn render_without_ae_diagnostics_fails_with_an_explicit_reason_and_restores_the_profile() {
    let (harness, result) = run_ae_trace(
        config_with(ExposureControllerKind::Own, |_| {}),
        Publishing::Nothing,
        true,
    );

    assert_eq!(result["status"], "failed", "{result}");
    assert!(error_of(&result).contains("не публикует его диагностику"), "{result}");
    assert!(
        harness.shell.prompts.shown().is_empty(),
        "без записи владельца не беспокоим"
    );
    assert_eq!(
        harness.shell.prompts.clear_count(),
        1,
        "подсказку снимают и при провале"
    );
    assert_profile_restored(&harness);
}

#[test]
fn vendor_ae_is_refused_at_start_with_the_reason_and_never_claims_the_camera() {
    let (harness, result) = run_ae_trace(
        config_with(ExposureControllerKind::Vendor, |_| {}),
        Publishing::Diagnostics { gain_ev: 0.0 },
        true,
    );

    assert_eq!(result["status"], "failed", "{result}");
    assert!(error_of(&result).contains("вендорский AE"), "{result}");
    assert!(harness.shell.prompts.shown().is_empty());
    assert_eq!(
        harness.shell.prompts.clear_count(),
        1,
        "подсказку снимают и при отказе на старте"
    );
}

#[test]
fn prompts_that_cannot_be_shown_fail_the_run_with_a_clear_reason() {
    let (harness, result) = run_ae_trace_with_prompt_outcome(
        config_with(ExposureControllerKind::Own, |_| {}),
        Publishing::Diagnostics { gain_ev: 0.0 },
        true,
        Some(CommandOutcome::NotAttached),
    );

    assert_eq!(result["status"], "failed", "{result}");
    assert!(error_of(&result).contains("владельцу не показан сигнал"), "{result}");
    assert!(error_of(&result).contains("NotAttached"), "{result}");
    assert_eq!(result["report"]["prompts"]["queued"], 0, "{result}");
    assert_eq!(harness.shell.prompts.events().last(), Some(&PromptEvent::Cleared));
}

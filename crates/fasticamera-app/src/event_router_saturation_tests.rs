//! Сценарий `gpu-saturation-bench` через настоящий маршрутизатор и настоящий движок камеры (платформа — fake).
//!
//! GPU подставной: исполнитель работ отвечает по сценарию теста (цифра, нет кадров, провал, ожидание отмены), поэтому
//! проверяются порядок, стадии, тишина камеры, бюджет строки результата, отмена и вердикт — без Vulkan.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use camera_contract::lens_shading::LensShadingCalibrations;
use camera_core::CameraState;
use gpu_vulkan::saturation::CancelToken;

use super::*;
use crate::camera_config::CameraConfig;
use crate::camera_scenario_config::CameraScenarioConfig;
use crate::camera_test_support::{FakeCameraRig, WAIT};
use crate::logging::record::LOGCAT_MAX_TEXT_BYTES;
use crate::result_line::{RESULT_LINE_PREFIX, STAGE_LINE_PREFIX};
use crate::saturation_bench::catalog::{CatalogEntry, all_names};
use crate::saturation_bench::report::{EndReasonName, FailureKind, Measurement, WorkloadFailure};
use crate::saturation_bench::runner::{ExecutedWorkload, WorkloadExecutor};
use crate::saturation_bench::services::{ExecutorSetup, SaturationGpu, SaturationServices};
use crate::saturation_scenario_config::SaturationScenarioConfig;

/// Что делает подставная работа.
#[derive(Clone)]
enum Behavior {
    Frames(f64),
    NoFrames,
    Fail(String),
    /// Ждёт отмены токеном и отдаёт частичный итог; `running` сообщает тесту, что работа началась.
    WaitsForCancel,
}

/// Общее состояние подставного GPU: что исполнено, что видела камера на старте каждой работы.
#[derive(Default)]
struct SaturationLog {
    executed: Mutex<Vec<&'static str>>,
    camera_states_at_start: Mutex<Vec<&'static str>>,
    running: AtomicBool,
}

struct FakeSaturationGpu {
    behaviors: HashMap<&'static str, Behavior>,
    default: Behavior,
    log: Arc<SaturationLog>,
    camera: Option<camera_core::CameraHandle>,
}

struct FakeExecutor {
    behaviors: HashMap<&'static str, Behavior>,
    default: Behavior,
    log: Arc<SaturationLog>,
    camera: Option<camera_core::CameraHandle>,
}

impl SaturationGpu for FakeSaturationGpu {
    fn executor(&self, _setup: ExecutorSetup) -> Result<Box<dyn WorkloadExecutor>, String> {
        Ok(Box::new(FakeExecutor {
            behaviors: self.behaviors.clone(),
            default: self.default.clone(),
            log: Arc::clone(&self.log),
            camera: self.camera.clone(),
        }))
    }

    fn validation(&self) -> ValidationState {
        ValidationState::Disabled
    }
}

fn measurement(fps: Option<f64>, end_reason: EndReasonName) -> Measurement {
    Measurement {
        end_reason,
        warmup_frames: 3,
        measured_frames: fps.map_or(0, |value| value as u64),
        drained_frames: 0,
        window_ms: 1_000.0,
        frames_per_second: fps,
        frame_cost_ms: fps.map(|value| 1_000.0 / value),
        passes: Vec::new(),
        unmeasured_pass_frames: 0,
        mismatched_pass_frames: 0,
        timeline_fps: fps.into_iter().collect(),
    }
}

impl WorkloadExecutor for FakeExecutor {
    fn execute(&mut self, entry: &CatalogEntry, cancel: &CancelToken) -> Result<ExecutedWorkload, WorkloadFailure> {
        self.log.executed.lock().expect("замок").push(entry.name);
        if let Some(camera) = &self.camera {
            self.log
                .camera_states_at_start
                .lock()
                .expect("замок")
                .push(camera.status().state.name());
        }
        let done = |measurement| {
            Ok(ExecutedWorkload {
                measurement,
                scene: None,
                ui: None,
            })
        };
        match self.behaviors.get(entry.name).unwrap_or(&self.default).clone() {
            Behavior::Frames(fps) => done(measurement(Some(fps), EndReasonName::Completed)),
            Behavior::NoFrames => done(measurement(None, EndReasonName::Completed)),
            Behavior::Fail(reason) => Err(WorkloadFailure::new(FailureKind::BuildFailed, reason)),
            Behavior::WaitsForCancel => {
                self.log.running.store(true, Ordering::SeqCst);
                let deadline = Instant::now() + Duration::from_secs(10);
                while !cancel.is_cancelled() && Instant::now() < deadline {
                    thread::sleep(Duration::from_millis(2));
                }
                done(measurement(Some(5.0), EndReasonName::Cancelled))
            }
        }
    }
}

fn config(names: &[&str]) -> SaturationScenarioConfig {
    SaturationScenarioConfig {
        workloads: names.iter().map(|name| (*name).to_owned()).collect(),
        cooldown_between_ms: 0,
        quiet_settle_ms: 1,
        camera_close_timeout_ms: 3_000,
        ..SaturationScenarioConfig::default()
    }
}

struct Setup {
    harness: Harness,
    log: Arc<SaturationLog>,
}

fn setup_with(
    saturation: SaturationScenarioConfig,
    default: Behavior,
    behaviors: Vec<(&'static str, Behavior)>,
    product_viewfinder: bool,
) -> Setup {
    let rig = if product_viewfinder {
        let mut camera = CameraConfig::default();
        camera.viewfinder_autostart.enabled = true;
        camera.scenarios = CameraScenarioConfig {
            saturation,
            ..CameraScenarioConfig::default()
        };
        FakeCameraRig::with_product_config(camera)
    } else {
        FakeCameraRig::with_scenarios(CameraScenarioConfig {
            saturation,
            ..CameraScenarioConfig::default()
        })
    };
    let handle = rig.handle();
    let mut harness = harness_for_rig(rig);
    let log = Arc::new(SaturationLog::default());
    let gpu = FakeSaturationGpu {
        behaviors: behaviors.into_iter().collect(),
        default,
        log: Arc::clone(&log),
        camera: Some(handle),
    };
    harness.router.gpu = harness
        .router
        .gpu
        .clone()
        .with_saturation(SaturationServices::new(Arc::new(gpu), LensShadingCalibrations::none()));
    Setup { harness, log }
}

fn setup(names: &[&str], default: Behavior) -> Setup {
    setup_with(config(names), default, Vec::new(), false)
}

fn run(setup: &mut Setup) -> serde_json::Value {
    setup
        .harness
        .start(Scenario::parse("gpu-saturation-bench").expect("сценарий зарегистрирован"));
    setup.harness.run_until_result();
    setup.harness.output.single_result()
}

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

fn assert_fits_logcat(harness: &Harness, result: &serde_json::Value) {
    let result_bytes = RESULT_LINE_PREFIX.len() + result.to_string().len();
    assert!(
        result_bytes <= LOGCAT_MAX_TEXT_BYTES,
        "строка результата {result_bytes} байт"
    );
    for line in harness.output.stages.lock().expect("замок").iter() {
        let bytes = STAGE_LINE_PREFIX.len() + line.to_string().len();
        assert!(
            bytes <= LOGCAT_MAX_TEXT_BYTES,
            "строка стадии {line} занимает {bytes} байт"
        );
    }
}

#[test]
fn scenario_is_registered_observing_and_available() {
    let scenario = Scenario::parse("gpu-saturation-bench").expect("зарегистрирован");
    assert_eq!(scenario.mode(), crate::scenario::ScenarioMode::Observing);
    assert_eq!(
        scenario.observing().expect("наблюдающий").name(),
        "gpu-saturation-bench"
    );
}

#[test]
fn every_work_gets_a_start_and_finish_stage_and_the_result_carries_the_compact_table() {
    let mut setup = setup(&["viewfinder", "compositor", "still"], Behavior::Frames(250.0));
    // Каталог камер читается движком в фоне: ждём его, чтобы сенсор брался из него, а не из запасного набора.
    setup
        .harness
        .rig()
        .handle()
        .wait_until_status(WAIT, |status| !status.catalog.cameras().is_empty())
        .expect("каталог камер прочитан");
    let result = run(&mut setup);

    assert_eq!(result["status"], "passed", "{result}");
    assert_eq!(
        *setup.log.executed.lock().expect("замок"),
        ["viewfinder", "compositor", "still"]
    );
    let started = stages_named(&setup.harness, "workload_started");
    let names: Vec<&str> = started.iter().map(|line| line["name"].as_str().expect("имя")).collect();
    assert_eq!(names, ["viewfinder", "compositor", "still"]);
    assert_eq!(
        (started[1]["index"].clone(), started[1]["total"].clone()),
        (2.into(), 3.into())
    );
    assert!(
        started[0]["offset_ms"].is_f64(),
        "смещение от начала прогона — для совмещения журнала тепла"
    );
    assert!(started[0].get("thermal_status").is_some());
    assert_eq!(stages_named(&setup.harness, "workload_finished").len(), 3);
    assert_eq!(stages_named(&setup.harness, "bench_started").len(), 1);

    let report = &result["report"];
    assert_eq!(
        report["table"],
        serde_json::json!(["viewfinder|250|4.00|ok", "compositor|250|4.00|ok", "still|250|4.00|ok"])
    );
    assert_eq!(report["measured"], 3);
    assert_eq!(
        report["sensor"]["source"], "engine",
        "сенсор из каталога движка, а не запасной"
    );
    assert_eq!(report["sensor"]["camera"], "2", "основная камера");
    assert_eq!(report["render"]["delta"]["frames_presented"], 0);
    let details = stage_details(&setup.harness, "detail_rows");
    assert_eq!(details.len(), 3, "полная строка по каждой работе");
    assert_eq!(details[0]["outcome"]["state"], "measured");
    assert_fits_logcat(&setup.harness, &result);
}

/// Элементы строк стадии `stage`.
fn stage_details(harness: &Harness, stage: &str) -> Vec<serde_json::Value> {
    stages_named(harness, stage)
        .iter()
        .flat_map(|line| line["items"].as_array().cloned().unwrap_or_default())
        .collect()
}

#[test]
fn a_work_without_a_number_or_with_an_error_fails_the_verdict_but_the_others_still_run() {
    let mut setup = setup_with(
        config(&["viewfinder", "headroom_v4", "still", "compositor"]),
        Behavior::Frames(100.0),
        vec![
            ("headroom_v4", Behavior::Fail("не хватило памяти".to_owned())),
            ("still", Behavior::NoFrames),
        ],
        false,
    );
    let result = run(&mut setup);

    assert_eq!(result["status"], "failed", "{result}");
    assert_eq!(
        setup.log.executed.lock().expect("замок").len(),
        4,
        "ошибка одной работы не срывает остальные"
    );
    let error = result["error"].as_str().expect("ошибка");
    assert!(
        error.contains("headroom_v4") && error.contains("не хватило памяти"),
        "{error}"
    );
    assert!(
        error.contains("still") && error.contains("не завершился ни один кадр"),
        "{error}"
    );
    assert_eq!(
        result["report"]["table"],
        serde_json::json!([
            "viewfinder|100|10.0|ok",
            "headroom_v4|-|-|build_failed",
            "still|-|-|no_frames",
            "compositor|100|10.0|ok"
        ])
    );
}

#[test]
fn unknown_workload_name_is_a_config_error_before_anything_starts() {
    // Движок камеры проверяет конфиг сценариев при запуске: опечатка в имени работы не доживает до сценария.
    let broken = CameraScenarioConfig {
        saturation: config(&["viewfinder", "viewfinderr"]),
        ..CameraScenarioConfig::default()
    };
    let error = broken.validate().expect_err("опечатка в имени работы").to_string();
    assert!(error.contains("viewfinderr") && error.contains("доступны"), "{error}");
    let startup = CameraConfig {
        scenarios: broken,
        ..CameraConfig::default()
    };
    assert!(startup.validate().is_err(), "initialize отказал бы до первого запуска");
}

#[test]
fn the_longest_list_with_every_work_failed_and_long_reasons_still_fits_the_result_line() {
    let long_reason = "причина отказа ".repeat(80);
    let mut setup = setup_with(
        config(&all_names().iter().map(String::as_str).collect::<Vec<_>>()),
        Behavior::Fail(long_reason),
        Vec::new(),
        false,
    );
    let result = run(&mut setup);

    assert_eq!(result["status"], "failed");
    assert_eq!(
        result["report"]["table"].as_array().expect("таблица").len(),
        14,
        "строка на каждую работу"
    );
    assert_fits_logcat(&setup.harness, &result);
    let details = stage_details(&setup.harness, "detail_rows");
    assert_eq!(details.len(), 14);
    assert!(
        details[0]["outcome"]["failure"]["detail"]
            .as_str()
            .expect("причина")
            .len()
            > 1000,
        "полная причина — в подробностях, а не обрезана"
    );
}

#[test]
fn camera_is_closed_while_measuring_and_the_product_viewfinder_comes_back_after() {
    let mut setup = setup_with(
        config(&["viewfinder", "compositor"]),
        Behavior::Frames(60.0),
        Vec::new(),
        true,
    );
    // Продуктовый автозапуск: приложение на переднем плане, видоискатель идёт до сценария.
    setup.harness.deliver(ShellEvent::Resumed);
    setup.harness.rig().wait_streaming();

    let result = run(&mut setup);

    assert_eq!(result["status"], "passed", "{result}");
    assert_eq!(result["report"]["camera"], "closed_by_scenario");
    let states = setup.log.camera_states_at_start.lock().expect("замок").clone();
    assert_eq!(
        states,
        ["closed", "closed"],
        "на старте каждой работы камера закрыта, а не стримит"
    );
    // Маршрутизатор вернул видоискатель продукта по концу сценария.
    let status = setup
        .harness
        .rig()
        .handle()
        .wait_until_status(WAIT, |status| matches!(status.state, CameraState::Streaming { .. }))
        .expect("видоискатель продукта вернулся");
    assert!(matches!(status.state, CameraState::Streaming { .. }));
}

#[test]
fn leaving_the_app_cancels_the_run_keeps_the_partial_table_and_marks_the_rest_not_run() {
    let mut setup = setup_with(
        config(&["viewfinder", "compositor", "still"]),
        Behavior::Frames(100.0),
        vec![("compositor", Behavior::WaitsForCancel)],
        false,
    );
    setup
        .harness
        .start(Scenario::parse("gpu-saturation-bench").expect("сценарий зарегистрирован"));
    let deadline = Instant::now() + Duration::from_secs(5);
    while !setup.log.running.load(Ordering::SeqCst) && Instant::now() < deadline {
        setup.harness.router.step(Duration::from_millis(2));
    }
    assert!(setup.log.running.load(Ordering::SeqCst), "вторая работа началась");
    setup.harness.deliver(ShellEvent::Paused);
    setup.harness.run_until_result();
    let result = setup.harness.output.single_result();

    assert_eq!(result["status"], "failed", "{result}");
    assert_eq!(
        *setup.log.executed.lock().expect("замок"),
        ["viewfinder", "compositor"],
        "третья не стартовала"
    );
    assert_eq!(
        result["report"]["table"],
        serde_json::json!([
            "viewfinder|100|10.0|ok",
            "compositor|5.00|200|cancelled",
            "still|-|-|not_run"
        ])
    );
    assert_eq!(result["report"]["cancelled"], true);
    assert!(result["error"].as_str().expect("ошибка").contains("ушло в фон"));
}

#[test]
fn leaving_the_app_before_the_run_starts_is_a_clean_failure_without_any_work() {
    let mut setup = setup(&["viewfinder"], Behavior::Frames(1.0));
    setup
        .harness
        .start(Scenario::parse("gpu-saturation-bench").expect("сценарий зарегистрирован"));
    // Старт сценария обработан первым же шагом; уход в фон приходит до паузы «тишины камеры».
    setup.harness.deliver(ShellEvent::Paused);
    setup.harness.run_until_result();
    let result = setup.harness.output.single_result();
    assert_eq!(result["status"], "failed");
    assert!(result["error"].as_str().expect("ошибка").contains("до начала замера"));
    assert!(setup.log.executed.lock().expect("замок").is_empty());
}

#[test]
fn gpu_that_cannot_start_the_bench_is_a_failure_with_the_reason_not_a_hang() {
    // Подставной GPU не подключён: сервисы по умолчанию «недоступно».
    let rig = FakeCameraRig::with_scenarios(CameraScenarioConfig {
        saturation: config(&["viewfinder"]),
        ..CameraScenarioConfig::default()
    });
    let mut harness = harness_for_rig(rig);
    harness.start(Scenario::parse("gpu-saturation-bench").expect("сценарий зарегистрирован"));
    harness.run_until_result();
    let result = harness.output.single_result();
    assert_eq!(result["status"], "failed");
    let error = result["error"].as_str().expect("ошибка");
    assert!(
        error.contains("стенд не запущен") && error.contains("не подключён"),
        "{error}"
    );
}

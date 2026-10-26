//! Функциональные тесты сценариев камеры: настоящая очередь и маршрутизатор, настоящий движок камеры,
//! fake-платформа. Данные доходят до строки результата, которую читает скрипт smoke.

use std::sync::Arc;

use camera_android::BackendDiagnostics;
use camera_core::backend::BackendFailure;
use camera_core::fake::FakeBackendControl;

use super::*;
use crate::camera_config::CameraConfig;
use crate::camera_scenario_config::CameraScenarioConfig;
use crate::camera_service::BackendDiagnosticsSource;
use crate::camera_test_support::FakeCameraRig;
use crate::camera_wiring::{CameraBackendParts, start_camera};

fn services_over(control: &Arc<FakeBackendControl>, config: &CameraConfig) -> CameraServices {
    start_camera(
        config,
        CameraBackendParts {
            factory: control.factory(),
            diagnostics: Arc::new(BackendDiagnostics::new(4)) as Arc<dyn BackendDiagnosticsSource>,
        },
    )
}

fn run_catalog(harness: &mut Harness) -> serde_json::Value {
    harness.start(Scenario::parse("camera-catalog").expect("сценарий зарегистрирован"));
    harness.run_until_result();
    harness.output.single_result()
}

/// Элементы всех строк стадии `stage` (подробности отчёта), в порядке записи.
fn stage_items(harness: &Harness, stage: &str) -> Vec<serde_json::Value> {
    harness
        .output
        .stages
        .lock()
        .expect("замок")
        .iter()
        .filter(|line| line["stage"] == stage)
        .flat_map(|line| line["items"].as_array().cloned().unwrap_or_default())
        .collect()
}

/// Результат и каждая строка стадии помещаются в запись logcat: иначе скрипт получил бы вместо отчёта
/// «отчёт не помещается» (результат) или оборванный JSON (стадия).
fn assert_fits_logcat(harness: &Harness, result: &serde_json::Value) {
    use crate::logging::record::LOGCAT_MAX_TEXT_BYTES;
    use crate::result_line::{RESULT_LINE_PREFIX, STAGE_LINE_PREFIX};
    let result_bytes = RESULT_LINE_PREFIX.len() + result.to_string().len();
    assert!(
        result_bytes <= LOGCAT_MAX_TEXT_BYTES,
        "строка результата {result_bytes} байт"
    );
    for line in harness.output.stages.lock().expect("замок").iter() {
        let stage_bytes = STAGE_LINE_PREFIX.len() + line.to_string().len();
        assert!(
            stage_bytes <= LOGCAT_MAX_TEXT_BYTES,
            "строка стадии {line} занимает {stage_bytes} байт"
        );
    }
}

#[test]
fn catalog_of_the_phone_reaches_the_report_with_roles_and_backend_diagnostics() {
    let rig = FakeCameraRig::new();
    rig.diagnostics
        .record_failure("test.operation", -5, "для проверки доставки диагностики");
    let mut harness = harness_for_rig(rig);
    let result = run_catalog(&mut harness);

    assert_eq!(result["scenario"], "camera-catalog");
    assert_eq!(result["status"], "passed", "{result}");
    let report = &result["report"];
    let ids: Vec<&str> = report["cameras"]
        .as_array()
        .expect("cameras")
        .iter()
        .map(|camera| camera["id"].as_str().expect("id"))
        .collect();
    assert_eq!(ids, ["0", "1", "2", "3"]);
    let roles: Vec<&str> = report["roles"]
        .as_array()
        .expect("roles")
        .iter()
        .map(|entry| entry["role"].as_str().expect("role"))
        .collect();
    assert!(
        roles.contains(&"wide") && roles.contains(&"ultra_wide") && roles.contains(&"front"),
        "{roles:?}"
    );
    assert_eq!(report["backend"]["last_failure"]["operation"], "test.operation");
    assert!(report["event_count"].is_u64());

    // Полные характеристики и диагностика бэкенда — строками стадий до результата.
    let cameras = stage_items(&harness, "detail_cameras");
    assert_eq!(cameras.len(), 4);
    assert_eq!(cameras[2]["id"], "2");
    assert_eq!(cameras[2]["raw"]["supported"], true);
    let backend = stage_items(&harness, "detail_backend");
    assert_eq!(backend[0]["recent_failures"][0]["operation"], "test.operation");
    assert_fits_logcat(&harness, &result);
}

#[test]
fn catalog_read_failure_is_reported_at_once_with_the_cause_and_an_empty_report() {
    let control = FakeBackendControl::new();
    control.set_catalog_failure(Some(BackendFailure {
        operation: "ACameraManager_getCameraIdList",
        code: -1,
        detail: "нет доступа".to_owned(),
    }));
    let mut harness = harness_for_services(services_over(&control, &CameraConfig::default()));
    let result = run_catalog(&mut harness);

    assert_eq!(result["status"], "failed");
    let error = result["error"].as_str().expect("error");
    assert!(
        error.contains("окончательной ошибке") && error.contains("CatalogUnavailable"),
        "{error}"
    );
    assert_eq!(
        result["report"]["cameras"],
        serde_json::json!([]),
        "отчёт есть и при провале"
    );
}

#[test]
fn phone_with_no_cameras_times_out_with_a_clear_reason() {
    let control = FakeBackendControl::new();
    control.set_cameras(Vec::new());
    let config = CameraConfig {
        scenarios: CameraScenarioConfig {
            catalog_timeout_ms: 150,
            ..CameraScenarioConfig::default()
        },
        ..CameraConfig::default()
    };
    let mut harness = harness_for_services(services_over(&control, &config));
    let result = run_catalog(&mut harness);

    assert_eq!(result["status"], "failed");
    let error = result["error"].as_str().expect("error");
    assert!(error.contains("не прочитан за отведённое время"), "{error}");
    assert!(error.contains("каталог пуст"), "{error}");
}

#[test]
fn catalog_scenario_on_an_unavailable_engine_fails_with_its_reason() {
    let services = CameraServices::unavailable("нет камеры", CameraScenarioConfig::default());
    let mut harness = harness_for_services(services);
    let result = run_catalog(&mut harness);
    assert_eq!(result["status"], "failed");
    assert!(
        result["error"].as_str().expect("error").contains("нет камеры"),
        "{result}"
    );
}

// ---- camera-stream ----

use std::time::Duration as StdDuration;

use crate::camera_scenario_config::{
    CaptureResultsExpectation, ExpectedLever, RawFormatSetting, ReadoutSetting, StreamStepConfig,
};
use crate::camera_test_support::FrameFeeder;

const SIXTY_FPS: StdDuration = StdDuration::from_nanos(16_666_667);
const THIRTY_FPS: StdDuration = StdDuration::from_nanos(33_333_333);

fn step(
    camera_id: &str,
    raw_format: RawFormatSetting,
    readout: ReadoutSetting,
    frame_rate: u32,
    expected_lever: ExpectedLever,
    min_measured_fps: f64,
) -> StreamStepConfig {
    StreamStepConfig {
        camera_id: camera_id.to_owned(),
        raw_format,
        readout,
        frame_rate,
        expected_lever,
        min_measured_fps,
        capture_results: CaptureResultsExpectation::Required,
    }
}

/// Движок с короткими замерами и заданными шагами.
fn stream_rig(steps: Vec<StreamStepConfig>) -> FakeCameraRig {
    FakeCameraRig::with_scenarios(CameraScenarioConfig {
        measure_ms: 120,
        stream_start_timeout_ms: 3_000,
        stream_close_timeout_ms: 3_000,
        stream_steps: steps,
        ..CameraScenarioConfig::default()
    })
}

fn sixty_fps_matrix() -> Vec<StreamStepConfig> {
    use ExpectedLever::{PublicAeTargetRange, VendorSensorFrameDurationOverride};
    use RawFormatSetting::{Raw10, Raw16};
    use ReadoutSetting::{FourByThree, SixteenByNine};
    vec![
        step("2", Raw10, SixteenByNine, 60, PublicAeTargetRange, 57.0),
        step("2", Raw10, FourByThree, 60, VendorSensorFrameDurationOverride, 57.0),
        step("3", Raw10, FourByThree, 60, PublicAeTargetRange, 55.0),
        step("2", Raw16, SixteenByNine, 60, PublicAeTargetRange, 57.0),
    ]
}

/// Приложение на экране (движок уходит из фона), запускает `camera-stream` и ждёт итог.
fn run_stream(harness: &mut Harness) -> serde_json::Value {
    harness.deliver(ShellEvent::Resumed);
    harness.start(Scenario::parse("camera-stream").expect("сценарий зарегистрирован"));
    harness.run_until_result();
    harness.output.single_result()
}

fn step_reports(result: &serde_json::Value) -> &Vec<serde_json::Value> {
    result["report"]["steps"].as_array().expect("steps")
}

#[test]
fn every_step_is_opened_streamed_measured_and_judged_and_the_camera_ends_closed() {
    let mut harness = harness_for_rig(stream_rig(sixty_fps_matrix()));
    let _frames = FrameFeeder::start(harness.rig(), SIXTY_FPS);
    let result = run_stream(&mut harness);

    assert_eq!(result["status"], "passed", "{result}");
    let steps = step_reports(&result);
    assert_eq!(steps.len(), 4);
    let levers: Vec<&str> = steps
        .iter()
        .map(|step| step["lever"].as_str().expect("lever"))
        .collect();
    assert_eq!(
        levers,
        [
            "public_ae_target_range",
            "vendor_sensor_frame_duration_override",
            "public_ae_target_range",
            "public_ae_target_range"
        ]
    );
    // Что бэкенд реально сконфигурировал, видно в полных отчётах шагов (строки стадии detail_steps).
    let details = stage_items(&harness, "detail_steps");
    let cameras: Vec<&str> = details
        .iter()
        .map(|step| step["configured"]["camera"].as_str().expect("camera"))
        .collect();
    assert_eq!(cameras, ["2", "2", "3", "2"], "каждый шаг открыл свою камеру");
    assert_eq!(details[3]["configured"]["raw_format"], "raw16");
    assert_eq!(details[0]["stats"]["results_completed"], steps[0]["results_completed"]);
    assert_fits_logcat(&harness, &result);
    for step in steps {
        let fps = step["measured_fps"].as_f64().expect("fps");
        assert!((59.0..=61.0).contains(&fps), "измеренная частота {fps}");
        assert!(
            step["raw_sink"]["frames_acquired"].as_u64().expect("кадры") > 0,
            "кадры дошли до RAW-приёмника"
        );
        assert!(step.get("failure").is_none(), "{step}");
        assert_eq!(step["state_at_end"], "streaming");
        assert!(step["time_to_streaming_ms"].is_u64());
    }
    // Сценарий закончился — маршрутизатор закрыл камеру, которую тот заявил.
    let closed = harness.rig().wait_state("closed");
    assert_eq!(closed.state.name(), "closed");
    assert!(!harness.rig().control.is_device_open());
}

#[test]
fn full_default_matrix_of_six_steps_fits_into_logcat_lines_with_full_details_in_stages() {
    // Настройки по умолчанию (шесть шагов решения 21), только замер короче. Подставной HAL шлёт финальные
    // результаты и для камеры 0, поэтому её шагу возвращается обычный режим ожиданий.
    let mut scenarios = CameraScenarioConfig {
        measure_ms: 100,
        ..CameraScenarioConfig::default()
    };
    scenarios.stream_steps[4].capture_results = CaptureResultsExpectation::Required;
    let rig = FakeCameraRig::with_scenarios(scenarios);
    let mut harness = harness_for_rig(rig);
    let _frames = FrameFeeder::start(harness.rig(), SIXTY_FPS);
    let result = run_stream(&mut harness);

    assert_eq!(result["status"], "passed", "{result}");
    assert_eq!(step_reports(&result).len(), 6);
    let details = stage_items(&harness, "detail_steps");
    assert_eq!(details.len(), 6, "полный отчёт каждого шага выложен");
    assert!(
        details
            .iter()
            .all(|detail| detail["stats"].is_object() && detail["events"].is_array())
    );
    assert_fits_logcat(&harness, &result);
}

#[test]
fn stages_follow_the_steps_in_order() {
    let mut harness = harness_for_rig(stream_rig(sixty_fps_matrix()[..2].to_vec()));
    let _frames = FrameFeeder::start(harness.rig(), SIXTY_FPS);
    let result = run_stream(&mut harness);

    assert_eq!(result["status"], "passed", "{result}");
    assert_eq!(
        harness.output.stage_names(),
        [
            "started",
            "step_started",
            "step_streaming",
            "step_measured",
            "step_started",
            "step_streaming",
            "step_measured",
            "detail_steps",
            "detail_steps"
        ]
    );
}

#[test]
fn a_failed_step_does_not_stop_the_others_and_the_report_keeps_all_of_them() {
    use ExpectedLever::PublicAeTargetRange;
    use RawFormatSetting::Raw10;
    use ReadoutSetting::SixteenByNine;
    let steps = vec![
        step("2", Raw10, SixteenByNine, 60, PublicAeTargetRange, 57.0),
        // Камеры 9 нет в каталоге: движок ответит Unsupported.
        step("9", Raw10, SixteenByNine, 60, PublicAeTargetRange, 57.0),
        step("3", Raw10, ReadoutSetting::FourByThree, 60, PublicAeTargetRange, 55.0),
    ];
    let mut harness = harness_for_rig(stream_rig(steps));
    let _frames = FrameFeeder::start(harness.rig(), SIXTY_FPS);
    let result = run_stream(&mut harness);

    assert_eq!(result["status"], "failed", "{result}");
    let steps = step_reports(&result);
    assert_eq!(steps.len(), 3, "отчёт полный");
    assert!(steps[0].get("failure").is_none());
    assert_eq!(steps[1]["failure"]["kind"], "command_not_accepted");
    assert_eq!(steps[1]["failure"]["command"], "open");
    assert!(steps[2].get("failure").is_none(), "шаг после провала выполнен");
    assert_eq!(steps[2]["lever"], "public_ae_target_range", "шаг после провала измерен");
    assert_eq!(stage_items(&harness, "detail_steps")[2]["configured"]["camera"], "3");
    let error = result["error"].as_str().expect("error");
    assert!(
        error.contains("шаг 2") && !error.contains("шаг 1") && !error.contains("шаг 3"),
        "{error}"
    );
}

#[test]
fn slow_stream_fails_the_step_with_the_measured_number_in_the_message() {
    let mut harness = harness_for_rig(stream_rig(sixty_fps_matrix()[..1].to_vec()));
    let _frames = FrameFeeder::start(harness.rig(), THIRTY_FPS);
    let result = run_stream(&mut harness);

    assert_eq!(result["status"], "failed", "{result}");
    let fps = step_reports(&result)[0]["measured_fps"].as_f64().expect("fps");
    assert!((29.0..=31.0).contains(&fps), "{fps}");
    let error = result["error"].as_str().expect("error");
    assert!(error.contains("ниже порога 57.0"), "{error}");
}

fn logical_step(min_measured_fps: f64) -> StreamStepConfig {
    StreamStepConfig {
        capture_results: CaptureResultsExpectation::NotSentByHal,
        ..step(
            "0",
            RawFormatSetting::Raw10,
            ReadoutSetting::FourByThree,
            30,
            ExpectedLever::Any,
            min_measured_fps,
        )
    }
}

fn logical_rig(min_measured_fps: f64) -> FakeCameraRig {
    FakeCameraRig::with_scenarios(CameraScenarioConfig {
        measure_ms: 300,
        stream_start_timeout_ms: 3_000,
        stream_close_timeout_ms: 3_000,
        stream_steps: vec![logical_step(min_measured_fps)],
        ..CameraScenarioConfig::default()
    })
}

#[test]
fn logical_camera_without_final_results_passes_by_raw_frame_rate() {
    let mut harness = harness_for_rig(logical_rig(20.0));
    let _frames = FrameFeeder::start_raw_only(harness.rig(), StdDuration::from_millis(20));
    let result = run_stream(&mut harness);

    assert_eq!(result["status"], "passed", "{result}");
    let step = &step_reports(&result)[0];
    assert_eq!(step["capture_results"], "not_sent_by_hal");
    assert_eq!(step["results_completed"], 0);
    assert!(step.get("measured_fps").is_none(), "по меткам сенсора измерять нечего");
    let raw_fps = step["raw_fps"].as_f64().expect("частота по RAW");
    assert!(
        (20.0..=60.0).contains(&raw_fps),
        "RAW-кадры идут примерно 50 к/с, получено {raw_fps}"
    );
    assert!(step["raw_sink"]["frames_acquired"].as_u64().expect("кадры") >= 5);
    assert_fits_logcat(&harness, &result);
}

#[test]
fn logical_camera_with_too_few_raw_frames_fails_by_raw_frame_rate() {
    let mut harness = harness_for_rig(logical_rig(20.0));
    let _frames = FrameFeeder::start_raw_only(harness.rig(), StdDuration::from_millis(100));
    let result = run_stream(&mut harness);

    assert_eq!(result["status"], "failed", "{result}");
    let error = result["error"].as_str().expect("error");
    assert!(
        error.contains("по RAW-кадрам") && error.contains("ниже порога 20.0"),
        "{error}"
    );
}

#[test]
fn final_results_appearing_on_the_logical_camera_fail_with_the_changed_fact_message() {
    let mut harness = harness_for_rig(logical_rig(20.0));
    // HAL «обновился»: теперь шлёт и финальные результаты, и RAW-кадры идут быстро.
    let _frames = FrameFeeder::start(harness.rig(), THIRTY_FPS);
    let result = run_stream(&mut harness);

    assert_eq!(result["status"], "failed", "{result}");
    let error = result["error"].as_str().expect("error");
    assert!(error.contains("факт изменился (прошивка?)"), "{error}");
    assert!(
        step_reports(&result)[0]["results_completed"]
            .as_u64()
            .expect("результаты")
            > 0
    );
}

#[test]
fn raw_frame_rate_is_reported_next_to_the_sensor_rate_for_ordinary_steps() {
    let mut harness = harness_for_rig(stream_rig(sixty_fps_matrix()[..1].to_vec()));
    let _frames = FrameFeeder::start(harness.rig(), SIXTY_FPS);
    let result = run_stream(&mut harness);

    assert_eq!(result["status"], "passed", "{result}");
    let step = &step_reports(&result)[0];
    assert!(step["raw_fps"].as_f64().expect("raw_fps") > 0.0);
    assert!(step.get("capture_results").is_none(), "обычный режим не пишется");
    assert!(
        stage_items(&harness, "detail_steps")[0]["window_ms"]
            .as_u64()
            .expect("окно")
            >= 100
    );
}

#[test]
fn leaking_raw_buffers_fail_the_step() {
    let mut harness = harness_for_rig(stream_rig(sixty_fps_matrix()[..1].to_vec()));
    let _frames = FrameFeeder::start_leaking(harness.rig(), SIXTY_FPS);
    let result = run_stream(&mut harness);

    assert_eq!(result["status"], "failed", "{result}");
    let error = result["error"].as_str().expect("error");
    assert!(error.contains("утечка буферов"), "{error}");
    assert_eq!(step_reports(&result)[0]["raw_sink"]["frames_released"], 0);
}

#[test]
fn camera_that_never_reaches_streaming_is_reported_by_deadline_not_by_hanging() {
    let rig = FakeCameraRig::with_scenarios(CameraScenarioConfig {
        measure_ms: 100,
        stream_start_timeout_ms: 200,
        stream_close_timeout_ms: 1_000,
        stream_steps: sixty_fps_matrix()[..1].to_vec(),
        ..CameraScenarioConfig::default()
    });
    let mut harness = harness_for_rig(rig);
    // Resumed не приходит: приложение «в фоне», Android камеры не даёт, движок ждёт переднего плана.
    harness.start(Scenario::parse("camera-stream").expect("сценарий зарегистрирован"));
    harness.run_until_result();
    let result = harness.output.single_result();

    assert_eq!(result["status"], "failed", "{result}");
    assert_eq!(step_reports(&result)[0]["failure"]["kind"], "never_streaming");
    assert_eq!(step_reports(&result)[0]["failure"]["last_state"], "closed");
}

#[test]
fn camera_open_refused_by_the_phone_is_a_step_failure_with_the_reason() {
    let rig = stream_rig(sixty_fps_matrix()[..1].to_vec());
    rig.control
        .push_open_result(Err(camera_core::backend::OpenFailure::Unavailable {
            reason: camera_contract::availability::CameraUnavailableReason::PermissionDenied,
        }));
    let mut harness = harness_for_rig(rig);
    let result = run_stream(&mut harness);

    assert_eq!(result["status"], "failed", "{result}");
    let step = &step_reports(&result)[0];
    assert!(step["failure"].is_object(), "{step}");
    let journal = stage_items(&harness, "detail_steps")[0]["events"].to_string();
    assert!(
        journal.contains("permission_denied"),
        "причина отказа есть в журнале шага (detail_steps): {journal}"
    );
}

#[test]
fn camera_that_does_not_close_in_time_fails_the_next_step_by_deadline() {
    let rig = FakeCameraRig::with_scenarios(CameraScenarioConfig {
        measure_ms: 100,
        stream_start_timeout_ms: 3_000,
        stream_close_timeout_ms: 150,
        stream_steps: sixty_fps_matrix()[..2].to_vec(),
        ..CameraScenarioConfig::default()
    });
    let mut harness = harness_for_rig(rig);
    let _frames = FrameFeeder::start(harness.rig(), SIXTY_FPS);
    harness.deliver(ShellEvent::Resumed);
    harness.start(Scenario::parse("camera-stream").expect("сценарий зарегистрирован"));
    let deadline = std::time::Instant::now() + StdDuration::from_secs(5);
    while !harness.output.stage_names().contains(&"step_streaming".to_owned()) && std::time::Instant::now() < deadline {
        harness.router.step(StdDuration::from_millis(2));
    }
    // С этого момента закрытие камеры занимает 400 мс — дольше срока ожидания (150 мс).
    harness
        .rig()
        .close_device_delay_ms
        .store(400, std::sync::atomic::Ordering::SeqCst);
    harness.run_until_result();
    harness
        .rig()
        .close_device_delay_ms
        .store(0, std::sync::atomic::Ordering::SeqCst);
    let result = harness.output.single_result();

    assert_eq!(result["status"], "failed", "{result}");
    let steps = step_reports(&result);
    assert_eq!(steps.len(), 2);
    assert!(
        steps[0].get("failure").is_none() && steps[0]["measured_fps"].is_f64(),
        "первый шаг прошёл"
    );
    assert_eq!(steps[1]["failure"]["kind"], "previous_camera_did_not_close");
    assert!(
        steps[1]["lever"].is_null() && steps[1]["measured_fps"].is_null(),
        "шаг не начинался"
    );
}

#[test]
fn next_step_waits_for_the_close_acknowledgement_and_not_for_a_stale_closed_snapshot() {
    let (release, gate) = std::sync::mpsc::channel::<()>();
    let control = FakeBackendControl::new();
    let inner_factory = control.factory();
    // Поток камеры стоит в фабрике: снимок «закрыто» остался с момента запуска, а команды ждут в очереди.
    let factory: camera_core::backend::BackendFactory = Box::new(move |sink| {
        let _ = gate.recv();
        inner_factory(sink)
    });
    let services = start_camera(
        &CameraConfig::default(),
        CameraBackendParts {
            factory,
            diagnostics: Arc::new(BackendDiagnostics::new(4)) as Arc<dyn BackendDiagnosticsSource>,
        },
    );
    let mut harness = harness_for_services(services);
    harness.deliver(ShellEvent::Resumed);
    harness.start(Scenario::parse("camera-stream").expect("сценарий зарегистрирован"));
    for _ in 0..50 {
        harness.router.step(StdDuration::from_millis(2));
    }

    assert_eq!(
        harness.output.stage_names(),
        ["started"],
        "движок ещё не подтвердил close: старый снимок Closed не повод начинать шаг"
    );
    drop(release);
}

#[test]
fn stream_scenario_on_an_unavailable_engine_fails_with_its_reason() {
    let services = CameraServices::unavailable("нет камеры", CameraScenarioConfig::default());
    let mut harness = harness_for_services(services);
    harness.start(Scenario::parse("camera-stream").expect("сценарий зарегистрирован"));
    harness.run_until_result();
    let result = harness.output.single_result();
    assert_eq!(result["status"], "failed");
    assert!(
        result["error"].as_str().expect("error").contains("нет камеры"),
        "{result}"
    );
}

// ---- camera-evict (только отладочная сборка): тесты в отдельном файле ----

#[cfg(feature = "debug-scenarios")]
#[path = "event_router_camera_evict_tests.rs"]
mod evict;

#[cfg(not(feature = "debug-scenarios"))]
#[test]
fn evict_scenario_is_refused_in_release_as_debug_only() {
    let error = Scenario::parse("camera-evict").expect_err("в release её нет");
    assert!(
        matches!(error, crate::scenario::ScenarioError::DebugOnly { .. }),
        "{error}"
    );
}

//! Функциональные тесты сценариев `raw-stream*`: настоящий маршрутизатор, настоящий движок камеры на
//! fake-платформе и подставной «живой поток» (`StreamPump`). Каждое правило вердикта проверено на
//! пути до строки результата, которую читает скрипт smoke.

use camera_core::backend::{ConfigureFailure, UnsupportedStream};

use super::*;
use crate::camera_config::CameraConfig;
use crate::camera_scenario_config::CameraScenarioConfig;
use crate::raw_stream_scenario_config::RawStreamScenarioConfig;
use crate::raw_stream_test_support::{FrameNumbers, PumpFaults, StreamPump};

/// Короткие сроки: замер 300 мс, прогресс каждые 100 мс.
fn short_raw_config(tune: impl FnOnce(&mut RawStreamScenarioConfig)) -> CameraConfig {
    let mut raw_stream = RawStreamScenarioConfig {
        measure_ms: 300,
        endurance_measure_ms: 600,
        cpu_probe_measure_ms: 200,
        streaming_timeout_ms: 2_000,
        render_live_timeout_ms: 500,
        viewfinder_settle_ms: 100,
        progress_interval_ms: 100,
        ..RawStreamScenarioConfig::default()
    };
    tune(&mut raw_stream);
    CameraConfig {
        scenarios: CameraScenarioConfig {
            raw_stream,
            ..CameraScenarioConfig::default()
        },
        ..CameraConfig::default()
    }
}

/// Приложение на экране, поток запущен с заданными неисправностями; сценарий `name` идёт до итога.
fn run_raw(config: CameraConfig, name: &str, faults: PumpFaults) -> (Harness, serde_json::Value) {
    let mut harness = harness_for_rig(FakeCameraRig::with_config(config));
    let result = run_raw_on(&mut harness, name, faults);
    (harness, result)
}

fn run_raw_on(harness: &mut Harness, name: &str, faults: PumpFaults) -> serde_json::Value {
    harness.deliver(ShellEvent::Resumed);
    let _pump = StreamPump::start(harness.rig(), Arc::clone(&harness.render), faults);
    harness.start(Scenario::parse(name).expect("сценарий зарегистрирован"));
    harness.run_until_result();
    harness.output.single_result()
}

fn stages_of(harness: &Harness, name: &str) -> Vec<serde_json::Value> {
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

fn error_of(result: &serde_json::Value) -> String {
    result["error"].as_str().unwrap_or_default().to_owned()
}

fn assert_failed_with(result: &serde_json::Value, needle: &str) {
    assert_eq!(result["status"], "failed", "{result}");
    let error = error_of(result);
    assert!(error.contains(needle), "в ошибке нет «{needle}»: {error}");
}

/// Строка результата и каждая строка стадии помещаются в запись logcat.
fn assert_fits_logcat(harness: &Harness, result: &serde_json::Value) {
    use crate::logging::record::LOGCAT_MAX_TEXT_BYTES;
    use crate::result_line::{RESULT_LINE_PREFIX, STAGE_LINE_PREFIX};
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
fn healthy_run_passes_and_reports_fps_zero_copies_latency_and_metadata_ownership() {
    let (harness, result) = run_raw(short_raw_config(|_| {}), "raw-stream", PumpFaults::default());

    assert_eq!(result["status"], "passed", "{result}");
    let report = &result["report"];
    assert_eq!(report["kind"], "raw-stream");
    let fps = report["sensor_fps"].as_f64().expect("fps");
    assert!((59.0..=61.0).contains(&fps), "частота по меткам сенсора {fps}");
    assert_eq!(report["target_fps"], 60);
    assert_eq!(report["cpu_copies_per_frame"], 0.0);
    assert_eq!(report["configured"]["camera"], "2");
    assert_eq!(report["configured"]["raw_format"], "raw10");
    assert!(report["presented_since_live"].as_u64().expect("показано") > 0);
    // Владение метаданными: номера кадров растут вместе с метками, нарушений нет.
    assert_eq!(report["ownership"]["violations"], 0);
    assert!(report["ownership"]["pairs_checked"].as_u64().expect("пары") > 0);
    // Задержка: рендер имитирует ровно 40 мс, смещение часов то же, что читает сценарий.
    let p50 = report["latency"]["p50_ms"].as_f64().expect("p50");
    assert!((39.5..=40.5).contains(&p50), "p50 задержки {p50}");
    assert!(report["latency"]["samples"].as_u64().expect("замеры") > 0);
    assert_eq!(report["latency"]["implausible"], 0);
    assert_eq!(report["thermal_start"]["status"], "none");
    // Обычный прогон идёт с выключенной пробой: читатель создаётся без CPU_READ_RARELY.
    assert_eq!(*harness.rig().probe_at_configure.lock().expect("замок"), [false]);
    assert_fits_logcat(&harness, &result);
}

#[test]
fn stages_come_in_order_started_streaming_live_progress_details_and_live_is_signalled_before_the_measure() {
    let (harness, result) = run_raw(short_raw_config(|_| {}), "raw-stream", PumpFaults::default());
    assert_eq!(result["status"], "passed", "{result}");

    let names = harness.output.stage_names();
    let position = |name: &str| {
        names
            .iter()
            .position(|stage| stage == name)
            .unwrap_or_else(|| panic!("нет стадии {name}: {names:?}"))
    };
    assert!(position("started") < position("streaming"), "{names:?}");
    assert!(position("streaming") < position("viewfinder_live"), "{names:?}");
    assert!(position("viewfinder_live") < position("progress"), "{names:?}");
    assert!(position("progress") < position("detail_stream"), "{names:?}");
    assert_eq!(
        names.iter().filter(|name| *name == "viewfinder_live").count(),
        1,
        "сигнал для скриншотов один"
    );
    // Прогресс: замер 300 мс, шаг 100 мс — несколько строк с растущими счётчиками.
    let progress = stages_of(&harness, "progress");
    assert!(progress.len() >= 2, "{progress:?}");
    let offered: Vec<u64> = progress
        .iter()
        .map(|line| line["frames_offered"].as_u64().expect("кадры"))
        .collect();
    assert!(offered.windows(2).all(|pair| pair[1] >= pair[0]), "{offered:?}");
    assert!(offered.last().copied().unwrap_or(0) > 0);
    assert!(progress[0]["elapsed_ms"].is_u64() && progress[0]["thermal"].is_string());
    // Подробности — строками стадии до результата: счётчики потока целиком и читатели.
    let details = stages_of(&harness, "detail_stream");
    let items: Vec<&serde_json::Value> = details
        .iter()
        .flat_map(|line| line["items"].as_array().expect("items"))
        .collect();
    assert!(
        items
            .iter()
            .any(|item| item["stream"]["frames_offered"].as_u64().unwrap_or(0) > 0)
    );
    assert!(
        items
            .iter()
            .any(|item| item["reader"]["frames_acquired"].as_u64().unwrap_or(0) > 0)
    );
}

#[test]
fn screenshots_and_measure_wait_for_auto_exposure_to_settle_after_the_first_camera_frame() {
    // Причина (телефон, сессия 05): первый кадр после старта потока почти чёрный — автоэкспозиция
    // ещё не сошлась, и скриншот на нём провалил проверку «экран не чёрный».
    let settle_ms = 400;
    let (harness, result) = run_raw(
        short_raw_config(|raw| raw.viewfinder_settle_ms = settle_ms),
        "raw-stream",
        PumpFaults::default(),
    );
    assert_eq!(result["status"], "passed", "{result}");
    let first_frame_ms = result["report"]["time_to_live_ms"]
        .as_u64()
        .unwrap_or_else(|| panic!("нет time_to_live_ms: {result}"));
    let live = stages_of(&harness, "viewfinder_live");
    let live_after_ms = live[0]["after_ms"].as_u64().expect("after_ms");
    assert!(
        live_after_ms >= first_frame_ms + settle_ms,
        "viewfinder_live через {live_after_ms} мс, первый кадр на {first_frame_ms} мс, установление {settle_ms} мс"
    );
}

#[test]
fn camera_is_closed_by_the_router_after_the_scenario() {
    let (harness, result) = run_raw(short_raw_config(|_| {}), "raw-stream", PumpFaults::default());
    assert_eq!(result["status"], "passed", "{result}");
    let closed = harness.rig().wait_state("closed");
    assert_eq!(closed.state.name(), "closed");
    assert!(!harness.rig().control.is_device_open());
}

#[test]
fn endurance_uses_its_own_longer_measure_and_name() {
    let (harness, result) = run_raw(short_raw_config(|_| {}), "raw-stream-endurance", PumpFaults::default());
    assert_eq!(result["scenario"], "raw-stream-endurance");
    assert_eq!(result["status"], "passed", "{result}");
    assert!(
        result["report"]["window_ms"].as_u64().expect("окно") >= 550,
        "окно endurance 600 мс, а не 300: {}",
        result["report"]["window_ms"]
    );
    assert_eq!(stages_of(&harness, "started")[0]["measure_ms"], 600);
}

#[test]
fn cpu_copy_on_the_viewfinder_path_fails_the_run() {
    let faults = PumpFaults {
        cpu_copy_per_frame: true,
        ..PumpFaults::default()
    };
    let (_harness, result) = run_raw(short_raw_config(|_| {}), "raw-stream", faults);
    assert_failed_with(&result, "cpu_copies");
    assert!(result["report"]["cpu_copies_per_frame"].as_f64().expect("доля") > 0.99);
}

#[test]
fn leaked_leases_pool_exhaustion_and_unpaired_frames_each_fail_with_their_own_message() {
    for (faults, needle) in [
        (
            PumpFaults {
                leak_leases: true,
                ..PumpFaults::default()
            },
            "leaked_leases",
        ),
        (
            PumpFaults {
                pool_exhausted: true,
                ..PumpFaults::default()
            },
            "pool_exhausted",
        ),
        (
            PumpFaults {
                unpaired: true,
                ..PumpFaults::default()
            },
            "с метаданными",
        ),
        (
            PumpFaults {
                max_frames_outstanding: 99,
                ..PumpFaults::default()
            },
            "пик кадров на руках 99",
        ),
        (
            PumpFaults {
                release_export_failures: true,
                ..PumpFaults::default()
            },
            "release_export_failures",
        ),
    ] {
        let (_harness, result) = run_raw(short_raw_config(|_| {}), "raw-stream", faults);
        assert_failed_with(&result, needle);
    }
}

#[test]
fn swapped_frame_numbers_fail_metadata_ownership_and_missing_numbers_do_not_pass_silently() {
    let swapped = PumpFaults {
        frame_numbers: FrameNumbers::SwappedInPairs,
        ..PumpFaults::default()
    };
    let (_harness, result) = run_raw(short_raw_config(|_| {}), "raw-stream", swapped);
    assert_failed_with(&result, "метаданные кадров перепутаны");
    assert!(result["report"]["ownership"]["violations"].as_u64().expect("нарушения") > 0);

    let absent = PumpFaults {
        frame_numbers: FrameNumbers::Absent,
        ..PumpFaults::default()
    };
    let (_harness, result) = run_raw(short_raw_config(|_| {}), "raw-stream", absent);
    assert_failed_with(&result, "владение метаданными не проверено");
}

#[test]
fn logical_camera_without_final_results_can_skip_the_pairing_rule_by_config() {
    let faults = PumpFaults {
        unpaired: true,
        frame_numbers: FrameNumbers::Absent,
        ..PumpFaults::default()
    };
    let config = short_raw_config(|raw| raw.expect_paired_metadata = false);
    let (_harness, result) = run_raw(config, "raw-stream", faults);
    assert_eq!(result["status"], "passed", "{result}");
}

#[test]
fn render_that_never_shows_the_camera_fails_by_deadline_with_a_clear_reason() {
    let faults = PumpFaults {
        render_shows_nothing: true,
        ..PumpFaults::default()
    };
    let (_harness, result) = run_raw(short_raw_config(|_| {}), "raw-stream", faults);
    assert_failed_with(&result, "рендер не показал ни одного кадра камеры");
    assert!(
        stages_of(&_harness, "viewfinder_live").is_empty(),
        "сигнал для скриншотов не подан"
    );
}

#[test]
fn unavailable_render_fails_with_the_render_reason() {
    let rig = FakeCameraRig::with_config(short_raw_config(|_| {}));
    let camera = rig.services.clone();
    let mut harness = build_harness_with_camera(
        ScenarioConfig::default(),
        Box::new(RecordingRoute::new(ReceiptPolicy::Confirm)),
        FakeRenderStatus::unavailable("GPU не поднялся"),
        FakeGpuDiagnostics::new(Vec::new(), None),
        RenderLoopConfig::default(),
        camera,
    );
    harness.rig = Some(rig);
    let result = run_raw_on(&mut harness, "raw-stream", PumpFaults::default());
    assert_failed_with(&result, "GPU не поднялся");
}

#[test]
fn camera_that_never_streams_fails_by_deadline_and_still_carries_a_report() {
    let mut harness = harness_for_rig(FakeCameraRig::with_config(short_raw_config(|raw| {
        raw.streaming_timeout_ms = 200;
    })));
    // Resumed не приходит: приложение «в фоне», Android камеры не даёт.
    harness.start(Scenario::parse("raw-stream").expect("сценарий"));
    harness.run_until_result();
    let result = harness.output.single_result();
    assert_failed_with(&result, "Streaming");
    assert_eq!(result["report"]["kind"], "raw-stream");
}

#[test]
fn engine_degradation_is_reported_not_failed_and_the_target_becomes_the_fallback_rate() {
    let rig = FakeCameraRig::with_config(short_raw_config(|_| {}));
    rig.control.push_configure_failure(ConfigureFailure::Unsupported(
        UnsupportedStream::FrameRateNotAchievable {
            requested: camera_contract::stream::TargetFrameRate::new(60).expect("60"),
        },
    ));
    let mut harness = harness_for_rig(rig);
    let result = run_raw_on(&mut harness, "raw-stream", PumpFaults::default());

    assert_eq!(result["status"], "passed", "откат частоты — не отказ: {result}");
    let report = &result["report"];
    assert_eq!(report["stream_degraded"]["requested_fps"], 60);
    assert_eq!(report["stream_degraded"]["fallback_fps"], 30);
    assert_eq!(report["target_fps"], 30, "цель — запасная частота");
    assert_eq!(report["requested"]["frame_rate"], 60, "запрошенное тоже видно");
}

#[test]
fn result_line_stays_within_the_logcat_limit_even_with_a_long_error_list() {
    let faults = PumpFaults {
        cpu_copy_per_frame: true,
        leak_leases: true,
        pool_exhausted: true,
        unpaired: true,
        release_export_failures: true,
        max_frames_outstanding: 99,
        frame_numbers: FrameNumbers::SwappedInPairs,
        ..PumpFaults::default()
    };
    let (harness, result) = run_raw(short_raw_config(|_| {}), "raw-stream", faults);
    assert_eq!(result["status"], "failed", "{result}");
    assert_fits_logcat(&harness, &result);
}

#[test]
fn scenarios_are_registered_with_their_names() {
    assert!(Scenario::parse("raw-stream").is_ok());
    assert!(Scenario::parse("raw-stream-endurance").is_ok());
}

#[cfg(not(feature = "debug-scenarios"))]
#[test]
fn cpu_probe_scenario_does_not_exist_in_release_builds() {
    assert!(matches!(
        Scenario::parse("raw-stream-cpu-probe"),
        Err(crate::scenario::ScenarioError::DebugOnly { .. })
    ));
}

#[cfg(feature = "debug-scenarios")]
mod cpu_probe {
    use super::*;

    #[test]
    fn probe_is_armed_before_streaming_counts_copies_passes_and_is_disarmed_after() {
        let mut harness = harness_for_rig(FakeCameraRig::with_config(short_raw_config(|_| {})));
        harness.deliver(ShellEvent::Resumed);
        let _pump = StreamPump::start(harness.rig(), Arc::clone(&harness.render), PumpFaults::default());
        harness.start(Scenario::parse("raw-stream-cpu-probe").expect("сценарий"));
        harness.run_until_result();
        let result = harness.output.single_result();

        assert_eq!(result["status"], "passed", "{result}");
        let armed_at_configure = harness.rig().probe_at_configure.lock().expect("замок").clone();
        assert_eq!(
            armed_at_configure,
            [true],
            "проба взведена ДО конфигурации сессии: иначе читатель создался бы без CPU_READ_RARELY"
        );
        assert!(result["report"]["cpu_copies_per_frame"].as_f64().expect("доля") > 0.0);
        assert!(!harness.rig().cpu_probe.is_armed(), "проба выключена после сценария");
        assert!(
            harness
                .output
                .stage_names()
                .iter()
                .all(|name| name != "viewfinder_live"),
            "пробе экран не нужен"
        );
    }

    #[test]
    fn probe_refused_by_the_platform_fails_with_an_explicit_message_and_is_still_disarmed() {
        let faults = PumpFaults {
            probe_refused_by_platform: true,
            ..PumpFaults::default()
        };
        let (harness, result) = run_raw(short_raw_config(|_| {}), "raw-stream-cpu-probe", faults);
        assert_failed_with(&result, "платформа отвергла пробу");
        assert!(!harness.rig().cpu_probe.is_armed());
    }

    #[test]
    fn dead_copy_counter_fails_the_probe() {
        let faults = PumpFaults {
            probe_counter_dead: true,
            ..PumpFaults::default()
        };
        let (_harness, result) = run_raw(short_raw_config(|_| {}), "raw-stream-cpu-probe", faults);
        assert_failed_with(&result, "не подключён");
    }

    #[test]
    fn probe_is_disarmed_even_when_the_camera_never_streams() {
        let mut harness = harness_for_rig(FakeCameraRig::with_config(short_raw_config(|raw| {
            raw.streaming_timeout_ms = 150;
        })));
        harness.start(Scenario::parse("raw-stream-cpu-probe").expect("сценарий"));
        harness.run_until_result();
        assert_eq!(harness.output.single_result()["status"], "failed");
        assert!(!harness.rig().cpu_probe.is_armed());
    }
}

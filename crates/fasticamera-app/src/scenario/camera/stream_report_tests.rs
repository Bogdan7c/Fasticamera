//! Вердикт шага `camera-stream`: проходной образец и каждое правило отказа по отдельности.

use camera_android::BackendDiagnostics;
use camera_contract::capabilities::FpsRange;
use camera_contract::stream::FrameRateMechanism;
use camera_core::status::MissingKeyCount;

use super::*;
use crate::camera_scenario_config::{CaptureResultsExpectation, ExpectedLever, RawFormatSetting, ReadoutSetting};

const LIMIT: u32 = 4;

fn step_config(expected_lever: ExpectedLever) -> StreamStepConfig {
    StreamStepConfig {
        camera_id: "2".to_owned(),
        raw_format: RawFormatSetting::Raw10,
        readout: ReadoutSetting::SixteenByNine,
        frame_rate: 60,
        expected_lever,
        min_measured_fps: 57.0,
        capture_results: CaptureResultsExpectation::Required,
    }
}

fn healthy_stats() -> StreamStats {
    let mut stats = StreamStats::empty();
    stats.results_completed = 300;
    stats.intervals = 299;
    stats.measured_frames_per_second = Some(59.98);
    stats.missing_keys = vec![MissingKeyCount {
        key: ResultKey::FrameDuration,
        results: 300,
    }];
    stats
}

fn healthy() -> StepObservation {
    StepObservation {
        failure: None,
        configured: Some(ConfiguredEntry {
            camera: "2".to_owned(),
            raw_format: "raw10",
            buffer_width: 4096,
            buffer_height: 3072,
            frame_rate: 60,
            lever: "public_ae_target_range",
            lever_detail: FrameRateMechanism::PublicAeTargetRange {
                range: FpsRange { min: 60, max: 60 },
            },
        }),
        time_to_streaming_ms: Some(850),
        stats: Some(healthy_stats()),
        state_at_end: Some("streaming"),
        raw_sink: Some(RawSinkDelta {
            frames_acquired: 300,
            frames_released: 299,
            acquire_errors: 0,
        }),
        window_ms: Some(5_000),
        raw_fps: Some(59.8),
        callback_panics: 0,
        unmatched_frame_numbers: 0,
        unavailable: Vec::new(),
        events: Vec::new(),
        events_dropped: 0,
    }
}

fn violations_with(expected_lever: ExpectedLever, change: impl FnOnce(&mut StepObservation)) -> Vec<String> {
    let mut observation = healthy();
    change(&mut observation);
    evaluate_step(&step_config(expected_lever), &observation, LIMIT)
}

fn only(change: impl FnOnce(&mut StepObservation)) -> Vec<String> {
    violations_with(ExpectedLever::PublicAeTargetRange, change)
}

#[test]
fn healthy_step_passes_even_though_frame_duration_key_is_absent() {
    assert_eq!(only(|_| {}), Vec::<String>::new());
}

#[test]
fn step_that_never_streamed_reports_only_the_failure() {
    let violations = only(|o| {
        o.failure = Some(RunFailure::NeverStreaming { last_state: "opening" });
        o.configured = None;
    });
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(
        violations[0].contains("Streaming") && violations[0].contains("opening"),
        "{violations:?}"
    );
}

#[test]
fn every_failure_kind_has_a_readable_description() {
    let described = [
        RunFailure::CommandRejected {
            command: "open",
            rejected: "очередь полна".to_owned(),
        },
        RunFailure::CommandNotAccepted {
            command: "configure",
            outcome: serde_json::json!({"kind": "unsupported"}),
        },
        RunFailure::ConfigurationFailed {
            failure: "формат".to_owned(),
        },
        RunFailure::EngineFatal {
            cause: "Boom".to_owned(),
        },
        RunFailure::PreviousCameraDidNotClose {
            last_state: "streaming",
        },
    ]
    .map(|failure| failure.describe());
    assert!(described[0].contains("open") && described[0].contains("очередь полна"));
    assert!(described[1].contains("configure") && described[1].contains("unsupported"));
    assert!(described[2].contains("формат"));
    assert!(described[3].contains("Boom"));
    assert!(described[4].contains("streaming") && described[4].contains("не выполнялся"));
}

#[test]
fn frame_rate_below_threshold_fails_but_exactly_the_threshold_passes() {
    let low = only(|o| o.stats.as_mut().expect("stats").measured_frames_per_second = Some(56.9));
    assert_eq!(low.len(), 1, "{low:?}");
    assert!(low[0].contains("56.90") && low[0].contains("57.0"), "{low:?}");
    let boundary = only(|o| o.stats.as_mut().expect("stats").measured_frames_per_second = Some(57.0));
    assert_eq!(boundary, Vec::<String>::new());
    let unmeasured = only(|o| o.stats.as_mut().expect("stats").measured_frames_per_second = None);
    assert!(
        unmeasured.iter().any(|text| text.contains("не измерена")),
        "{unmeasured:?}"
    );
}

#[test]
fn wrong_lever_fails_and_any_lever_is_not_checked() {
    let wrong = violations_with(ExpectedLever::VendorSensorFrameDurationOverride, |_| {});
    assert_eq!(wrong.len(), 1, "{wrong:?}");
    assert!(
        wrong[0].contains("public_ae_target_range") && wrong[0].contains("vendor_sensor_frame_duration_override"),
        "{wrong:?}"
    );
    assert_eq!(violations_with(ExpectedLever::Any, |_| {}), Vec::<String>::new());
}

#[test]
fn missing_required_result_keys_fail_but_optional_ones_do_not() {
    for key in [
        ResultKey::SensorTimestamp,
        ResultKey::ExposureTime,
        ResultKey::Sensitivity,
    ] {
        let violations = only(|o| {
            o.stats.as_mut().expect("stats").missing_keys = vec![MissingKeyCount { key, results: 3 }];
        });
        assert_eq!(violations.len(), 1, "{key:?}: {violations:?}");
        assert!(violations[0].contains("3 результатах"), "{violations:?}");
    }
    let optional = only(|o| {
        o.stats.as_mut().expect("stats").missing_keys = vec![
            MissingKeyCount {
                key: ResultKey::FrameDuration,
                results: 300,
            },
            MissingKeyCount {
                key: ResultKey::RollingShutterSkew,
                results: 300,
            },
        ];
    });
    assert_eq!(optional, Vec::<String>::new());
    let silent = only(|o| {
        o.stats.as_mut().expect("stats").missing_keys = vec![MissingKeyCount {
            key: ResultKey::ExposureTime,
            results: 0,
        }];
    });
    assert_eq!(silent, Vec::<String>::new(), "ключ с нулём пропусков не нарушение");
}

#[test]
fn results_without_timestamp_and_zero_results_fail() {
    assert!(
        only(|o| o.stats.as_mut().expect("stats").results_without_timestamp = 2)
            .iter()
            .any(|text| text.contains("SENSOR_TIMESTAMP"))
    );
    assert!(
        only(|o| o.stats.as_mut().expect("stats").results_completed = 0)
            .iter()
            .any(|text| text.contains("ни одного результата"))
    );
}

#[test]
fn stream_that_stopped_or_lost_the_camera_fails() {
    let stopped = only(|o| o.state_at_end = Some("error"));
    assert_eq!(stopped.len(), 1, "{stopped:?}");
    assert!(stopped[0].contains("error"), "{stopped:?}");
    let evicted = only(|o| o.unavailable = vec![CameraUnavailableReason::Evicted]);
    assert!(evicted.iter().any(|text| text.contains("недоступной")), "{evicted:?}");
}

#[test]
fn raw_sink_rules_zero_frames_leak_beyond_allowance_and_errors() {
    let none = only(|o| {
        o.raw_sink = Some(RawSinkDelta {
            frames_acquired: 0,
            frames_released: 0,
            acquire_errors: 0,
        })
    });
    assert!(none.iter().any(|text| text.contains("ни одного кадра")), "{none:?}");

    let at_limit = only(|o| {
        o.raw_sink = Some(RawSinkDelta {
            frames_acquired: 300,
            frames_released: 296,
            acquire_errors: 0,
        })
    });
    assert_eq!(at_limit, Vec::<String>::new(), "в полёте ровно допуск — можно");
    let leak = only(|o| {
        o.raw_sink = Some(RawSinkDelta {
            frames_acquired: 300,
            frames_released: 295,
            acquire_errors: 0,
        })
    });
    assert!(leak.iter().any(|text| text.contains("утечка")), "{leak:?}");
    let over_release = only(|o| {
        o.raw_sink = Some(RawSinkDelta {
            frames_acquired: 300,
            frames_released: 305,
            acquire_errors: 0,
        })
    });
    assert!(
        over_release.iter().any(|text| text.contains("утечка")),
        "{over_release:?}"
    );
    let errors = only(|o| {
        o.raw_sink = Some(RawSinkDelta {
            frames_acquired: 300,
            frames_released: 300,
            acquire_errors: 1,
        })
    });
    assert!(errors.iter().any(|text| text.contains("acquire")), "{errors:?}");
    let absent = only(|o| o.raw_sink = None);
    assert!(absent.iter().any(|text| text.contains("счётчиков RAW")), "{absent:?}");
}

#[test]
fn callback_panics_fail() {
    let violations = only(|o| o.callback_panics = 1);
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(violations[0].contains("паники колбэков"), "{violations:?}");
}

#[test]
fn raw_sink_delta_is_the_difference_of_snapshots_and_saturates() {
    let before = BackendDiagnostics::new(2);
    before.raw_sink().record_frame_acquired();
    before.raw_sink().record_frame_released();
    let snapshot_before = before.snapshot();
    let after = BackendDiagnostics::new(2);
    for _ in 0..5 {
        after.raw_sink().record_frame_acquired();
    }
    after.raw_sink().record_frame_released();
    after.raw_sink().record_acquire_error(-9);
    let snapshot_after = after.snapshot();

    assert_eq!(
        RawSinkDelta::between(&snapshot_before, &snapshot_after),
        RawSinkDelta {
            frames_acquired: 4,
            frames_released: 0,
            acquire_errors: 1
        }
    );
    assert_eq!(
        RawSinkDelta::between(&snapshot_after, &snapshot_before).frames_acquired,
        0,
        "откат счётчика насыщается нулём"
    );
}

#[test]
fn report_prefixes_violations_with_the_step_number_and_target() {
    let good = StepReport::judge(0, step_config(ExpectedLever::Any), healthy(), LIMIT);
    let mut bad_observation = healthy();
    bad_observation.callback_panics = 2;
    let bad = StepReport::judge(1, step_config(ExpectedLever::Any), bad_observation, LIMIT);
    let report = StreamReport {
        measure_ms: 5000,
        raw_in_flight_limit: LIMIT,
        steps: vec![good, bad],
        backend: BackendDiagnostics::new(2).snapshot(),
    };
    let violations = report.violations();
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(violations[0].starts_with("шаг 2 (камера 2"), "{violations:?}");
}

fn two_step_report() -> StreamReport {
    let mut stats = healthy_stats();
    stats.gaps = 2;
    stats.frames_missed_in_gaps = 3;
    stats.min_interval_nanos = Some(16_000_000);
    stats.max_interval_nanos = Some(50_000_000);
    stats.last_reported_frame_duration = Some(camera_contract::capabilities::FrameDuration::from_nanos(33_333_333));
    let mut first = healthy();
    first.stats = Some(stats);
    first.events = vec![TimedNote {
        at_ms: 5,
        note: super::super::progress::EventNote::State { state: "streaming" },
    }];
    first.events_dropped = 4;
    let mut second = healthy();
    second.callback_panics = 2;
    StreamReport {
        measure_ms: 5000,
        raw_in_flight_limit: LIMIT,
        steps: vec![
            StepReport::judge(0, step_config(ExpectedLever::Any), first, LIMIT),
            StepReport::judge(1, step_config(ExpectedLever::Any), second, LIMIT),
        ],
        backend: BackendDiagnostics::new(2).snapshot(),
    }
}

#[test]
fn summary_carries_the_key_numbers_the_owner_reads_after_the_run() {
    let report = two_step_report();
    let json = serde_json::to_value(report.summary_within(100_000)).expect("json");
    let first = &json["steps"][0];
    assert_eq!(first["index"], 0);
    assert!(
        first.get("failure").is_none() && first.get("unavailable").is_none(),
        "пустые «хвосты» не пишутся"
    );
    assert_eq!(first["lever"], "public_ae_target_range");
    assert_eq!(first["measured_fps"], 59.98, "округлено до сотых");
    assert_eq!(first["results_completed"], 300);
    assert_eq!(first["intervals"], 299);
    assert_eq!(first["gaps"], 2);
    assert_eq!(first["frames_missed_in_gaps"], 3);
    assert_eq!(first["min_interval_ns"], 16_000_000);
    assert_eq!(first["max_interval_ns"], 50_000_000);
    assert_eq!(
        first["last_reported_frame_duration_ns"], 33_333_333,
        "факт «CamX врёт» виден в сводке"
    );
    assert_eq!(first["missing_keys"][0]["key"], "frame_duration");
    assert_eq!(first["raw_sink"]["frames_acquired"], 300);
    assert_eq!(first["raw_sink"]["frames_released"], 299);
    assert_eq!(first["target"], "2/raw10/16x9@60");
    assert_eq!(first["state_at_end"], "streaming");
    assert!(
        first.get("violations").is_none(),
        "нарушения — в error результата и в detail_steps"
    );
}

#[test]
fn labels_are_readable_for_messages_and_compact_ascii_for_the_result_line() {
    let config = step_config(ExpectedLever::Any);
    assert_eq!(config.label(), "камера 2 raw10 16:9@60");
    assert_eq!(config.short_label(), "2/raw10/16x9@60");
    assert!(config.short_label().is_ascii());
}

#[test]
fn worst_case_summary_of_six_busy_steps_leaves_room_in_the_logcat_line() {
    use crate::logging::record::LOGCAT_MAX_TEXT_BYTES;
    use crate::result_line::RESULT_LINE_PREFIX;

    let mut steps = Vec::new();
    for index in 0..6 {
        let mut stats = healthy_stats();
        stats.results_completed = 123_456;
        stats.intervals = 123_455;
        stats.gaps = 1_234;
        stats.frames_missed_in_gaps = 12_345;
        stats.measured_frames_per_second = Some(59.123_456_789);
        stats.min_interval_nanos = Some(16_123_456);
        stats.max_interval_nanos = Some(166_123_456);
        stats.last_reported_frame_duration = Some(camera_contract::capabilities::FrameDuration::from_nanos(33_333_333));
        stats.capture_failures = 1_234;
        stats.missing_keys = [
            ResultKey::SensorTimestamp,
            ResultKey::ExposureTime,
            ResultKey::Sensitivity,
        ]
        .into_iter()
        .map(|key| MissingKeyCount { key, results: 123_456 })
        .collect();
        let mut observation = healthy();
        observation.stats = Some(stats);
        observation.time_to_streaming_ms = Some(12_345);
        observation.unmatched_frame_numbers = 12_345;
        observation.unavailable = vec![CameraUnavailableReason::TooManyCamerasOpen];
        observation.raw_sink = Some(RawSinkDelta {
            frames_acquired: 123_456,
            frames_released: 123_455,
            acquire_errors: 1_234,
        });
        let mut config = step_config(ExpectedLever::VendorSensorFrameDurationOverride);
        config.camera_id = "3".to_owned();
        observation.configured = observation.configured.map(|mut configured| {
            configured.lever = "vendor_sensor_frame_duration_override";
            configured
        });
        steps.push(StepReport::judge(index, config, observation, LIMIT));
    }
    let diagnostics = BackendDiagnostics::new(4);
    diagnostics.record_failure(
        "ACameraDevice_createCaptureSession",
        -10_001,
        "ошибка устройства камеры",
    );
    diagnostics.record_skipped_camera("9", "нет характеристик".to_owned());
    let report = StreamReport {
        measure_ms: 5_000,
        raw_in_flight_limit: LIMIT,
        steps,
        backend: diagnostics.snapshot(),
    };
    let summary = report.summary_within(3_000);
    assert!(
        summary.degraded,
        "полная сводка шести занятых шагов не влезает в бюджет: шаги урезаны"
    );
    let line = serde_json::json!({"scenario": "camera-stream", "status": "passed", "report": summary});
    let bytes = RESULT_LINE_PREFIX.len() + line.to_string().len();
    assert!(
        bytes + 600 <= LOGCAT_MAX_TEXT_BYTES,
        "урезанная сводка {bytes} байт: запас до {LOGCAT_MAX_TEXT_BYTES} меньше 600"
    );
    let json = serde_json::to_value(report.summary_within(3_000)).expect("json");
    assert_eq!(json["degraded"], true);
    assert_eq!(
        json["steps"][0]["raw_sink"]["frames_acquired"], 123_456,
        "по счётчикам RAW скрипт судит и урезанную сводку"
    );
    assert_eq!(json["steps"][0]["lever"], "vendor_sensor_frame_duration_override");
    assert!(json["steps"][0].get("gaps").is_none(), "подробности урезаны");
}

#[test]
fn healthy_summary_is_full_and_not_marked_degraded() {
    let report = two_step_report();
    let json = serde_json::to_value(report.summary_within(3_000)).expect("json");
    assert!(json.get("degraded").is_none());
    assert_eq!(json["steps"][0]["gaps"], 2);
}

#[test]
fn summary_of_a_step_that_never_streamed_has_nulls_instead_of_invented_numbers() {
    let mut observation = healthy();
    observation.failure = Some(RunFailure::NeverStreaming { last_state: "opening" });
    observation.configured = None;
    observation.stats = None;
    observation.raw_sink = None;
    observation.state_at_end = None;
    let report = StepReport::judge(0, step_config(ExpectedLever::Any), observation, LIMIT);
    let json = serde_json::to_value(StepSummary::of(&report)).expect("json");
    assert_eq!(json["failure"]["kind"], "never_streaming");
    for field in ["lever", "measured_fps", "results_completed", "raw_sink", "state_at_end"] {
        assert_eq!(json[field], serde_json::Value::Null, "{field}");
    }
}

#[test]
fn detail_items_hold_the_full_step_reports_with_events_and_stats() {
    let report = two_step_report();
    let items = report.detail_items();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0]["stats"]["measured_frames_per_second"], 59.98);
    assert_eq!(items[0]["configured"]["lever"], "public_ae_target_range");
    assert_eq!(items[0]["configured"]["buffer_width"], 4096);
    assert_eq!(items[0]["events"][0]["kind"], "state");
    assert_eq!(items[0]["events_dropped"], 4);
    assert_eq!(
        items[1]["violations"].as_array().map(Vec::len),
        Some(1),
        "полные нарушения шага — в подробностях"
    );
    assert!(
        items[1]["violations"][0]
            .as_str()
            .expect("текст")
            .contains("паники колбэков")
    );
}

#[test]
fn event_journal_is_capped_and_the_rest_is_counted() {
    let notes: Vec<TimedNote> = (0..45)
        .map(|at_ms| TimedNote {
            at_ms,
            note: super::super::progress::EventNote::State { state: "streaming" },
        })
        .collect();
    let (kept, dropped) = StepObservation::capped_events(&notes);
    assert_eq!(kept.len(), EVENTS_IN_REPORT_LIMIT);
    assert_eq!(dropped, 45 - EVENTS_IN_REPORT_LIMIT);
    assert_eq!(kept[0].at_ms, 0, "оставлены первые записи");
    let (all, none) = StepObservation::capped_events(&notes[..3]);
    assert_eq!((all.len(), none), (3, 0));
}

// ---- Режим «HAL не шлёт финальных результатов» (логическая камера 0, факт сессии 04) ----

fn logical_config() -> StreamStepConfig {
    StreamStepConfig {
        camera_id: "0".to_owned(),
        raw_format: RawFormatSetting::Raw10,
        readout: ReadoutSetting::FourByThree,
        frame_rate: 30,
        expected_lever: ExpectedLever::Any,
        min_measured_fps: 28.0,
        capture_results: CaptureResultsExpectation::NotSentByHal,
    }
}

/// Шаг логической камеры: RAW-кадры идут, финальных результатов нет, интервалов и ключей нет.
fn partial_only() -> StepObservation {
    let mut observation = healthy();
    observation.configured = observation.configured.map(|mut configured| {
        configured.lever = "hal_default";
        configured
    });
    observation.stats = Some(StreamStats::empty());
    observation.raw_fps = Some(29.9);
    observation
}

fn partial_violations(change: impl FnOnce(&mut StepObservation)) -> Vec<String> {
    let mut observation = partial_only();
    change(&mut observation);
    evaluate_step(&logical_config(), &observation, LIMIT)
}

#[test]
fn partial_only_hal_passes_by_raw_frame_rate_without_key_or_interval_checks() {
    assert_eq!(partial_violations(|_| {}), Vec::<String>::new());
    // Измеренной по меткам частоты и интервалов нет — и это не нарушение в этом режиме.
    let stats = partial_only().stats.expect("stats");
    assert_eq!(
        (
            stats.measured_frames_per_second,
            stats.intervals,
            stats.results_completed
        ),
        (None, 0, 0)
    );
}

#[test]
fn raw_frame_rate_below_threshold_or_unmeasured_fails_in_partial_mode() {
    let low = partial_violations(|o| o.raw_fps = Some(27.9));
    assert_eq!(low.len(), 1, "{low:?}");
    assert!(
        low[0].contains("по RAW-кадрам") && low[0].contains("27.90") && low[0].contains("28.0"),
        "{low:?}"
    );
    assert_eq!(
        partial_violations(|o| o.raw_fps = Some(28.0)),
        Vec::<String>::new(),
        "ровно порог — можно"
    );
    let unmeasured = partial_violations(|o| o.raw_fps = None);
    assert!(
        unmeasured.iter().any(|text| text.contains("не измерена")),
        "{unmeasured:?}"
    );
}

#[test]
fn final_results_appearing_is_a_changed_fact_not_a_silent_pass() {
    let violations = partial_violations(|o| o.stats.as_mut().expect("stats").results_completed = 12);
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(violations[0].contains("факт изменился (прошивка?)"), "{violations:?}");
    assert!(
        violations[0].contains("пересмотреть ожидание и отчёт 04") && violations[0].contains("12"),
        "{violations:?}"
    );
}

#[test]
fn other_rules_still_apply_in_partial_mode() {
    let leak = partial_violations(|o| {
        o.raw_sink = Some(RawSinkDelta {
            frames_acquired: 300,
            frames_released: 100,
            acquire_errors: 0,
        });
    });
    assert!(leak.iter().any(|text| text.contains("утечка")), "{leak:?}");
    assert!(
        partial_violations(|o| o.callback_panics = 1)
            .iter()
            .any(|text| text.contains("паники"))
    );
    assert!(
        partial_violations(|o| o.state_at_end = Some("error"))
            .iter()
            .any(|text| text.contains("не идёт"))
    );
    assert!(
        partial_violations(|o| o.unavailable = vec![CameraUnavailableReason::Evicted])
            .iter()
            .any(|text| text.contains("недоступной"))
    );
}

#[test]
fn required_mode_without_final_results_still_fails_and_raw_rate_does_not_save_it() {
    // Обычная камера, у которой пропали финальные результаты, — по-прежнему провал, как раньше.
    let violations = only(|o| {
        o.stats = Some(StreamStats::empty());
        o.raw_fps = Some(60.0);
    });
    assert!(
        violations.iter().any(|text| text.contains("ни одного результата")),
        "{violations:?}"
    );
    assert!(
        violations.iter().any(|text| text.contains("не измерена")),
        "{violations:?}"
    );
    // А низкая частота по RAW при хороших метках в обычном режиме — только справочное число.
    assert_eq!(only(|o| o.raw_fps = Some(10.0)), Vec::<String>::new());
}

#[test]
fn summary_shows_raw_rate_for_every_step_and_the_mode_only_when_it_is_not_default() {
    let normal = StepReport::judge(0, step_config(ExpectedLever::Any), healthy(), LIMIT);
    let partial = StepReport::judge(1, logical_config(), partial_only(), LIMIT);
    let json = serde_json::to_value([StepSummary::of(&normal), StepSummary::of(&partial)]).expect("json");
    assert_eq!(json[0]["raw_fps"], 59.8);
    assert!(json[0].get("capture_results").is_none());
    assert_eq!(json[1]["raw_fps"], 29.9);
    assert_eq!(json[1]["capture_results"], "not_sent_by_hal");
    assert_eq!(json[1]["results_completed"], 0);
    let details = serde_json::to_value(&partial).expect("json");
    assert_eq!(details["window_ms"], 5_000, "длительность замера — в полном отчёте");
    assert_eq!(details["target"]["capture_results"], "not_sent_by_hal");
}

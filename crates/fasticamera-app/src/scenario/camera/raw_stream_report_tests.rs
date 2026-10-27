use camera_contract::stream::FrameRateMechanism;

use camera_android::reader_counters::ArrivalAgeSnapshot;

use super::*;
use crate::raw_stream_scenario_config::RawStreamScenarioConfig;

fn configured(frame_rate: u32) -> ConfiguredEntry {
    ConfiguredEntry {
        camera: "2".to_owned(),
        raw_format: "raw10",
        buffer_width: 4096,
        buffer_height: 3072,
        frame_rate,
        lever: "vendor_sensor_frame_duration_override",
        lever_detail: FrameRateMechanism::HalDefault,
    }
}

fn thermal() -> ThermalMoment {
    ThermalMoment {
        status: Some("none"),
        headroom: Some(0.4),
        error: None,
    }
}

/// Здоровый минутный прогон 4:3@60: все правила выполнены с запасом.
fn healthy(kind: RawStreamRunKind) -> RawStreamObservation {
    RawStreamObservation {
        kind,
        failure: None,
        requested: RequestedMode {
            role: "wide",
            raw_format: "raw10",
            readout: "four_by_three",
            frame_rate: 60,
        },
        configured: Some(configured(60)),
        degraded: None,
        time_to_streaming_ms: Some(900),
        time_to_live_ms: Some(1_100),
        window_ms: 60_000,
        sensor_fps: Some(59.9),
        stats: None,
        stream: RawStreamCounters {
            frames_offered: 3_600,
            frames_paired: 3_600,
            frames_published: 3_600,
            max_frames_outstanding: 3,
            ..RawStreamCounters::default()
        },
        reader: ReaderSnapshot {
            frames_acquired: 3_600,
            acquire_errors: 0,
            last_acquire_error: None,
            pool_exhausted: 0,
            fence_merge_failures: 0,
            images_returned: 3_598,
            images_leaked: 0,
            readers_retired: 0,
            readers_deleted: 0,
            readers_leaked: 0,
            cpu_access_probe_copies: 0,
            cpu_access_probe_failures: 0,
            arrival_age: ArrivalAgeSnapshot::default(),
        },
        render: Some(RenderCameraDelta {
            camera_frames_presented: 3_590,
            camera_frames_imported: 3_590,
            ..RenderCameraDelta::default()
        }),
        presented_since_live: 3_580,
        display_refresh_hz: Some(120.0),
        pool_plan: PoolPlan {
            viewfinder: 3,
            pending_pairs: 2,
            snapshot_ring: 0,
            lossless: 0,
            slack: 1,
        },
        pool_max_images: 6,
        validation: Some(ValidationReportPart {
            mode: crate::gpu_services::ValidationState::Enabled,
            errors: 0,
            warnings: 0,
            known_deviations: Default::default(),
        }),
        thermal_start: thermal(),
        thermal_end: thermal(),
        ownership: OwnershipReport {
            frames_sampled: 500,
            frames_with_frame_number: 500,
            pairs_checked: 499,
            violations: 0,
            first_violation: None,
            interval_check_skipped: false,
            samples_over_limit: 0,
        },
        latency: LatencyReport {
            samples: 3_000,
            p50_ms: Some(38.0),
            p95_ms: Some(45.0),
            max_ms: Some(60.0),
            unmatched_presentations: 0,
            implausible: 0,
            dropped_over_limit: 0,
            clock_read_failures: 0,
            last_clock_error: None,
            meaning: super::super::raw_stream_samples::LATENCY_MEANING,
        },
        state_at_end: "streaming",
        cpu_probe_armed: false,
        events: Vec::new(),
        events_dropped: 0,
    }
}

fn verdict(change: impl FnOnce(&mut RawStreamObservation)) -> Vec<String> {
    let mut observation = healthy(RawStreamRunKind::Standard);
    change(&mut observation);
    evaluate_raw_stream(&RawStreamScenarioConfig::default(), &observation)
}

fn assert_single_violation(violations: &[String], needle: &str) {
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(violations[0].contains(needle), "{violations:?} не содержит «{needle}»");
}

#[test]
fn healthy_run_passes_in_standard_and_endurance_kinds() {
    assert_eq!(verdict(|_| {}), Vec::<String>::new());
    let endurance = healthy(RawStreamRunKind::Endurance);
    assert!(evaluate_raw_stream(&RawStreamScenarioConfig::default(), &endurance).is_empty());
}

#[test]
fn engine_failure_is_the_only_reported_problem() {
    let violations = verdict(|o| {
        o.failure = Some(RunFailure::NeverStreaming { last_state: "opening" });
    });
    assert_single_violation(&violations, "Streaming");
}

#[test]
fn fps_below_ninety_percent_of_the_target_fails_and_just_above_passes() {
    assert_single_violation(&verdict(|o| o.sensor_fps = Some(53.9)), "ниже 90%");
    assert!(verdict(|o| o.sensor_fps = Some(54.1)).is_empty());
    assert_single_violation(&verdict(|o| o.sensor_fps = None), "не измерена");
}

#[test]
fn after_a_degradation_the_target_is_the_fallback_rate_and_it_is_not_a_failure() {
    let violations = verdict(|o| {
        o.configured = Some(configured(30));
        o.sensor_fps = Some(29.8);
        o.degraded = Some(DegradedNote {
            requested_fps: 60,
            fallback_fps: 30,
            failure: "вендорские ключи недоступны".to_owned(),
        });
        // Кадров вдвое меньше: ожидание показа считается от 30 к/с, а не от 60.
        o.presented_since_live = 1_790;
    });
    assert!(violations.is_empty(), "{violations:?}");
    // А тот же fps при цели 60 был бы провалом.
    assert_single_violation(&verdict(|o| o.sensor_fps = Some(29.8)), "ниже");
}

#[test]
fn any_cpu_copy_fails() {
    assert_single_violation(&verdict(|o| o.stream.cpu_copies = 1), "cpu_copies");
}

#[test]
fn pool_exhaustion_on_either_side_fails() {
    assert_single_violation(
        &verdict(|o| o.stream.dropped_pool_exhausted = 2),
        "dropped_pool_exhausted",
    );
    assert_single_violation(&verdict(|o| o.reader.pool_exhausted = 1), "pool_exhausted");
}

#[test]
fn every_leak_counter_fails_by_name() {
    assert_single_violation(&verdict(|o| o.stream.leaked_leases = 1), "leaked_leases");
    assert_single_violation(&verdict(|o| o.reader.images_leaked = 1), "images_leaked");
    assert_single_violation(&verdict(|o| o.reader.fence_merge_failures = 1), "fence_merge_failures");
    assert_single_violation(&verdict(|o| o.stream.frames_leaked = 1), "frames_leaked");
}

#[test]
fn outstanding_peak_above_the_pool_plan_fails_and_equal_passes() {
    assert!(verdict(|o| o.stream.max_frames_outstanding = 6).is_empty());
    assert_single_violation(
        &verdict(|o| o.stream.max_frames_outstanding = 7),
        "пик кадров на руках 7",
    );
}

#[test]
fn presented_frames_below_ninety_percent_of_the_achievable_rate_fail() {
    // Экран 120 Гц, камера 60: достижимо 60 к/с, нужно 0,9 * 60 с * 60 = 3240.
    assert!(verdict(|o| o.presented_since_live = 3_240).is_empty());
    assert_single_violation(&verdict(|o| o.presented_since_live = 3_239), "на экран поставлено 3239");
}

#[test]
fn a_slow_display_lowers_the_required_presented_count() {
    // Экран 30 Гц: достижимо 30 к/с, нужно 0,9 * 60 * 30 = 1620.
    let violations = verdict(|o| {
        o.display_refresh_hz = Some(30.0);
        o.presented_since_live = 1_700;
    });
    assert!(violations.is_empty(), "{violations:?}");
}

#[test]
fn render_that_showed_nothing_fails() {
    assert_single_violation(&verdict(|o| o.render = None), "не показал ни одного кадра");
}

#[test]
fn release_fence_export_failures_fail() {
    assert_single_violation(
        &verdict(|o| {
            if let Some(render) = o.render.as_mut() {
                render.release_export_failures = 3;
            }
        }),
        "release_export_failures",
    );
}

#[test]
fn paired_fraction_threshold_is_checked_for_physical_cameras_only() {
    // 3 564 из 3 600 = 99,0 %: проходит; 3 563 — нет.
    assert!(
        verdict(|o| {
            o.stream.frames_paired = 3_564;
            o.stream.frames_missing_meta_timeout = 36;
        })
        .is_empty()
    );
    assert_single_violation(
        &verdict(|o| {
            o.stream.frames_paired = 3_563;
            o.stream.frames_missing_meta_timeout = 37;
        }),
        "с метаданными",
    );
    // Для логической камеры (HAL без финальных результатов) правило выключено конфигом.
    let mut logical = healthy(RawStreamRunKind::Standard);
    logical.stream.frames_paired = 0;
    logical.stream.frames_missing_meta_timeout = 3_600;
    logical.ownership.frames_with_frame_number = 0;
    let config = RawStreamScenarioConfig {
        expect_paired_metadata: false,
        ..RawStreamScenarioConfig::default()
    };
    assert!(evaluate_raw_stream(&config, &logical).is_empty());
}

#[test]
fn metadata_ownership_violations_fail_and_an_empty_check_is_not_a_pass() {
    assert_single_violation(
        &verdict(|o| {
            o.ownership.violations = 2;
            o.ownership.first_violation = Some("номер кадра не растёт".to_owned());
        }),
        "перепутаны",
    );
    assert_single_violation(
        &verdict(|o| o.ownership.frames_with_frame_number = 0),
        "владение метаданными не проверено",
    );
}

#[test]
fn validation_errors_fail_only_when_the_layer_is_enabled() {
    assert_single_violation(
        &verdict(|o| {
            if let Some(validation) = o.validation.as_mut() {
                validation.errors = 1;
            }
        }),
        "validation",
    );
    assert!(
        verdict(|o| {
            if let Some(validation) = o.validation.as_mut() {
                validation.mode = crate::gpu_services::ValidationState::Disabled;
                validation.errors = 1;
            }
        })
        .is_empty(),
        "в release слоя нет, нули ничего не доказывают и ошибки не судятся"
    );
}

#[test]
fn engine_not_streaming_at_the_end_fails() {
    assert_single_violation(&verdict(|o| o.state_at_end = "recovering"), "не в Streaming");
}

// ---- cpu-probe ----

fn probe_verdict(change: impl FnOnce(&mut RawStreamObservation)) -> Vec<String> {
    let mut observation = healthy(RawStreamRunKind::CpuProbe);
    observation.cpu_probe_armed = true;
    observation.stream.cpu_copies = 300;
    observation.reader.cpu_access_probe_copies = 300;
    change(&mut observation);
    evaluate_raw_stream(&RawStreamScenarioConfig::default(), &observation)
}

#[test]
fn probe_passes_only_when_copies_were_counted() {
    assert!(probe_verdict(|_| {}).is_empty());
    assert_single_violation(&probe_verdict(|o| o.stream.cpu_copies = 0), "не подключён");
}

#[test]
fn probe_refused_by_the_platform_is_reported_explicitly() {
    let violations = probe_verdict(|o| {
        o.stream.cpu_copies = 0;
        o.reader.cpu_access_probe_failures = 12;
    });
    assert_single_violation(&violations, "платформа отвергла пробу");
}

#[test]
fn probe_without_frames_or_without_arming_names_that_cause() {
    assert_single_violation(
        &probe_verdict(|o| {
            o.stream.cpu_copies = 0;
            o.stream.frames_offered = 0;
        }),
        "ни одного кадра",
    );
    assert_single_violation(
        &probe_verdict(|o| {
            o.stream.cpu_copies = 0;
            o.cpu_probe_armed = false;
        }),
        "не была взведена",
    );
}

#[test]
fn probe_run_does_not_apply_the_zero_copy_rules() {
    // В пробе копии — это цель, а не нарушение; остальные правила потока не применяются.
    let violations = probe_verdict(|o| o.sensor_fps = None);
    assert!(violations.is_empty(), "{violations:?}");
}

// ---- сводка ----

#[test]
fn summary_reports_cpu_copies_per_frame_and_fits_the_budget() {
    let mut observation = healthy(RawStreamRunKind::Standard);
    observation.stream.cpu_copies = 36;
    let summary = observation.summary_within(3_000);
    let json = serde_json::to_value(&summary).expect("сериализуется");
    assert!((json["cpu_copies_per_frame"].as_f64().expect("число") - 0.01).abs() < 1e-9);
    assert_eq!(json["kind"], "raw-stream");
    assert_eq!(json["target_fps"], 60);
    assert!(serde_json::to_string(&summary).expect("json").len() <= 3_000);
}

#[test]
fn summary_carries_arrival_age_in_milliseconds_even_when_counters_are_dropped() {
    let mut observation = healthy(RawStreamRunKind::Standard);
    observation.reader.arrival_age = ArrivalAgeSnapshot {
        samples: 4,
        total_micros: 160_000,
        min_micros: Some(30_000),
        max_micros: Some(52_500),
        clock_failures: 0,
        negative: 0,
    };
    for budget in [3_000, 10] {
        let json = serde_json::to_value(observation.summary_within(budget)).expect("сериализуется");
        assert_eq!(
            json["arrival_age"],
            serde_json::json!({ "mean_ms": 40.0, "min_ms": 30.0, "max_ms": 52.5, "samples": 4 }),
            "бюджет {budget}"
        );
    }
}

#[test]
fn arrival_age_without_samples_reports_no_numbers() {
    let observation = healthy(RawStreamRunKind::Standard);
    let json = serde_json::to_value(observation.summary_within(3_000)).expect("сериализуется");
    assert_eq!(json["arrival_age"], serde_json::json!({ "samples": 0 }));
}

#[test]
fn summary_that_does_not_fit_drops_the_big_counter_blocks_and_says_so() {
    let observation = healthy(RawStreamRunKind::Standard);
    let summary = observation.summary_within(10);
    let json = serde_json::to_value(&summary).expect("сериализуется");
    assert_eq!(json["degraded_report"], true);
    assert!(json.get("stream").is_none() && json.get("reader").is_none());
    assert_eq!(json["sensor_fps"], 59.9, "ключевые числа остаются");
}

#[test]
fn detail_items_carry_all_counters_for_the_stage_lines() {
    let observation = healthy(RawStreamRunKind::Standard);
    let items = observation.detail_items();
    assert_eq!(items[0]["stream"]["frames_offered"], 3_600);
    assert_eq!(items[0]["pool_plan"]["viewfinder"], 3);
    assert_eq!(items[1]["reader"]["images_returned"], 3_598);
    assert_eq!(items[2]["render"]["camera_frames_presented"], 3_590);
}

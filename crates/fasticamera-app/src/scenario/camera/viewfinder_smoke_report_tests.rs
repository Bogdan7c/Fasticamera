use std::collections::BTreeMap;

use raw_frame_stream::RawStreamCounters;

use super::*;
use crate::gpu_services::ValidationState;
use crate::scenario::camera::raw_stream_samples::LATENCY_MEANING;
use crate::scenario::camera::viewfinder_metrics::IspCountersDelta;
use crate::viewfinder_control::PresentTargetSetting;
use render_loop::config::{AdpfMode, DepthReductionMode, LateLatchMode, PresentTimelinePolicy};

fn session() -> SessionSummary {
    SessionSummary {
        configured: None,
        degraded: None,
        time_to_streaming_ms: Some(900),
        time_to_live_ms: Some(1_100),
        state_at_end: "streaming",
        events: Vec::new(),
    }
}

fn latency(samples: u64) -> LatencyReport {
    LatencyReport {
        samples,
        p50_ms: (samples > 0).then_some(40.0),
        p95_ms: (samples > 0).then_some(45.0),
        max_ms: (samples > 0).then_some(50.0),
        unmatched_presentations: 0,
        implausible: 0,
        dropped_over_limit: 0,
        clock_read_failures: 0,
        last_clock_error: None,
        meaning: LATENCY_MEANING,
    }
}

fn time(p50_ms: f64, p95_ms: f64) -> GpuTimeMs {
    GpuTimeMs {
        samples: 600,
        p50_ms,
        p95_ms,
        max_ms: p95_ms + 0.5,
    }
}

fn thermal() -> ThermalSnapshot {
    ThermalSnapshot {
        status: Some(shell_contract::thermal::ThermalStatus::None),
        headroom: Some(0.5),
        error: None,
    }
}

/// Минута 60 к/с, каждый показанный кадр прошёл ISP, всё в бюджете.
fn healthy() -> ViewfinderSmokeObservation {
    ViewfinderSmokeObservation {
        failure: None,
        session: session(),
        sensor_fps: Some(60.0),
        window: WindowDelta {
            window_ms: 60_000,
            camera_frames_presented: 3_600,
            isp: IspCountersDelta {
                frames_processed: 3_600,
                params_resolved: 3_600,
                statistics_published: 3_600,
                ..IspCountersDelta::default()
            },
            ..WindowDelta::default()
        },
        // Минута при 60,03 Гц камеры против 60,00 Гц экрана: два переполнения буфера (дрейф).
        pacing: PacingWindowReport::of_window(
            &RawStreamCounters {
                frames_published: 3_602,
                viewfinder_delivered: 3_600,
                dropped_jitter_overflow: 2,
                ..RawStreamCounters::default()
            },
            0,
            60_000,
        ),
        gpu_time: ViewfinderGpuTimesMs {
            isp_dispatch: Some(time(1.8, 2.4)),
            frame_isp_span: Some(time(2.0, 2.6)),
            compose: Some(time(0.4, 0.5)),
            frame_total: Some(time(2.6, 3.3)),
        },
        look: Some(ViewfinderLook {
            painter: "isp",
            all_stages: true,
            display_color_space: "display_p3",
            storage: Some("ten_bit_unorm"),
            output_extent: Some([1440, 1080]),
        }),
        validation: Some(ValidationReportPart {
            mode: ValidationState::Enabled,
            errors: 0,
            warnings: 0,
            known_deviations: BTreeMap::new(),
        }),
        latency: latency(3_000),
        present_target: Some(PresentTargetReport {
            selection: PresentTargetSetting {
                policy: PresentTimelinePolicy::Preferred,
                late_latch: LateLatchMode::Disabled,
                depth_reduction: DepthReductionMode::Disabled,
            },
            gpu_source: "measured",
            estimated_cpu_ms: 2.1,
            estimated_gpu_ms: 9.4,
            frame_gpu_time: Some(time(8.9, 10.2)),
        }),
        adpf: Some(adpf_report(AdpfMode::Disabled, "disabled")),
        acquire_wait: None,
        latch_to_submit: None,
        thermal_start: thermal(),
        thermal_end: thermal(),
        last_isp_failure: None,
        last_params_failure: None,
    }
}

fn adpf_report(mode: AdpfMode, session: &'static str) -> AdpfReport {
    AdpfReport {
        mode,
        session,
        reports_sent: 0,
        report_failures: 0,
        last_error: None,
    }
}

fn violations_of(change: impl FnOnce(&mut ViewfinderSmokeObservation)) -> Vec<String> {
    let mut observation = healthy();
    change(&mut observation);
    evaluate_viewfinder(&ViewfinderSmokeConfig::default(), &observation)
}

fn assert_single(violations: &[String], needle: &str) {
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(violations[0].contains(needle), "нет «{needle}» в {violations:?}");
}

#[test]
fn healthy_minute_passes() {
    assert_eq!(violations_of(|_| {}), Vec::<String>::new());
}

#[test]
fn drift_level_jitter_overflow_passes_and_is_reported_apart_from_superseded() {
    let mut observation = healthy();
    observation.pacing = PacingWindowReport::of_window(
        &RawStreamCounters {
            frames_published: 3_610,
            viewfinder_delivered: 3_600,
            dropped_jitter_overflow: 4,
            dropped_superseded: 6,
            ..RawStreamCounters::default()
        },
        0,
        60_000,
    );

    // Четыре переполнения за минуту — ровно допуск по умолчанию (4 в минуту): проходит.
    assert_eq!(
        evaluate_viewfinder(&ViewfinderSmokeConfig::default(), &observation),
        Vec::<String>::new()
    );
    // Потери не спрятаны: обе причины видны в сводке раздельно.
    let summary = observation.summary();
    assert_eq!(summary["pacing"]["jitter_overflow"], 4);
    assert_eq!(summary["pacing"]["superseded"], 6);
    assert_eq!(summary["pacing"]["dropped"], 10);
}

#[test]
fn jitter_overflow_beyond_drift_fails_with_the_count_and_the_allowance() {
    let violations = violations_of(|observation| {
        observation.pacing.jitter_overflow = 5;
    });

    assert_single(&violations, "джиттер-буфер переполнился 5 раз");
    assert!(violations[0].contains("допустимо 4"), "{violations:?}");
}

#[test]
fn superseded_frames_alone_do_not_fail_the_verdict() {
    // Вытеснение — норма LatestWins; оно считается и показывается, но вердикт им не судит.
    let violations = violations_of(|observation| {
        observation.pacing.superseded = 500;
        observation.pacing.dropped = 500;
    });

    assert_eq!(violations, Vec::<String>::new());
}

#[test]
fn overflow_allowance_scales_with_the_window_and_is_at_least_one() {
    let config = ViewfinderSmokeConfig::default();

    assert_eq!(max_jitter_overflow_in_window(&config, 60_000), 4);
    assert_eq!(max_jitter_overflow_in_window(&config, 120_000), 8);
    assert_eq!(max_jitter_overflow_in_window(&config, 10_000), 1, "ceil(0,67) = 1");
    assert_eq!(
        max_jitter_overflow_in_window(&config, 0),
        1,
        "пустое окно: не меньше одного"
    );
}

#[test]
fn failure_is_the_only_violation_reported() {
    let violations = violations_of(|observation| {
        observation.failure = Some(RunFailure::StreamInterrupted { state: "error" });
        observation.sensor_fps = None;
    });
    assert_single(&violations, "прервался");
}

#[test]
fn slow_sensor_fails() {
    assert_single(
        &violations_of(|observation| observation.sensor_fps = Some(50.0)),
        "ниже 57",
    );
    assert_single(
        &violations_of(|observation| observation.sensor_fps = None),
        "не измерена",
    );
}

#[test]
fn isp_must_keep_up_with_the_sensor_and_with_presented_frames() {
    // ISP пропускает каждый десятый кадр сенсора и показа.
    let violations = violations_of(|observation| observation.window.isp.frames_processed = 3_240);
    assert_eq!(violations.len(), 2, "{violations:?}");
    assert!(violations[0].contains("кадров/с"));
    assert!(violations[1].contains("каждый показанный"));
    // Расхождение в 1 % — в пределах допуска 3 %.
    assert!(violations_of(|observation| observation.window.isp.frames_processed = 3_564).is_empty());
    assert_single(
        &violations_of(|observation| {
            observation.window.camera_frames_presented = 0;
        }),
        "ни одного нового кадра",
    );
}

#[test]
fn isp_dispatch_over_budget_or_missing_fails_and_missing_frame_total_fails() {
    assert_single(
        &violations_of(|observation| observation.gpu_time.isp_dispatch = Some(time(2.5, 3.2))),
        "больше бюджета 3",
    );
    assert_single(
        &violations_of(|observation| observation.gpu_time.isp_dispatch = None),
        "isp_dispatch",
    );
    assert_single(
        &violations_of(|observation| observation.gpu_time.frame_total = None),
        "frame_total",
    );
}

#[test]
fn any_cpu_copy_or_validation_error_fails() {
    assert_single(
        &violations_of(|observation| observation.window.stream_cpu_copies = 1),
        "CPU-копий",
    );
    assert_single(
        &violations_of(|observation| observation.window.isp.cpu_copies = 2),
        "CPU-копий кадров: 2",
    );
    let violations = violations_of(|observation| {
        if let Some(validation) = &mut observation.validation {
            validation.errors = 3;
        }
    });
    assert_eq!(violations.len(), 1, "{violations:?}");
    // В release слоя нет: нули ничего не доказывают и ошибкой не считаются.
    assert!(
        violations_of(|observation| {
            if let Some(validation) = &mut observation.validation {
                validation.mode = ValidationState::Disabled;
                validation.errors = 3;
            }
        })
        .is_empty()
    );
}

#[test]
fn every_isp_counter_rule_is_checked() {
    // Как на телефоне: параметры разрешены один раз до окна, приращение за окно 0 — это не провал.
    assert!(
        violations_of(|observation| observation.window.isp.params_resolved = 0).is_empty(),
        "params_resolved = 0 за окно — норма: параметры разрешаются при старте потока"
    );
    for (name, change) in [
        (
            "params_failed",
            (|isp: &mut IspCountersDelta| isp.params_failed = 1) as fn(&mut IspCountersDelta),
        ),
        ("isp_record_failures", |isp| isp.isp_record_failures = 1),
        ("compose_failures", |isp| isp.compose_failures = 1),
        ("frames_dropped_no_params", |isp| isp.frames_dropped_no_params = 1),
        ("frames_rejected_geometry", |isp| isp.frames_rejected_geometry = 1),
    ] {
        assert_single(&violations_of(|observation| change(&mut observation.window.isp)), name);
    }
    assert_single(
        &violations_of(|observation| observation.window.isp.statistics_published = 0),
        "AWB",
    );
}

#[test]
fn latency_must_be_measured_and_viewfinder_must_be_the_product_one() {
    assert_single(
        &violations_of(|observation| observation.latency = latency(0)),
        "задержка",
    );
    assert_single(
        &violations_of(|observation| {
            if let Some(look) = &mut observation.look {
                look.painter = "debug_mosaic";
            }
        }),
        "не продуктовый",
    );
    assert_single(
        &violations_of(|observation| {
            if let Some(look) = &mut observation.look {
                look.all_stages = false;
            }
        }),
        "не продуктовый",
    );
    assert_single(
        &violations_of(|observation| {
            if let Some(look) = &mut observation.look {
                look.display_color_space = "unknown";
            }
        }),
        "цветовое пространство",
    );
    assert_single(&violations_of(|observation| observation.look = None), "нет диагностики");
    // sRGB — честный ответ, не нарушение.
    assert!(
        violations_of(|observation| {
            if let Some(look) = &mut observation.look {
                look.display_color_space = "srgb";
            }
        })
        .is_empty()
    );
}

#[test]
fn enabled_adpf_with_failed_session_fails_but_unsupported_and_disabled_do_not() {
    let failed = violations_of(|observation| {
        let mut report = adpf_report(AdpfMode::Enabled, "failed");
        report.last_error = Some("APerformanceHint_createSession вернул null".to_owned());
        observation.adpf = Some(report);
    });
    assert_single(&failed, "ADPF включён");
    assert!(failed[0].contains("createSession"), "{failed:?}");

    for (mode, session) in [
        (AdpfMode::Enabled, "unsupported"),
        (AdpfMode::Enabled, "active"),
        (AdpfMode::Disabled, "failed"),
    ] {
        assert!(
            violations_of(|observation| observation.adpf = Some(adpf_report(mode, session))).is_empty(),
            "{mode:?} / {session}"
        );
    }
}

#[test]
fn summary_reports_adpf_and_present_target_selection() {
    let mut observation = healthy();
    let mut report = adpf_report(AdpfMode::Enabled, "active");
    report.reports_sent = 3_500;
    observation.adpf = Some(report);

    let summary = observation.summary();

    assert_eq!(summary["adpf"]["mode"], "enabled");
    assert_eq!(summary["adpf"]["session"], "active");
    assert_eq!(summary["adpf"]["reports_sent"], 3_500);
    let selection = &summary["present_target"]["selection"];
    assert_eq!(selection["policy"]["kind"], "preferred");
    assert_eq!(selection["late_latch"], "disabled");
    assert_eq!(selection["gpu_source"], "measured");
    assert_eq!(selection["estimated_gpu_ms"], 9.4);
    assert_eq!(selection["frame_gpu_time"]["p50_ms"], 8.9);
    assert_eq!(selection["frame_gpu_time"]["p95_ms"], 10.2);
}

#[test]
fn summary_fits_the_result_line_budget_and_carries_the_key_numbers() {
    let summary = healthy().summary();
    let bytes = summary.to_string().len();
    assert!(bytes < 2_900, "сводка {bytes} байт");
    assert_eq!(summary["isp_fps"], 60.0);
    assert_eq!(summary["look"]["display_color_space"], "display_p3");
    assert_eq!(summary["gpu_time"]["isp_dispatch"]["p95_ms"], 2.4);
    assert_eq!(summary["awb_published"], 3_600);
    assert_eq!(summary["latency"]["p50_ms"], 40.0);
}

/// Худший провал: длинные причины на кириллице во всех текстовых полях и все правила нарушены
/// разом. Строка результата обязана влезть в запись logcat и сохранить сводку и первое нарушение.
#[test]
fn worst_case_failure_line_fits_the_logcat_record() {
    use crate::scenario::camera::result_budget::judge_within_budget;
    use crate::scenario::camera::result_budget::worst_case::{
        assert_failure_line_fits, long_failure, long_reason, worst_session,
    };

    let mut observation = healthy();
    observation.session = worst_session();
    observation.sensor_fps = Some(1.0);
    observation.window.camera_frames_presented = 0;
    observation.window.isp.params_resolved = 0;
    observation.window.isp.params_failed = 9;
    observation.window.isp.isp_record_failures = 9;
    observation.window.isp.compose_failures = 9;
    observation.window.isp.frames_dropped_no_params = 9;
    observation.window.isp.frames_rejected_geometry = 9;
    observation.window.isp.statistics_published = 0;
    observation.window.isp.cpu_copies = 9;
    observation.gpu_time = ViewfinderGpuTimesMs::default();
    observation.latency = latency(0);
    observation.look = None;
    observation.thermal_start.error = Some(long_reason("тепло"));
    observation.thermal_end.error = Some(long_reason("тепло"));
    let mut worst_adpf = adpf_report(AdpfMode::Enabled, "failed");
    worst_adpf.last_error = Some("ж".repeat(120));
    observation.adpf = Some(worst_adpf);
    observation.last_isp_failure = Some(long_reason("ISP"));
    observation.last_params_failure = Some(long_reason("параметры"));
    let mut violations = evaluate_viewfinder(&ViewfinderSmokeConfig::default(), &observation);
    assert!(violations.len() >= 8, "{violations:?}");
    observation.failure = Some(long_failure());
    violations.push(long_failure().describe());

    let verdict = judge_within_budget(observation.summary(), violations.clone());
    let line = assert_failure_line_fits("viewfinder", verdict.outcome);
    let error = line["error"].as_str().expect("текст ошибки");
    assert!(error.starts_with(&violations[0]), "первое нарушение целиком: {error}");
    assert_eq!(
        line["report"]["frames_processed"], 3_600,
        "ключевые числа сводки на месте"
    );
    assert_eq!(
        verdict.violation_details.len(),
        violations.len(),
        "полный список — в деталях"
    );
}

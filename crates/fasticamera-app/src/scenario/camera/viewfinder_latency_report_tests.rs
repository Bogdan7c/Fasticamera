use render_loop::config::AdpfMode;

use super::*;
use crate::scenario::camera::raw_stream_samples::LATENCY_MEANING;
use crate::scenario::camera::viewfinder_latency_verdict::compare_profiles;
use crate::scenario::camera::viewfinder_metrics::{GpuTimeMs, VsyncShiftDelta};
use crate::scenario::camera::viewfinder_pacing_report::PacingWindowReport;
use crate::scenario::camera::viewfinder_tuning_report::AdpfWindowDelta;
use crate::viewfinder_latency_profile::default_latency_profiles;

fn latency(samples: u64, p50_ms: f64) -> LatencyReport {
    LatencyReport {
        samples,
        p50_ms: Some(p50_ms),
        p95_ms: Some(p50_ms + 5.0),
        max_ms: Some(p50_ms + 10.0),
        unmatched_presentations: 0,
        implausible: 0,
        dropped_over_limit: 0,
        clock_read_failures: 0,
        last_clock_error: None,
        meaning: LATENCY_MEANING,
    }
}

/// Окно `index`: p50 задержки и число опоздавших на 1 vsync кадров из 1000.
fn window(index: usize, p50_ms: f64, late: u64) -> LatencyWindow {
    let profile = ProfileSlot::of_window(index);
    LatencyWindow {
        index,
        pair: index / 2,
        profile,
        measured_ms: 17_000,
        processed_fps: Some(58.0),
        sensor_fps: Some(60.0),
        superseded: 20,
        superseded_per_s: Some(1.2),
        pacing: PacingWindowReport::default(),
        latency: latency(1_000, p50_ms),
        present_target: PresentTargetDelta {
            frames_planned: 1_000,
            vsync_shift: VsyncShiftDelta {
                on_time: 1_000 - late,
                late_1: late,
                ..VsyncShiftDelta::default()
            },
            ..PresentTargetDelta::default()
        },
        adpf: AdpfWindowReport {
            mode: if profile == ProfileSlot::Tuned {
                AdpfMode::Enabled
            } else {
                AdpfMode::Disabled
            },
            session: "active",
            delta: AdpfWindowDelta::default(),
        },
        frame_gpu_time: None,
        selection_matched: true,
    }
}

/// Шесть окон: база даёт `baseline_ms`, проверяемый профиль — `tuned_ms`.
fn six_windows(baseline_ms: f64, tuned_ms: f64, late: u64) -> Vec<LatencyWindow> {
    (0..6)
        .map(|index| match ProfileSlot::of_window(index) {
            ProfileSlot::Baseline => window(index, baseline_ms, late),
            ProfileSlot::Tuned => window(index, tuned_ms, late),
        })
        .collect()
}

fn observation(windows: Vec<LatencyWindow>) -> LatencyObservation {
    let config = ViewfinderLatencyConfig::default();
    LatencyObservation {
        failure: None,
        session: SessionSummary {
            configured: None,
            degraded: None,
            time_to_streaming_ms: None,
            time_to_live_ms: None,
            state_at_end: "streaming",
            events: Vec::new(),
        },
        profiles: default_latency_profiles(),
        expected_windows: 6,
        comparison: compare_profiles(&config, &windows),
        windows,
        undelivered: Vec::new(),
    }
}

#[test]
fn valid_measurement_passes_whatever_the_policy_verdict() {
    let config = ViewfinderLatencyConfig::default();
    assert!(evaluate_latency(&config, &observation(six_windows(48.0, 40.0, 10))).is_empty());
    assert!(evaluate_latency(&config, &observation(six_windows(40.0, 48.0, 90))).is_empty());
}

#[test]
fn measurement_problems_fail_the_scenario() {
    let config = ViewfinderLatencyConfig::default();
    let mut few = six_windows(48.0, 40.0, 10);
    few[3].latency.samples = 10;
    let violations = evaluate_latency(&config, &observation(few));
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(violations[0].contains("окно 3"));

    let mut stray = six_windows(48.0, 40.0, 10);
    stray[1].selection_matched = false;
    assert!(evaluate_latency(&config, &observation(stray))[0].contains("не с настройками профиля"));

    // ADPF включён, а сессия не создалась: профиль не проверен — замер негоден. Unsupported — нет.
    let mut adpf_failed = six_windows(48.0, 40.0, 10);
    adpf_failed[1].adpf.session = "failed";
    let violations = evaluate_latency(&config, &observation(adpf_failed));
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(
        violations[0].contains("окно 1 (tuned)") && violations[0].contains("ADPF"),
        "{violations:?}"
    );
    let mut adpf_unsupported = six_windows(48.0, 40.0, 10);
    adpf_unsupported[1].adpf.session = "unsupported";
    assert!(evaluate_latency(&config, &observation(adpf_unsupported)).is_empty());

    let mut short = observation(six_windows(48.0, 40.0, 10));
    short.windows.truncate(4);
    assert!(evaluate_latency(&config, &short)[0].contains("4 из 6"));

    let mut undelivered = observation(six_windows(48.0, 40.0, 10));
    undelivered.undelivered.push("окно 0: RenderGone".to_owned());
    assert!(evaluate_latency(&config, &undelivered)[0].contains("не доставлена"));

    let mut failed = observation(Vec::new());
    failed.failure = Some(RunFailure::NeverStreaming { last_state: "opening" });
    assert_eq!(evaluate_latency(&config, &failed).len(), 1);
}

#[test]
fn summary_fits_the_result_line() {
    let summary = observation(six_windows(48.0, 40.0, 10)).summary();
    let bytes = summary.to_string().len();
    assert!(bytes < 2_900, "сводка {bytes} байт");
    assert_eq!(summary["comparison"]["verdict"]["verdict"], "tuned_better");
    assert_eq!(summary["comparison"]["latency_gain_ms"], 8.0);
    assert_eq!(summary["profiles"][0]["name"], "baseline");
    assert_eq!(summary["profiles"][1]["adpf"], "enabled");
    assert_eq!(summary["comparison"]["tuned"]["windows"], 3);
}

/// Худший провал при наибольшем допустимом числе пар: каждое окно нарушает оба правила, каждая
/// регулировка не доставлена с длинной причиной. Строка результата обязана влезть в запись logcat.
#[test]
fn worst_case_failure_line_with_max_pairs_fits_the_logcat_record() {
    use crate::scenario::camera::result_budget::judge_within_budget;
    use crate::scenario::camera::result_budget::worst_case::{
        assert_failure_line_fits, long_failure, long_reason, worst_session,
    };
    use crate::viewfinder_scenario_config::MAX_LATENCY_PAIRS;

    let config = ViewfinderLatencyConfig::default();
    let window_count = usize::try_from(MAX_LATENCY_PAIRS).expect("пары") * 2;
    let windows: Vec<LatencyWindow> = (0..window_count)
        .map(|index| {
            let mut window = window(index, 40.0 + index as f64, 900);
            window.latency.samples = 1;
            window.latency.last_clock_error = Some(long_reason("часы"));
            window.selection_matched = false;
            // Самые «длинные» числа: все сводки заполнены и каждое с дробной частью.
            window.processed_fps = Some(59.99 - index as f64 * 0.01);
            window.sensor_fps = Some(119.97);
            window.superseded_per_s = Some(1_234.56);
            window.frame_gpu_time = Some(GpuTimeMs {
                samples: 600,
                p50_ms: 12.34,
                p95_ms: 15.67,
                max_ms: 99.99,
            });
            window.adpf.session = "unsupported";
            window
        })
        .collect();
    let mut observation = observation(windows);
    observation.session = worst_session();
    // Самые длинные допустимые имена профилей: 16 символов кириллицей = 32 байта.
    for profile in &mut observation.profiles {
        profile.name = "ж".repeat(16);
    }
    observation.expected_windows = window_count + 1;
    observation.undelivered = (0..window_count)
        .map(|index| long_reason(&format!("окно {index}")))
        .collect();
    let mut violations = evaluate_latency(&config, &observation);
    assert!(violations.len() > 3 * window_count, "{}", violations.len());
    observation.failure = Some(long_failure());
    violations.push(long_failure().describe());

    let verdict = judge_within_budget(observation.summary(), violations.clone());
    let line = assert_failure_line_fits("viewfinder-latency", verdict.outcome);
    assert!(
        line["error"].as_str().is_some_and(|error| error.contains("…ещё ")),
        "{line}"
    );
    assert_eq!(line["report"]["windows"], window_count);
    assert_eq!(
        line["report"]["comparison"]["tuned"]["windows"],
        window_count / 2,
        "обе сводки профилей на месте"
    );
    assert_eq!(line["report"]["profiles"][1]["name"], "ж".repeat(16));
    assert_eq!(line["report"]["undelivered"]["count"], window_count);
    assert_eq!(
        line["report"]["comparison"]["pair_p50_deltas_ms"]
            .as_array()
            .map(Vec::len),
        Some(window_count / 2)
    );
    assert_eq!(verdict.violation_details.len(), violations.len());
}

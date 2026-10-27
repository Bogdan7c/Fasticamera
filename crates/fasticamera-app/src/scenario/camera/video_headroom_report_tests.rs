use std::time::Duration;

use render_loop::thread::DurationSummary;
use render_loop::viewfinder::{HeadroomPassTime, ViewfinderExtent};
use shell_contract::thermal::ThermalStatus;

use super::*;
use crate::power_measurement::ThermalSnapshot;
use crate::power_measurement::endurance::EnduranceTotals;

fn summary(p50_ms: u64, p95_ms: u64) -> DurationSummary {
    DurationSummary {
        samples: 600,
        p50: Duration::from_millis(p50_ms),
        p95: Duration::from_millis(p95_ms),
        max: Duration::from_millis(p95_ms + 1),
    }
}

fn bench(total_p95_ms: u64) -> HeadroomBenchDiagnostics {
    HeadroomBenchDiagnostics {
        mode: HeadroomBench::Enabled,
        workload_extent: Some(ViewfinderExtent {
            width: 4096,
            height: 2304,
        }),
        frames_recorded: 36_100,
        pass_times: vec![
            HeadroomPassTime {
                pass: "levels",
                gpu_time: summary(1, 1),
            },
            HeadroomPassTime {
                pass: "temporal_denoise",
                gpu_time: summary(3, 4),
            },
        ],
        total: Some(summary(total_p95_ms - 2, total_p95_ms)),
        ..HeadroomBenchDiagnostics::default()
    }
}

#[test]
fn snapshot_counts_frames_since_start_and_lists_passes_in_ms() {
    let snapshot = BenchSnapshot::of(&bench(12), 100);
    assert!(snapshot.enabled);
    assert_eq!(snapshot.frames_recorded, 36_000);
    assert_eq!(snapshot.workload_extent, Some([4096, 2304]));
    assert_eq!(snapshot.passes[1].pass, "temporal_denoise");
    assert_eq!(snapshot.passes[1].gpu_time.p95_ms, 4.0);
    assert_eq!(snapshot.total.map(|total| total.p95_ms), Some(12.0));
}

#[test]
fn frame_budget_is_judged_by_total_p95() {
    assert_eq!(
        fits_frame_budget(Some(&BenchSnapshot::of(&bench(12), 0)), 16.67),
        Some(true)
    );
    assert_eq!(
        fits_frame_budget(Some(&BenchSnapshot::of(&bench(18), 0)), 16.67),
        Some(false)
    );
    assert_eq!(fits_frame_budget(None, 16.67), None);
}

#[test]
fn refresh_is_derived_from_the_driver_period() {
    assert_eq!(
        DisplayDuringBench::refresh_hz_of(Some(Duration::from_nanos(8_333_333))),
        Some(120.0)
    );
    assert_eq!(DisplayDuringBench::refresh_hz_of(Some(Duration::ZERO)), None);
    assert_eq!(DisplayDuringBench::refresh_hz_of(None), None);
}

fn observation(bench_snapshot: Option<BenchSnapshot>, minutes: usize) -> HeadroomObservation {
    let thermal = ThermalSnapshot {
        status: Some(ThermalStatus::Light),
        headroom: Some(0.7),
        error: None,
    };
    HeadroomObservation {
        failure: None,
        session: SessionSummary {
            configured: None,
            degraded: None,
            time_to_streaming_ms: None,
            time_to_live_ms: None,
            state_at_end: "streaming",
            events: Vec::new(),
        },
        sensor_fps: Some(60.0),
        frame_budget_ms: 16.67,
        fits_frame_budget: fits_frame_budget(bench_snapshot.as_ref(), 16.67),
        bench: bench_snapshot,
        display: DisplayDuringBench {
            requested_hz: 120.0,
            request: Some(FrameRateOutcome::Applied),
            refresh_hz: Some(60.0),
            pacing_miss_ratio: 0.0,
            pacing_intervals: 100,
        },
        expected_minutes: 10,
        minutes: Vec::new(),
        endurance: EnduranceReport {
            discharge_sign: None,
            start: thermal.clone(),
            totals: EnduranceTotals {
                measured_minutes: 0,
                rejected_minutes: 0,
                mean_discharge_ma: None,
                charge_used_mah: None,
                battery_temperature_start: None,
                battery_temperature_end: None,
                worst_thermal_status: thermal.status,
                peak_headroom: thermal.headroom,
            },
            minutes: (1..=minutes)
                .map(|minute| crate::power_measurement::endurance::EnduranceSnapshot {
                    minute: u32::try_from(minute).unwrap_or(0),
                    power: crate::power_measurement::endurance::MinutePower::Rejected {
                        rejection: crate::power_measurement::WindowRejection::NoCurrentReadings,
                    },
                    observation: crate::power_measurement::MinuteObservation {
                        thermal: thermal.clone(),
                        sensor_fps: Some(60.0),
                        isp_gpu_time: None,
                    },
                })
                .collect(),
            samples_before_start: 0,
        },
        undelivered: Vec::new(),
    }
}

#[test]
fn over_budget_and_60_hz_are_honest_numbers_not_failures() {
    let slow = observation(Some(BenchSnapshot::of(&bench(20), 0)), 10);
    assert_eq!(slow.fits_frame_budget, Some(false));
    assert!(evaluate_headroom(&slow).is_empty(), "цифра для плана video, не правило");
    let summary = slow.summary();
    assert_eq!(summary["display"]["refresh_hz"], 60.0);
    assert!(summary.to_string().len() < 2_900);
}

#[test]
fn bench_that_did_not_work_fails() {
    assert!(evaluate_headroom(&observation(None, 10))[0].contains("нет диагностики"));
    let mut off = BenchSnapshot::of(&bench(12), 0);
    off.enabled = false;
    assert!(evaluate_headroom(&observation(Some(off), 10))[0].contains("не включён"));
    let idle = BenchSnapshot::of(&bench(12), 36_100);
    assert!(evaluate_headroom(&observation(Some(idle), 10))[0].contains("ни одного кадра"));
    let mut broken = BenchSnapshot::of(&bench(12), 0);
    broken.failures = 1;
    broken.last_failure = Some("нет памяти".to_owned());
    assert!(evaluate_headroom(&observation(Some(broken), 10))[0].contains("нет памяти"));
    let mut untimed = BenchSnapshot::of(&bench(12), 0);
    untimed.total = None;
    assert!(evaluate_headroom(&observation(Some(untimed), 10))[0].contains("нет замеров"));
    assert!(evaluate_headroom(&observation(Some(BenchSnapshot::of(&bench(12), 0)), 3))[0].contains("3 из 10"));
}

/// Худший провал: стенд выключен, ни одного кадра, отказы с длинной причиной (по 16 проходов
/// времени — с запасом над 11 проходами Menon + уровни, шумодав, выход), регулировки не доставлены,
/// прогон не дошёл до конца. Строка результата обязана влезть в запись logcat; полная причина
/// отказа стенда — в подробностях.
#[test]
fn worst_case_failure_line_fits_the_logcat_record_and_keeps_the_full_reason_in_details() {
    use crate::scenario::camera::result_budget::judge_within_budget;
    use crate::scenario::camera::result_budget::worst_case::{
        assert_failure_line_fits, long_failure, long_reason, worst_session,
    };

    const PASS_NAMES: [&str; 16] = [
        "levels",
        "menon_green_h",
        "menon_green_v",
        "menon_green_select",
        "menon_red_blue_at_green",
        "menon_red_blue_at_blue",
        "menon_red_blue_at_red",
        "menon_refine_green",
        "menon_refine_red_blue",
        "temporal_denoise",
        "output",
        "pass_11",
        "pass_12",
        "pass_13",
        "pass_14",
        "pass_15",
    ];
    let mut diagnostics = bench(12);
    diagnostics.pass_times = PASS_NAMES
        .iter()
        .map(|pass| HeadroomPassTime {
            pass,
            gpu_time: summary(100, 200),
        })
        .collect();
    let mut snapshot = BenchSnapshot::of(&diagnostics, 36_100);
    snapshot.enabled = false;
    snapshot.failures = 9;
    let full_reason = long_reason("стенд");
    snapshot.last_failure = Some(full_reason.clone());
    let mut observation = observation(Some(snapshot), 2);
    observation.session = worst_session();
    observation.undelivered = (0..40)
        .map(|index| long_reason(&format!("регулировка {index}")))
        .collect();
    let mut violations = evaluate_headroom(&observation);
    assert!(violations.len() > 40, "{}", violations.len());
    observation.failure = Some(long_failure());
    violations.push(long_failure().describe());

    let verdict = judge_within_budget(observation.summary(), violations.clone());
    let line = assert_failure_line_fits("video-headroom-bench", verdict.outcome);
    assert_eq!(
        line["report"]["bench"]["passes"].as_array().map(Vec::len),
        Some(PASS_NAMES.len()),
        "проходы стенда целиком"
    );
    assert_eq!(line["report"]["bench"]["failures"], 9);
    assert!(
        observation
            .detail_items()
            .iter()
            .any(|item| item["bench_last_failure"].as_str() == Some(full_reason.as_str())),
        "полная причина отказа стенда — в подробностях"
    );
    assert_eq!(verdict.violation_details.len(), violations.len());
}

use render_loop::config::AdpfMode;

use super::*;
use crate::scenario::camera::raw_stream_samples::{LATENCY_MEANING, LatencyReport};
use crate::scenario::camera::viewfinder_metrics::{GpuTimeMs, PresentTargetDelta, VsyncShiftDelta};
use crate::scenario::camera::viewfinder_pacing_report::PacingWindowReport;
use crate::scenario::camera::viewfinder_tuning_report::{AdpfWindowDelta, AdpfWindowReport};

/// Числа одного окна профиля, остальное — типичные значения.
#[derive(Clone, Copy)]
struct Numbers {
    fps: f64,
    p50_ms: f64,
    /// Опоздавших на 1 vsync кадров из 1000.
    late: u64,
}

fn window(index: usize, numbers: Numbers) -> LatencyWindow {
    let profile = ProfileSlot::of_window(index);
    LatencyWindow {
        index,
        pair: index / 2,
        profile,
        measured_ms: 17_000,
        processed_fps: Some(numbers.fps),
        sensor_fps: Some(60.0),
        superseded: 30,
        superseded_per_s: Some(1.5),
        pacing: PacingWindowReport::default(),
        latency: LatencyReport {
            samples: 1_000,
            p50_ms: Some(numbers.p50_ms),
            p95_ms: Some(numbers.p50_ms + 5.0),
            max_ms: Some(numbers.p50_ms + 10.0),
            unmatched_presentations: 0,
            implausible: 0,
            dropped_over_limit: 0,
            clock_read_failures: 0,
            last_clock_error: None,
            meaning: LATENCY_MEANING,
        },
        present_target: PresentTargetDelta {
            frames_planned: 1_000,
            fallbacks_to_preferred: u64::from(profile == ProfileSlot::Tuned) * 2,
            latch_waits_refused: u64::from(profile == ProfileSlot::Tuned),
            vsync_shift: VsyncShiftDelta {
                on_time: 1_000 - numbers.late,
                late_1: numbers.late,
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
            delta: AdpfWindowDelta {
                reports_sent: if profile == ProfileSlot::Tuned { 1_000 } else { 0 },
                reports_with_gpu: 0,
                report_failures: 0,
            },
        },
        frame_gpu_time: Some(GpuTimeMs {
            samples: 600,
            p50_ms: 9.0,
            p95_ms: 11.0,
            max_ms: 12.0,
        }),
        selection_matched: true,
    }
}

/// Шесть окон (ABBA): база и проверяемый профиль с заданными числами.
fn six_windows(baseline: Numbers, tuned: Numbers) -> Vec<LatencyWindow> {
    (0..6)
        .map(|index| {
            window(
                index,
                match ProfileSlot::of_window(index) {
                    ProfileSlot::Baseline => baseline,
                    ProfileSlot::Tuned => tuned,
                },
            )
        })
        .collect()
}

const BASE: Numbers = Numbers {
    fps: 52.0,
    p50_ms: 48.0,
    late: 10,
};

fn verdict_of(tuned: Numbers) -> ProfileVerdict {
    compare_profiles(&ViewfinderLatencyConfig::default(), &six_windows(BASE, tuned)).verdict
}

#[test]
fn schedule_is_abba() {
    let order: Vec<ProfileSlot> = (0..6).map(ProfileSlot::of_window).collect();
    use ProfileSlot::{Baseline as A, Tuned as B};
    assert_eq!(order, [A, B, B, A, A, B]);
}

#[test]
fn tuned_is_better_when_faster_and_smoother_without_more_misses() {
    let tuned = Numbers {
        fps: 58.0,
        p50_ms: 40.0,
        late: 10,
    };
    let comparison = compare_profiles(&ViewfinderLatencyConfig::default(), &six_windows(BASE, tuned));

    assert_eq!(comparison.verdict.verdict, VerdictKind::TunedBetter);
    assert_eq!(
        comparison.verdict.better_on,
        [Criterion::ProcessedFps, Criterion::LatencyP50]
    );
    assert!(comparison.verdict.worse_on.is_empty());
    assert_eq!(comparison.fps_gain, Some(6.0));
    assert_eq!(comparison.latency_gain_ms, Some(8.0));
    assert_eq!(comparison.miss_ratio_change, Some(0.0));
    assert_eq!(comparison.baseline.processed_fps, Some(52.0));
    assert_eq!(comparison.tuned.processed_fps, Some(58.0));
    assert_eq!(comparison.baseline.windows, 3);
    assert_eq!(comparison.pair_p50_deltas_ms, [-8.0, -8.0, -8.0]);
    assert_eq!(comparison.pair_spread_ms, Some(0.0));
    // Суммы счётчиков идут по окнам своего профиля.
    assert_eq!(comparison.tuned.fallbacks_to_preferred, 6);
    assert_eq!(comparison.baseline.fallbacks_to_preferred, 0);
    assert_eq!(comparison.tuned.latch_waits_refused, 3);
    assert_eq!(comparison.tuned.adpf_reports_sent, 3_000);
    assert_eq!(comparison.baseline.adpf_reports_sent, 0);
    assert_eq!(comparison.tuned.frame_gpu_p95_ms, Some(11.0));
    assert_eq!(comparison.baseline.superseded_per_s, Some(1.5));
}

#[test]
fn tuned_is_worse_when_slower_or_late_more_often() {
    let slower = verdict_of(Numbers {
        fps: 52.0,
        p50_ms: 55.0,
        late: 10,
    });
    assert_eq!(slower.verdict, VerdictKind::TunedWorse);
    assert_eq!(slower.worse_on, [Criterion::LatencyP50]);

    // 1 % → 5 % опозданий при допуске 1 %.
    let late = verdict_of(Numbers {
        fps: 52.0,
        p50_ms: 48.0,
        late: 50,
    });
    assert_eq!(late.verdict, VerdictKind::TunedWorse);
    assert_eq!(late.worse_on, [Criterion::MissRatio]);

    let fewer_frames = verdict_of(Numbers {
        fps: 50.0,
        p50_ms: 48.0,
        late: 10,
    });
    assert_eq!(fewer_frames.verdict, VerdictKind::TunedWorse);
    assert_eq!(fewer_frames.worse_on, [Criterion::ProcessedFps]);
}

#[test]
fn differences_below_thresholds_are_no_clear_difference() {
    let verdict = verdict_of(Numbers {
        fps: 52.3,
        p50_ms: 47.5,
        late: 12,
    });

    assert_eq!(verdict.verdict, VerdictKind::NoClearDifference);
    assert!(
        verdict.better_on.is_empty() && verdict.worse_on.is_empty(),
        "{verdict:?}"
    );
}

#[test]
fn fewer_misses_count_as_better_and_mixed_results_are_named_mixed() {
    let fewer_misses = verdict_of(Numbers {
        fps: 52.0,
        p50_ms: 48.0,
        late: 0,
    });
    assert_eq!(fewer_misses.verdict, VerdictKind::TunedBetter);
    assert_eq!(fewer_misses.better_on, [Criterion::MissRatio]);

    // Быстрее по задержке, но чаще опаздывает: решает владелец.
    let mixed = verdict_of(Numbers {
        fps: 52.0,
        p50_ms: 40.0,
        late: 50,
    });
    assert_eq!(mixed.verdict, VerdictKind::Mixed);
    assert_eq!(mixed.better_on, [Criterion::LatencyP50]);
    assert_eq!(mixed.worse_on, [Criterion::MissRatio]);
}

#[test]
fn thresholds_come_from_the_config() {
    let strict = ViewfinderLatencyConfig {
        min_latency_gain_ms: 0.2,
        ..ViewfinderLatencyConfig::default()
    };
    let tuned = Numbers {
        fps: 52.0,
        p50_ms: 47.5,
        late: 10,
    };

    let verdict = compare_profiles(&strict, &six_windows(BASE, tuned)).verdict;

    assert_eq!(verdict.verdict, VerdictKind::TunedBetter);
    assert_eq!(verdict.better_on, [Criterion::LatencyP50]);
}

#[test]
fn nothing_to_compare_without_windows_of_both_profiles() {
    let config = ViewfinderLatencyConfig::default();
    assert_eq!(
        compare_profiles(&config, &[]).verdict.verdict,
        VerdictKind::NotComparable
    );
    let only_baseline = compare_profiles(&config, &[window(0, BASE)]);
    assert_eq!(only_baseline.verdict.verdict, VerdictKind::NotComparable);
    assert_eq!(only_baseline.tuned.windows, 0);
}

#[test]
fn summaries_use_the_median_over_windows_so_one_outlier_does_not_decide() {
    let mut windows = six_windows(BASE, BASE);
    // Один выброс проверяемого профиля: p50 = 200 мс.
    windows[1].latency.p50_ms = Some(200.0);

    let comparison = compare_profiles(&ViewfinderLatencyConfig::default(), &windows);

    assert_eq!(comparison.tuned.latency_p50_ms, Some(48.0));
    assert_eq!(comparison.verdict.verdict, VerdictKind::NoClearDifference);
}

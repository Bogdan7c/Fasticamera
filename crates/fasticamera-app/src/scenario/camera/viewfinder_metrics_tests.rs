use std::time::Duration;

use render_loop::thread::{DurationSummary, PresentTargetDiagnostics, VsyncShiftHistogram};
use render_loop::viewfinder::{ViewfinderColorSpace, ViewfinderCounters};

use super::*;

#[test]
fn gpu_time_is_reported_in_rounded_milliseconds() {
    let summary = DurationSummary {
        samples: 600,
        p50: Duration::from_micros(1_234),
        p95: Duration::from_micros(2_004),
        max: Duration::from_micros(4_999),
    };
    assert_eq!(
        GpuTimeMs::of(&summary),
        GpuTimeMs {
            samples: 600,
            p50_ms: 1.23,
            p95_ms: 2.0,
            max_ms: 5.0
        }
    );
    assert_eq!(GpuTimeMs::of_optional(None), None);
}

#[test]
fn median_handles_odd_even_empty_and_nan() {
    assert_eq!(median(&[3.0, 1.0, 2.0]), Some(2.0));
    assert_eq!(median(&[4.0, 1.0, 3.0, 2.0]), Some(2.5));
    assert_eq!(median(&[]), None);
    assert_eq!(median(&[1.0, f64::NAN]), None, "NaN — не число, медианы нет");
}

#[test]
fn color_space_is_named_honestly() {
    assert_eq!(color_space_name(Some(ViewfinderColorSpace::DisplayP3)), "display_p3");
    assert_eq!(color_space_name(Some(ViewfinderColorSpace::Srgb)), "srgb");
    assert_eq!(color_space_name(None), "unknown");
}

#[test]
fn isp_delta_subtracts_each_counter_and_never_goes_negative() {
    let before = ViewfinderCounters {
        frames_processed: 100,
        params_resolved: 10,
        cpu_copies: 0,
        statistics_published: 100,
        ..ViewfinderCounters::default()
    };
    let after = ViewfinderCounters {
        frames_processed: 160,
        params_resolved: 12,
        params_failed: 1,
        cpu_copies: 0,
        statistics_published: 159,
        ..ViewfinderCounters::default()
    };
    let delta = IspCountersDelta::between(&before, &after);
    assert_eq!(delta.frames_processed, 60);
    assert_eq!(delta.params_resolved, 2);
    assert_eq!(delta.params_failed, 1);
    assert_eq!(delta.statistics_published, 59);
    // Счётчики пересоздавшегося бэкенда меньше прежних — приращение 0, а не переполнение.
    assert_eq!(IspCountersDelta::between(&after, &before).frames_processed, 0);
}

#[test]
fn vsync_shift_delta_gives_miss_ratio_of_late_frames_only() {
    let before = VsyncShiftHistogram {
        late_by_periods: [10, 1, 0, 0],
        early: 0,
        unmatched: 3,
    };
    let after = VsyncShiftHistogram {
        late_by_periods: [100, 6, 3, 1],
        early: 2,
        unmatched: 9,
    };
    let delta = VsyncShiftDelta::between(&before, &after);
    assert_eq!(
        delta,
        VsyncShiftDelta {
            on_time: 90,
            late_1: 5,
            late_2: 3,
            late_3_plus: 1,
            early: 2
        }
    );
    assert_eq!(delta.total(), 99);
    let ratio = delta.miss_ratio().expect("кадры есть");
    assert!((ratio - 9.0 / 99.0).abs() < 1e-12);
    assert_eq!(VsyncShiftDelta::default().miss_ratio(), None);
}

#[test]
fn present_target_delta_counts_fallbacks_and_late_latches_within_the_window() {
    let before = PresentTargetDiagnostics {
        frames_planned: 100,
        fallbacks_to_preferred: 4,
        latch_already_passed: 1,
        ..PresentTargetDiagnostics::default()
    };
    let after = PresentTargetDiagnostics {
        frames_planned: 1300,
        fallbacks_to_preferred: 10,
        latch_already_passed: 1,
        chosen_earlier_than_preferred: 700,
        ..PresentTargetDiagnostics::default()
    };
    let delta = PresentTargetDelta::between(&before, &after);
    assert_eq!(
        (
            delta.frames_planned,
            delta.fallbacks_to_preferred,
            delta.latch_already_passed,
            delta.chosen_earlier_than_preferred
        ),
        (1200, 6, 0, 700)
    );
}

#[test]
fn isp_rate_is_frames_per_window_second() {
    let delta = WindowDelta {
        window_ms: 2_000,
        isp: IspCountersDelta {
            frames_processed: 120,
            ..IspCountersDelta::default()
        },
        ..WindowDelta::default()
    };
    assert_eq!(delta.isp_frames_per_second(), Some(60.0));
    assert_eq!(
        WindowDelta::default().isp_frames_per_second(),
        None,
        "пустое окно — нет частоты"
    );
}

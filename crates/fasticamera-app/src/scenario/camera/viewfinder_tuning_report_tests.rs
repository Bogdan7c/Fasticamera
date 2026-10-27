use render_loop::config::{DepthReductionMode, LateLatchMode, PresentTimelinePolicy};
use render_loop::thread::{DurationSummary, FrameCostEstimate, PresentTargetSelection};

use super::*;

fn adpf(mode: AdpfMode, session: AdpfSessionState) -> AdpfDiagnostics {
    AdpfDiagnostics {
        mode,
        session,
        reports_sent: 100,
        report_failures: 2,
        reports_with_gpu: 90,
        ..AdpfDiagnostics::default()
    }
}

#[test]
fn adpf_report_names_the_session_and_trims_a_long_error() {
    let mut diagnostics = adpf(AdpfMode::Enabled, AdpfSessionState::Active);
    diagnostics.last_error = Some("ж".repeat(500));

    let report = AdpfReport::of(&diagnostics);

    assert_eq!(report.session, "active");
    assert_eq!((report.reports_sent, report.report_failures), (100, 2));
    assert_eq!(
        report.last_error.as_deref().map(|error| error.chars().count()),
        Some(120)
    );
    assert_eq!(report.violation(), None);
}

#[test]
fn only_failed_session_with_enabled_mode_is_a_violation() {
    let failed = AdpfReport::of(&adpf(AdpfMode::Enabled, AdpfSessionState::Failed));
    assert!(failed.violation().is_some_and(|text| text.contains("failed")));
    // Unsupported — только отчёт; выключенный режим с Failed (остаток) — тоже не нарушение.
    assert_eq!(
        AdpfReport::of(&adpf(AdpfMode::Enabled, AdpfSessionState::Unsupported)).violation(),
        None
    );
    assert_eq!(
        AdpfReport::of(&adpf(AdpfMode::Disabled, AdpfSessionState::Failed)).violation(),
        None
    );
}

#[test]
fn window_delta_is_a_saturating_difference() {
    let before = adpf(AdpfMode::Enabled, AdpfSessionState::Active);
    let mut after = before.clone();
    after.reports_sent += 50;
    after.reports_with_gpu += 40;
    after.report_failures += 1;

    assert_eq!(
        AdpfWindowDelta::between(&before, &after),
        AdpfWindowDelta {
            reports_sent: 50,
            reports_with_gpu: 40,
            report_failures: 1
        }
    );
    // Счётчик «откатился» (поток пересоздан) — ноль, а не переполнение.
    assert_eq!(AdpfWindowDelta::between(&after, &before), AdpfWindowDelta::default());
}

#[test]
fn present_target_report_carries_selection_cost_parts_and_gpu_time_in_ms() {
    let diagnostics = PresentTargetDiagnostics {
        selection: PresentTargetSelection {
            policy: PresentTimelinePolicy::NearestFeasible { margin_us: 1_500 },
            late_latch: LateLatchMode::BeforeCameraPoll,
            depth_reduction: DepthReductionMode::SkipOneVsync,
        },
        estimated_cost_parts: FrameCostEstimate {
            cpu: Duration::from_micros(2_346),
            gpu: Duration::from_micros(13_000),
            gpu_source: GpuCostSource::ConfiguredFallback,
        },
        frame_gpu_time: Some(DurationSummary {
            samples: 600,
            p50: Duration::from_micros(9_000),
            p95: Duration::from_micros(11_500),
            max: Duration::from_micros(14_000),
        }),
        ..PresentTargetDiagnostics::default()
    };

    let json = serde_json::to_value(PresentTargetReport::of(&diagnostics)).expect("json");

    assert_eq!(
        json["policy"],
        serde_json::json!({"kind": "nearest_feasible", "margin_us": 1500})
    );
    assert_eq!(json["late_latch"], "before_camera_poll");
    assert_eq!(json["gpu_source"], "configured_fallback");
    assert_eq!(json["estimated_cpu_ms"], 2.35);
    assert_eq!(json["estimated_gpu_ms"], 13.0);
    assert_eq!(json["frame_gpu_time"]["p50_ms"], 9.0);
    assert_eq!(json["frame_gpu_time"]["p95_ms"], 11.5);
}

#[test]
fn present_target_report_without_gpu_timings_says_none() {
    let report = PresentTargetReport::of(&PresentTargetDiagnostics::default());

    assert_eq!(report.frame_gpu_time, None);
    assert_eq!(report.gpu_source, "configured_fallback");
}

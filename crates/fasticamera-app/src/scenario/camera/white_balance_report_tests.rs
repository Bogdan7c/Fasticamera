use render_loop::viewfinder::{EstimateReading, WhiteDelta, WhiteReading};

use super::*;

#[test]
fn idle_diagnostics_report_no_white_and_no_estimates() {
    let report = WhiteBalanceReport::of(&WhiteBalanceDiagnostics::idle(4));
    assert_eq!(report.estimates, 0);
    assert_eq!(report.our_white, None);
    assert_eq!(report.statistics_every_n_frames, 4);
    assert!(!report.cold_start_pending);
}

#[test]
fn working_awb_reports_white_confidence_tiles_hal_delta_and_hidden_frames() {
    let mut diagnostics = WhiteBalanceDiagnostics::idle(4);
    diagnostics.estimates = 9;
    diagnostics.scene_white = Some(WhiteReading {
        temperature_kelvin: 5_012.4,
        duv: 0.003_456_7,
    });
    diagnostics.last_estimate = Some(EstimateReading {
        white: WhiteReading {
            temperature_kelvin: 5_000.0,
            duv: 0.0,
        },
        confidence: 0.834,
        accepted_tiles: 37,
        clamped_to_band: false,
    });
    diagnostics.hal_delta = Some(WhiteDelta {
        mired: -12.345,
        duv: 0.001_23,
    });
    diagnostics.hidden_frames_cold_start = 3;
    diagnostics.cold_start_forced = 1;
    diagnostics.cold_start = ColdStartState::Pending { hidden: 2 };
    let report = WhiteBalanceReport::of(&diagnostics);
    assert_eq!(
        report.our_white,
        Some(WhitePointReport {
            kelvin: 5_012.0,
            duv: 0.0035
        })
    );
    assert_eq!(report.confidence, Some(0.83));
    assert_eq!(report.accepted_tiles, Some(37));
    assert_eq!(report.hal_delta_mired, Some(-12.35));
    assert_eq!(report.hal_delta_duv, Some(0.0012));
    assert!(report.cold_start_pending);
    assert_eq!((report.hidden_frames, report.cold_start_forced), (3, 1));
}

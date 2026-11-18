//! Наблюдение за нашим AE: что считается вендорским запросом и как копятся признаки.

use camera_contract::capture_result::FrameNumber;
use camera_contract::three_a::{AeState, RequestOrigin};
use camera_core::three_a_status::{
    AdjustmentReason, AutoExposureProgress, ControlAdjustment, ControlParameter, LastThreeAResult, ThreeAStatus,
};

use super::*;

fn result(frame: i64, ae_state: Option<AeState>, origin: RequestOrigin) -> LastThreeAResult {
    LastThreeAResult {
        frame_number: FrameNumber(frame),
        sensor_timestamp: None,
        origin,
        af_state: None,
        ae_state,
        focus_distance_diopters: None,
        exposure_time: None,
        sensitivity_iso: None,
    }
}

fn status_with(progress: Option<AutoExposureProgress>) -> ThreeAStatus {
    ThreeAStatus {
        auto_exposure: progress,
        ..ThreeAStatus::initial()
    }
}

#[test]
fn source_follows_the_engine_status_not_a_config() {
    assert_eq!(AeSource::of(&status_with(None)), AeSource::Vendor);
    assert_eq!(
        AeSource::of(&status_with(Some(AutoExposureProgress::Searching))),
        AeSource::Own
    );
}

#[test]
fn watch_of_a_vendor_run_reports_nothing() {
    let mut watch = OwnAeWatch::default();
    watch.observe(&status_with(None));
    assert_eq!(watch.report(&ResultLog::default()), None);
}

#[test]
fn watch_collects_markers_limit_shortfall_and_counts_vendor_ae_results() {
    let mut watch = OwnAeWatch::default();
    watch.observe(&status_with(Some(AutoExposureProgress::Searching)));
    watch.observe(&status_with(Some(AutoExposureProgress::Settled)));
    let mut limited = status_with(Some(AutoExposureProgress::LimitReached { shortfall_ev: -1.5 }));
    limited.adjustments.push(ControlAdjustment {
        parameter: ControlParameter::ExposureTarget,
        requested: 3.0,
        applied: 1.5,
        reason: AdjustmentReason::TargetNotReached,
    });
    watch.observe(&limited);

    let mut results = ResultLog::default();
    for (frame, state) in [(1, Some(AeState::Inactive)), (2, Some(AeState::Converged)), (3, None)] {
        results.observe(Some(&result(frame, state, RequestOrigin::Repeating)));
    }
    // Разовый кадр с активным AE_STATE в журнал повторяющегося запроса не попадает.
    results.observe(Some(&result(
        4,
        Some(AeState::Searching),
        RequestOrigin::Burst {
            burst: camera_contract::three_a::BurstId(1),
            index: 0,
        },
    )));

    let report = watch.report(&results).expect("наш AE вёл экспозицию");
    assert!(report.settled_seen && report.limit_reached_seen && !report.holding_seen);
    assert_eq!(report.worst_shortfall_ev, Some(-1.5));
    assert_eq!(report.target_not_reached_ticks, 1);
    assert_eq!(report.repeating_results_checked, 3);
    assert_eq!(report.vendor_ae_results, 1);
    assert_eq!(report.first_vendor_ae_frame, Some(2));
    assert!(
        own_ae_violations(&report)
            .iter()
            .any(|text| text.contains("вендорским AE"))
    );
}

#[test]
fn convergence_markers_distinguish_settled_holding_limit_and_the_rest() {
    assert!(is_converged(Some(AutoExposureProgress::Settled)));
    assert!(is_converged(Some(AutoExposureProgress::Holding)));
    assert!(!is_converged(Some(AutoExposureProgress::Searching)));
    assert!(!is_converged(None));
    assert!(is_limit_reached(Some(AutoExposureProgress::LimitReached {
        shortfall_ev: 1.0
    })));
    assert!(!is_limit_reached(Some(AutoExposureProgress::Settled)));
    assert!(!is_vendor_ae_active(AeState::Inactive));
    assert!(is_vendor_ae_active(AeState::Locked));
}

#[test]
fn lock_counts_only_with_the_engine_lock_and_holding_together() {
    use camera_contract::three_a::AeAfLock;

    let with = |lock: AeAfLock, progress: Option<AutoExposureProgress>| ThreeAStatus {
        lock,
        auto_exposure: progress,
        ..ThreeAStatus::initial()
    };
    assert!(is_lock_reached(&with(
        AeAfLock::Locked,
        Some(AutoExposureProgress::Holding)
    )));
    // Устаревший статус до команды: Holding остался от прошлого, блокировки движок ещё не принял.
    assert!(!is_lock_reached(&with(
        AeAfLock::Unlocked,
        Some(AutoExposureProgress::Holding)
    )));
    // Предел без Holding блокировкой не считается.
    assert!(!is_lock_reached(&with(
        AeAfLock::Locked,
        Some(AutoExposureProgress::LimitReached { shortfall_ev: -1.0 })
    )));
    assert!(!is_lock_reached(&with(
        AeAfLock::Locked,
        Some(AutoExposureProgress::Settled)
    )));
    assert!(!is_lock_reached(&with(AeAfLock::Locked, None)));
}

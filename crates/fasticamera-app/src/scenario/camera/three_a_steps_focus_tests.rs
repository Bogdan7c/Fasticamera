//! Итог тапа сценария `3a` по событиям тика: итог HAL, снятие точки движком (решение 241) и ожидание.

use camera_core::CameraEvent;
use camera_core::point_hold::PointReleaseCause;
use camera_core::three_a_status::FocusOutcome;

use super::*;
use crate::scenario::camera::three_a_report::{FocusReport, TapReport};
use crate::scenario::camera::three_a_verdict::focus_violations;

#[test]
fn a_tap_ends_with_the_hal_outcome_of_its_own_request_only() {
    let events = [
        CameraEvent::FocusSettled {
            request: PointRequestId(6),
            outcome: FocusOutcome::NotFocused,
        },
        CameraEvent::FocusSettled {
            request: PointRequestId(7),
            outcome: FocusOutcome::Focused,
        },
    ];
    assert_eq!(
        tap_result_from_events(PointRequestId(7), &events),
        Some(TapResult::Focused)
    );
    assert_eq!(
        tap_result_from_events(PointRequestId(8), &events),
        None,
        "чужие итоги не считаются"
    );
}

#[test]
fn an_engine_release_before_the_outcome_ends_the_tap_instead_of_leaving_it_to_time_out() {
    let events = [CameraEvent::PointOfInterestReleased {
        request: PointRequestId(7),
        cause: PointReleaseCause::SceneChanged,
    }];
    assert_eq!(
        tap_result_from_events(PointRequestId(7), &events),
        Some(TapResult::ReleasedBySceneChange)
    );
    assert_eq!(
        tap_result_from_events(PointRequestId(9), &events),
        None,
        "снятие чужого тапа этот тап не заканчивает"
    );
}

fn focus_report_with(results: &[TapResult]) -> FocusReport {
    FocusReport {
        taps: results
            .iter()
            .map(|result| TapReport {
                x: 0.5,
                y: 0.5,
                result: *result,
                frames_to_settle: None,
                diopters_before: None,
                diopters_after: None,
            })
            .collect(),
        region_effect_diopters: None,
        triggers_sent: results.len() as u64,
        results_without_request_context: 0,
        results_with_unknown_origin: 0,
    }
}

#[test]
fn a_released_center_tap_is_a_violation_with_a_hint_and_a_released_corner_tap_is_only_a_note() {
    let center = focus_violations(&focus_report_with(&[
        TapResult::ReleasedBySceneChange,
        TapResult::Focused,
    ]));
    assert_eq!(center.len(), 1, "{center:?}");
    assert!(center[0].contains("отпустил точку"), "{center:?}");

    let corner = focus_violations(&focus_report_with(&[
        TapResult::Focused,
        TapResult::ReleasedBySceneChange,
    ]));
    assert!(corner.is_empty(), "{corner:?}");
}

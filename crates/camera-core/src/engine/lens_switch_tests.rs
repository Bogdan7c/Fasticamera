//! Машина смены объектива без движка: переходы, события, счётчики, блокировка.

use std::time::{Duration, Instant};

use camera_contract::availability::CameraUnavailableReason;
use camera_contract::ids::LensRole;

use super::*;
use crate::backend::{BackendFailure, ConfigureFailure};

fn steady(role: LensRole, now: Instant) -> LensSwitchTracker {
    let mut tracker = LensSwitchTracker::new();
    assert_eq!(tracker.on_frame(role, now), None, "первый показ — не смена");
    tracker
}

fn device_failure() -> LensSwitchFailure {
    LensSwitchFailure::Unavailable {
        reason: CameraUnavailableReason::DeviceFailure,
    }
}

#[test]
fn first_frame_is_a_first_show_not_a_switch() {
    let now = Instant::now();
    let mut tracker = LensSwitchTracker::new();
    assert_eq!(
        tracker.on_target_changed(LensRole::Wide, RetargetCause::Command, now),
        None
    );
    assert_eq!(tracker.state(), LensSwitchState::Idle, "камеры ещё нет — смены нет");
    assert_eq!(tracker.on_frame(LensRole::Wide, now), None);
    assert_eq!(tracker.state(), LensSwitchState::Steady { role: LensRole::Wide });
    assert_eq!(tracker.counters(), LensCounters::default());
}

#[test]
fn switch_completes_on_first_frame_of_the_new_camera_with_elapsed() {
    let start = Instant::now();
    let mut tracker = steady(LensRole::Wide, start);
    tracker.on_target_changed(LensRole::UltraWide, RetargetCause::Intent, start);
    assert_eq!(
        tracker.state(),
        LensSwitchState::Switching {
            from: LensRole::Wide,
            to: LensRole::UltraWide
        }
    );
    // Кадр прежней камеры смену не завершает.
    assert_eq!(tracker.on_frame(LensRole::Wide, start), None);
    let event = tracker.on_frame(LensRole::UltraWide, start + Duration::from_millis(480));
    assert_eq!(
        event,
        Some(CameraEvent::LensSwitched {
            from: LensRole::Wide,
            to: LensRole::UltraWide,
            elapsed: Duration::from_millis(480),
            method: crate::lens_status::LensSwitchMethod::Reopened,
        })
    );
    assert_eq!(
        tracker.state(),
        LensSwitchState::Steady {
            role: LensRole::UltraWide
        }
    );
    assert_eq!(tracker.counters().switches_completed, 1);
    assert_eq!(tracker.counters().last_switch_elapsed, Some(Duration::from_millis(480)));
}

#[test]
fn retarget_keeps_from_reports_the_abandoned_target_and_keeps_the_start_time() {
    let start = Instant::now();
    let mut tracker = steady(LensRole::Wide, start);
    tracker.on_target_changed(LensRole::UltraWide, RetargetCause::Intent, start);
    let event = tracker.on_target_changed(
        LensRole::Front,
        RetargetCause::Command,
        start + Duration::from_millis(100),
    );
    assert_eq!(
        event,
        Some(CameraEvent::LensSwitchRetargeted {
            from: LensRole::Wide,
            abandoned: LensRole::UltraWide,
            to: LensRole::Front
        })
    );
    assert_eq!(
        tracker.state(),
        LensSwitchState::Switching {
            from: LensRole::Wide,
            to: LensRole::Front
        }
    );
    // Та же цель повторно — не пересмотр.
    assert_eq!(
        tracker.on_target_changed(LensRole::Front, RetargetCause::Intent, start),
        None
    );
    let done = tracker.on_frame(LensRole::Front, start + Duration::from_millis(700));
    assert!(matches!(done, Some(CameraEvent::LensSwitched { elapsed, .. }) if elapsed == Duration::from_millis(700)));
    assert_eq!(tracker.counters().switches_retargeted, 1);
}

#[test]
fn retarget_back_to_origin_is_a_switch_back_that_completes_on_the_origin_frame() {
    let start = Instant::now();
    let mut tracker = steady(LensRole::Wide, start);
    tracker.on_target_changed(LensRole::UltraWide, RetargetCause::Intent, start);
    tracker.on_target_changed(LensRole::Wide, RetargetCause::Intent, start);
    assert_eq!(
        tracker.state(),
        LensSwitchState::Switching {
            from: LensRole::Wide,
            to: LensRole::Wide
        }
    );
    let event = tracker.on_frame(LensRole::Wide, start + Duration::from_millis(50));
    assert!(matches!(
        event,
        Some(CameraEvent::LensSwitched {
            from: LensRole::Wide,
            to: LensRole::Wide,
            ..
        })
    ));
    assert_eq!(
        tracker.counters().switches_completed,
        0,
        "возврат на прежний — не новая смена"
    );
}

#[test]
fn failure_returns_to_from_blocks_the_failed_role_and_counts() {
    let start = Instant::now();
    let mut tracker = steady(LensRole::Wide, start);
    tracker.on_target_changed(LensRole::UltraWide, RetargetCause::Intent, start);
    let revert = tracker.revert_candidate().expect("идёт смена на другой объектив");
    assert_eq!(
        revert,
        Revert {
            from: LensRole::Wide,
            to: LensRole::UltraWide
        }
    );
    let event = tracker.on_switch_failed(revert, device_failure());
    assert_eq!(
        event,
        CameraEvent::LensSwitchFailed {
            from: LensRole::Wide,
            to: LensRole::UltraWide,
            reason: device_failure()
        }
    );
    assert_eq!(tracker.blocked().map(|block| block.role), Some(LensRole::UltraWide));
    assert_eq!(tracker.counters().switches_failed, 1);
    assert_eq!(
        tracker.revert_candidate(),
        None,
        "провал самого возврата — обычное восстановление"
    );
}

#[test]
fn command_to_the_blocked_role_unblocks_but_intent_does_not() {
    let start = Instant::now();
    let mut tracker = steady(LensRole::Wide, start);
    tracker.on_target_changed(LensRole::UltraWide, RetargetCause::Intent, start);
    let revert = tracker.revert_candidate().expect("смена");
    tracker.on_switch_failed(
        revert,
        LensSwitchFailure::StreamRejected {
            failure: ConfigureFailure::Backend(BackendFailure {
                operation: "x",
                code: 1,
                detail: String::new(),
            }),
        },
    );
    tracker.on_target_changed(LensRole::UltraWide, RetargetCause::Intent, start);
    assert!(tracker.blocked().is_some(), "намерение блокировку не снимает");
    tracker.on_target_changed(LensRole::UltraWide, RetargetCause::Command, start);
    assert!(tracker.blocked().is_none(), "явная команда снимает");
}

#[test]
fn only_faults_of_the_new_camera_justify_a_revert() {
    for reason in [
        CameraUnavailableReason::DeviceFailure,
        CameraUnavailableReason::ServiceFailure,
        CameraUnavailableReason::CameraNotPresent,
    ] {
        assert!(is_fault_of_new_camera(reason), "{reason:?}");
    }
    for reason in [
        CameraUnavailableReason::Evicted,
        CameraUnavailableReason::InUseByAnotherClient,
        CameraUnavailableReason::TooManyCamerasOpen,
        CameraUnavailableReason::PermissionDenied,
        CameraUnavailableReason::DisabledByPolicy,
    ] {
        assert!(!is_fault_of_new_camera(reason), "{reason:?}");
    }
}

#[test]
fn reset_forgets_the_switch_and_the_block_but_keeps_counters() {
    let start = Instant::now();
    let mut tracker = steady(LensRole::Wide, start);
    tracker.on_target_changed(LensRole::UltraWide, RetargetCause::Intent, start);
    let revert = tracker.revert_candidate().expect("смена");
    tracker.on_switch_failed(revert, device_failure());
    tracker.reset();
    assert_eq!(tracker.state(), LensSwitchState::Idle);
    assert!(tracker.blocked().is_none());
    assert_eq!(tracker.counters().switches_failed, 1);
}

#[test]
fn stream_start_in_idle_makes_the_lens_shown_so_a_retarget_is_a_switch() {
    let now = Instant::now();
    let mut tracker = LensSwitchTracker::new();
    tracker.on_stream_started(LensRole::Wide);
    assert_eq!(tracker.state(), LensSwitchState::Steady { role: LensRole::Wide });
    assert_eq!(tracker.counters(), LensCounters::default(), "первый показ — не смена");

    tracker.on_target_changed(LensRole::UltraWide, RetargetCause::Intent, now);
    assert_eq!(
        tracker.state(),
        LensSwitchState::Switching {
            from: LensRole::Wide,
            to: LensRole::UltraWide
        }
    );
    let event = tracker.on_frame(LensRole::UltraWide, now + Duration::from_millis(200));
    assert!(matches!(event, Some(CameraEvent::LensSwitched { .. })));
    assert_eq!(tracker.counters().switches_completed, 1);
}

#[test]
fn retarget_before_any_stream_start_is_not_a_switch() {
    let now = Instant::now();
    let mut tracker = LensSwitchTracker::new();
    tracker.on_target_changed(LensRole::Wide, RetargetCause::Command, now);
    tracker.on_target_changed(LensRole::UltraWide, RetargetCause::Intent, now);
    assert_eq!(tracker.state(), LensSwitchState::Idle);
    tracker.on_stream_started(LensRole::UltraWide);
    assert_eq!(
        tracker.state(),
        LensSwitchState::Steady {
            role: LensRole::UltraWide
        }
    );
    assert_eq!(tracker.counters(), LensCounters::default());
}

#[test]
fn stream_start_during_a_switch_does_not_finish_it_the_first_frame_does() {
    let start = Instant::now();
    let mut tracker = steady(LensRole::Wide, start);
    tracker.on_target_changed(LensRole::UltraWide, RetargetCause::Intent, start);
    let switching = LensSwitchState::Switching {
        from: LensRole::Wide,
        to: LensRole::UltraWide,
    };
    tracker.on_stream_started(LensRole::UltraWide);
    assert_eq!(tracker.state(), switching, "старт потока смену не завершает");
    assert_eq!(tracker.counters().switches_completed, 0);
    assert!(
        tracker
            .on_frame(LensRole::UltraWide, start + Duration::from_millis(90))
            .is_some()
    );
}

#[test]
fn stream_start_in_steady_changes_nothing_for_the_same_or_another_role() {
    let now = Instant::now();
    let mut tracker = steady(LensRole::Wide, now);
    tracker.on_stream_started(LensRole::Wide);
    assert_eq!(tracker.state(), LensSwitchState::Steady { role: LensRole::Wide });
    // Другая роль без смены цели не бывает; если случится — решает кадр, а не старт потока.
    tracker.on_stream_started(LensRole::UltraWide);
    assert_eq!(tracker.state(), LensSwitchState::Steady { role: LensRole::Wide });
    assert_eq!(tracker.counters(), LensCounters::default());
}

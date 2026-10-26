//! Границы восстановления: лимит попыток при «сессия закрывается сразу», обнуление счётчика
//! только после устойчивой работы, повтор по таймеру при общем лимите камер и гонка сигнала
//! «камера свободна» с отключением устройства.

use std::time::Duration;

use camera_contract::availability::CameraUnavailableReason;
use camera_contract::fixtures::camera_id;

use super::*;
use crate::backend::{CameraAvailability, DeviceErrorKind, OpenFailure, SessionGeneration};
use crate::fake::BackendCall;
use crate::state::RecoveryPlan;

const FRAME_INTERVAL: Duration = Duration::from_micros(16_667);

fn retry_plan(attempt: u32, delay_ms: u64) -> CameraState {
    CameraState::Error {
        error: CameraError::Recoverable {
            reason: CameraUnavailableReason::DeviceFailure,
            recovery: RecoveryPlan::RetryAfterDelay { attempt, delay_ms },
        },
    }
}

fn recovered_attempts(events: &[CameraEvent]) -> Vec<u32> {
    events
        .iter()
        .filter_map(|event| match event {
            CameraEvent::Recovered { attempts, .. } => Some(*attempts),
            _ => None,
        })
        .collect()
}

#[test]
fn session_closed_right_after_every_setup_ends_in_recovery_exhausted() {
    let mut rig = Rig::new();
    rig.start_streaming_on_wide();
    let delays = [200_u64, 500, 1_000, 2_000, 2_000];

    // Платформа закрывает каждую свежую сессию, не успев отдать ни одного кадра.
    for delay_ms in delays {
        rig.fake.close_current_session_from_platform();
        rig.pump();
        assert_eq!(rig.state_name(), "error");
        rig.advance(Duration::from_millis(delay_ms));
        assert_eq!(rig.state_name(), "streaming", "настройка каждый раз удаётся");
    }
    rig.fake.close_current_session_from_platform();
    rig.pump();

    assert_eq!(
        rig.state(),
        CameraState::Error {
            error: CameraError::Fatal {
                cause: FatalCause::RecoveryExhausted {
                    reason: CameraUnavailableReason::DeviceFailure,
                    attempts: 5
                }
            }
        }
    );
    assert_eq!(rig.engine.status().counters.recovery_attempts, 5);
    assert_eq!(rig.engine.next_deadline(), None);
}

#[test]
fn counter_resets_only_after_the_configured_number_of_stable_frames() {
    let mut rig = Rig::new();
    rig.start_streaming_on_wide();
    let required = rig.engine.config.recovery.stable_frames_before_reset;
    assert_eq!(required, 30);

    rig.fake.fail_current_device(DeviceErrorKind::CameraDevice);
    rig.pump();
    rig.advance(Duration::from_millis(200));
    assert_eq!(recovered_attempts(&rig.fresh_events()), vec![1]);

    // Не хватило одного кадра: счётчик ещё держит попытку 1, следующий отказ — попытка 2.
    rig.fake.complete_frames(required - 1, FRAME_INTERVAL);
    rig.pump();
    rig.fake.fail_current_device(DeviceErrorKind::CameraDevice);
    rig.pump();
    assert_eq!(rig.state(), retry_plan(2, 500), "счётчик не обнулён раньше времени");
    rig.advance(Duration::from_millis(500));
    assert_eq!(recovered_attempts(&rig.fresh_events()), vec![2]);

    // Теперь набрали ровно порог: следующий отказ снова начинает с попытки 1.
    rig.fake.complete_frames(required, FRAME_INTERVAL);
    rig.pump();
    rig.fake.fail_current_device(DeviceErrorKind::CameraDevice);
    rig.pump();
    assert_eq!(rig.state(), retry_plan(1, 200));
}

#[test]
fn frames_from_a_closed_session_do_not_count_as_stability() {
    let mut rig = Rig::new();
    rig.start_streaming_on_wide();
    rig.fake.fail_current_device(DeviceErrorKind::CameraDevice);
    rig.pump();
    rig.advance(Duration::from_millis(200));
    let required = rig.engine.config.recovery.stable_frames_before_reset;
    // Кадры чужой (несуществующей) сессии: движок их отбрасывает как привидения.
    for _ in 0..required {
        rig.feed(BackendEvent::CaptureCompleted {
            session: SessionGeneration(99),
            result: rig_result(),
        });
    }
    rig.fake.fail_current_device(DeviceErrorKind::CameraDevice);
    rig.pump();
    assert_eq!(rig.state(), retry_plan(2, 500));
}

fn rig_result() -> camera_contract::capture_result::CaptureResultMeta {
    use camera_contract::capture_result::{CaptureResultMeta, FrameNumber, SensorTimestamp};
    CaptureResultMeta {
        frame_number: FrameNumber(1),
        sensor_timestamp: Some(SensorTimestamp::from_nanos(1_000)),
        exposure_time: None,
        sensitivity_iso: None,
        reported_frame_duration: None,
        rolling_shutter_skew: None,
    }
}

#[test]
fn recovery_without_a_wanted_stream_resets_the_counter_immediately() {
    let mut rig = Rig::new();
    rig.fake.push_open_result(Err(OpenFailure::Unavailable {
        reason: CameraUnavailableReason::DeviceFailure,
    }));
    rig.send(CameraCommand::Open { target: wide() });
    rig.advance(Duration::from_millis(200));
    assert_eq!(rig.state_name(), "opened");
    assert_eq!(recovered_attempts(&rig.fresh_events()), vec![1]);

    // Ждать кадров нечего (потока нет): следующий отказ — снова попытка 1.
    rig.fake.fail_current_device(DeviceErrorKind::CameraDevice);
    rig.pump();
    assert_eq!(rig.state(), retry_plan(1, 200));
}

#[test]
fn too_many_cameras_open_is_retried_by_timer_without_any_availability_signal() {
    let mut rig = Rig::new();
    rig.fake.push_open_result(Err(OpenFailure::Unavailable {
        reason: CameraUnavailableReason::TooManyCamerasOpen,
    }));
    rig.send(CameraCommand::Open { target: wide() });
    assert_eq!(rig.state_name(), "error");
    assert_eq!(rig.engine.next_deadline(), Some(rig.now + Duration::from_millis(200)));
    rig.advance(Duration::from_millis(200));
    assert_eq!(rig.state_name(), "opened", "лимит освободился сам, сигнала не было");
}

#[test]
fn availability_that_arrives_before_the_disconnect_still_triggers_the_reopen() {
    let mut rig = Rig::new();
    rig.start_streaming_on_wide();
    // Поток availability-обработчика обогнал looper устройства.
    rig.feed(BackendEvent::CameraAvailability {
        camera: camera_id("2"),
        availability: CameraAvailability::Available,
    });
    assert_eq!(rig.state_name(), "streaming", "пока отключения нет, ничего не меняется");
    rig.fake.disconnect_current_device();
    rig.pump();

    assert_eq!(rig.state_name(), "streaming", "ожидание не стало вечным");
    assert_eq!(
        recovered_attempts(&rig.fresh_events()),
        vec![1],
        "восстановление прошло сразу"
    );
    assert_eq!(rig.engine.status().counters.devices_opened, 2);
}

#[test]
fn availability_from_before_our_own_open_does_not_count_as_a_fresh_release() {
    let mut rig = Rig::new();
    // Камера была свободна ещё до того, как мы её открыли.
    rig.feed(BackendEvent::CameraAvailability {
        camera: camera_id("2"),
        availability: CameraAvailability::Available,
    });
    rig.start_streaming_on_wide();
    rig.fake.disconnect_current_device();
    rig.pump();

    assert_eq!(
        rig.state(),
        CameraState::Error {
            error: CameraError::Recoverable {
                reason: CameraUnavailableReason::Evicted,
                recovery: RecoveryPlan::WaitForCameraAvailable
            }
        }
    );
    assert!(
        rig.fake
            .take_calls()
            .iter()
            .all(|call| !matches!(call, BackendCall::OpenDevice { .. }))
    );
    assert_eq!(rig.engine.status().counters.recovery_attempts, 0);
}

//! Восстановление после вытеснения и сбоев (решение 39).

use std::time::Duration;

use camera_contract::availability::CameraUnavailableReason;
use camera_contract::fixtures::camera_id;

use super::*;
use crate::backend::{
    CameraAvailability, ConfigureFailure, DeviceErrorKind, DeviceGeneration, OpenFailure, RequestFailure,
    SessionGeneration, UnsupportedStream,
};
use crate::state::RecoveryPlan;

fn recoverable(reason: CameraUnavailableReason, recovery: RecoveryPlan) -> CameraState {
    CameraState::Error {
        error: CameraError::Recoverable { reason, recovery },
    }
}

fn recovered_events(events: &[CameraEvent]) -> Vec<(CameraUnavailableReason, u32)> {
    events
        .iter()
        .filter_map(|event| match event {
            CameraEvent::Recovered { reason, attempts } => Some((*reason, *attempts)),
            _ => None,
        })
        .collect()
}

#[test]
fn eviction_during_configuring_closes_waits_then_reopens_and_recovers() {
    let mut rig = Rig::new();
    rig.send(CameraCommand::Open { target: wide() });
    rig.fake.push_disconnect_during_configure();
    rig.fresh_events();
    rig.fake.take_calls();

    // Вытеснение посреди `Configuring`: и событие, и Err из вызова.
    rig.send(CameraCommand::Configure { mode: mode_raw10_60() });
    let events = rig.fresh_events();
    assert_eq!(state_names(&events), vec!["configuring", "error"]);
    assert!(events.contains(&CameraEvent::Unavailable {
        reason: CameraUnavailableReason::Evicted
    }));
    assert_eq!(
        rig.state(),
        recoverable(CameraUnavailableReason::Evicted, RecoveryPlan::WaitForCameraAvailable)
    );
    assert!(!rig.fake.is_device_open(), "устройство закрыто");
    assert_eq!(
        call_names(&rig.fake.take_calls()),
        vec!["configure_session", "close_device"]
    );
    // Запоздалое событие того же отключения пришло после закрытия — оно stale, а не вторая ошибка.
    assert_eq!(rig.engine.status().counters.stale_backend_events, 1);
    assert_eq!(
        rig.engine.status().last_unavailable,
        Some(CameraUnavailableReason::Evicted)
    );

    // Без сигнала камера не открывается, даже если время идёт.
    rig.advance(Duration::from_secs(60));
    assert!(rig.fake.take_calls().is_empty());

    // Хозяин камеры ушёл: система говорит «свободна» → переоткрытие с желаемым режимом.
    rig.send(CameraCommand::StartStreaming {
        intent: CaptureIntent::Preview,
    });
    assert!(
        rig.fake.take_calls().is_empty(),
        "команды в ошибке только меняют желаемое"
    );
    rig.fresh_events();
    rig.feed(BackendEvent::CameraAvailability {
        camera: camera_id("2"),
        availability: CameraAvailability::Available,
    });
    let events = rig.fresh_events();
    assert_eq!(
        state_names(&events),
        vec!["opening", "opened", "configuring", "streaming"]
    );
    assert_eq!(recovered_events(&events), vec![(CameraUnavailableReason::Evicted, 1)]);
    assert_eq!(
        call_names(&rig.fake.take_calls()),
        vec!["open_device", "configure_session", "set_repeating_request"]
    );
    let counters = rig.engine.status().counters;
    assert_eq!((counters.recovery_attempts, counters.recoveries), (1, 1));
    assert_eq!(counters.devices_opened, 2);
}

#[test]
fn availability_of_a_different_camera_or_unavailable_signal_does_not_restart_recovery() {
    let mut rig = Rig::new();
    rig.start_streaming_on_wide();
    rig.fake.disconnect_current_device();
    rig.pump();
    assert_eq!(rig.state_name(), "error");
    rig.fake.take_calls();
    rig.feed(BackendEvent::CameraAvailability {
        camera: camera_id("3"),
        availability: CameraAvailability::Available,
    });
    rig.feed(BackendEvent::CameraAvailability {
        camera: camera_id("2"),
        availability: CameraAvailability::Unavailable,
    });
    assert!(rig.fake.take_calls().is_empty());
    assert_eq!(rig.state_name(), "error");
    assert_eq!(rig.engine.status().counters.recovery_attempts, 0);
}

#[test]
fn camera_service_error_while_streaming_retries_on_timer_not_earlier() {
    let mut rig = Rig::new();
    rig.start_streaming_on_wide();
    rig.fake.fail_current_device(DeviceErrorKind::CameraService);
    rig.pump();
    assert_eq!(
        rig.state(),
        recoverable(
            CameraUnavailableReason::ServiceFailure,
            RecoveryPlan::RetryAfterDelay {
                attempt: 1,
                delay_ms: 200
            }
        )
    );
    assert_eq!(rig.engine.next_deadline(), Some(rig.now + Duration::from_millis(200)));
    assert_eq!(call_names(&rig.fake.take_calls()), vec!["close_device"]);

    rig.advance(Duration::from_millis(199));
    assert!(rig.fake.take_calls().is_empty(), "таймер раньше срока не срабатывает");
    assert_eq!(rig.state_name(), "error");

    rig.advance(Duration::from_millis(1));
    assert_eq!(rig.state_name(), "streaming");
    assert_eq!(
        call_names(&rig.fake.take_calls()),
        vec!["open_device", "configure_session", "set_repeating_request"]
    );
    assert_eq!(
        recovered_events(&rig.fresh_events()),
        vec![(CameraUnavailableReason::ServiceFailure, 1)]
    );
    assert_eq!(rig.engine.next_deadline(), None);
}

#[test]
fn repeated_failures_use_growing_backoff_then_give_up_with_recovery_exhausted() {
    let mut rig = Rig::new();
    rig.start_streaming_on_wide();
    let max = rig.engine.config.recovery.max_consecutive_attempts;
    assert_eq!(max, 5);
    rig.fake.fail_current_device(DeviceErrorKind::CameraService);
    rig.pump();

    let expected_delays = [200_u64, 500, 1_000, 2_000, 2_000];
    for (index, delay_ms) in expected_delays.iter().enumerate() {
        let attempt = u32::try_from(index).expect("мало") + 1;
        assert_eq!(
            rig.state(),
            recoverable(
                CameraUnavailableReason::ServiceFailure,
                RecoveryPlan::RetryAfterDelay {
                    attempt,
                    delay_ms: *delay_ms
                }
            ),
            "перед попыткой {attempt}"
        );
        // Каждая попытка упирается в тот же отказ открытия.
        rig.fake.push_open_result(Err(OpenFailure::Unavailable {
            reason: CameraUnavailableReason::ServiceFailure,
        }));
        rig.advance(Duration::from_millis(*delay_ms));
    }
    assert_eq!(
        rig.state(),
        CameraState::Error {
            error: CameraError::Fatal {
                cause: FatalCause::RecoveryExhausted {
                    reason: CameraUnavailableReason::ServiceFailure,
                    attempts: 5
                }
            }
        }
    );
    assert_eq!(rig.engine.status().counters.recovery_attempts, 5);
    assert_eq!(rig.engine.next_deadline(), None, "после отказа таймеров нет");
    assert_eq!(
        rig.send(CameraCommand::Open { target: wide() }),
        CommandOutcome::InvalidState(crate::command::InvalidStateReason::EngineFailed)
    );
}

#[test]
fn foreground_gives_a_fresh_set_of_attempts_after_recovery_exhausted() {
    let mut config = CameraEngineConfig::default();
    config.recovery.max_consecutive_attempts = 1;
    let mut rig = Rig::with_config(config, AppPresence::Foreground);
    rig.start_streaming_on_wide();
    rig.fake.fail_current_device(DeviceErrorKind::CameraDevice);
    rig.pump();
    rig.fake.push_open_result(Err(OpenFailure::Unavailable {
        reason: CameraUnavailableReason::DeviceFailure,
    }));
    rig.advance(Duration::from_millis(200));
    assert!(matches!(
        rig.state(),
        CameraState::Error {
            error: CameraError::Fatal {
                cause: FatalCause::RecoveryExhausted { .. }
            }
        }
    ));
    // Пользователь вернулся в приложение — это свежая попытка.
    assert_eq!(rig.send(CameraCommand::EnterBackground), CommandOutcome::Accepted);
    assert_eq!(rig.send(CameraCommand::EnterForeground), CommandOutcome::Accepted);
    assert_eq!(rig.state_name(), "streaming");
}

#[test]
fn open_failures_pick_the_plan_by_reason() {
    let cases = [
        (
            CameraUnavailableReason::InUseByAnotherClient,
            RecoveryPlan::WaitForCameraAvailable,
        ),
        // Лимит камер общий, сигнала «свободна» для желаемой камеры не будет: повтор по таймеру.
        (
            CameraUnavailableReason::TooManyCamerasOpen,
            RecoveryPlan::RetryAfterDelay {
                attempt: 1,
                delay_ms: 200,
            },
        ),
        (
            CameraUnavailableReason::PermissionDenied,
            RecoveryPlan::WaitForExternalAction,
        ),
        (
            CameraUnavailableReason::DisabledByPolicy,
            RecoveryPlan::WaitForExternalAction,
        ),
        (
            CameraUnavailableReason::DeviceFailure,
            RecoveryPlan::RetryAfterDelay {
                attempt: 1,
                delay_ms: 200,
            },
        ),
    ];
    for (reason, plan) in cases {
        let mut rig = Rig::new();
        rig.fake.push_open_result(Err(OpenFailure::Unavailable { reason }));
        rig.send(CameraCommand::Open { target: wide() });
        assert_eq!(rig.state(), recoverable(reason, plan), "{}", reason.name());
        assert_eq!(rig.engine.status().last_unavailable, Some(reason));
        assert!(!rig.fake.is_device_open());
    }
}

#[test]
fn device_error_codes_map_to_reasons() {
    let cases = [
        (
            DeviceErrorKind::CameraInUse,
            CameraUnavailableReason::InUseByAnotherClient,
        ),
        (
            DeviceErrorKind::MaxCamerasInUse,
            CameraUnavailableReason::TooManyCamerasOpen,
        ),
        (
            DeviceErrorKind::CameraDisabled,
            CameraUnavailableReason::DisabledByPolicy,
        ),
        (DeviceErrorKind::CameraDevice, CameraUnavailableReason::DeviceFailure),
        (DeviceErrorKind::Unknown(77), CameraUnavailableReason::DeviceFailure),
        (DeviceErrorKind::CameraService, CameraUnavailableReason::ServiceFailure),
    ];
    for (error, reason) in cases {
        let mut rig = Rig::new();
        rig.start_streaming_on_wide();
        rig.fake.fail_current_device(error);
        rig.pump();
        assert_eq!(rig.engine.status().last_unavailable, Some(reason), "{error:?}");
        assert_eq!(rig.state_name(), "error");
        assert!(!rig.fake.is_device_open());
    }
}

#[test]
fn permission_denied_waits_for_foreground_and_ignores_availability_and_timers() {
    let mut rig = Rig::new();
    rig.fake.push_open_result(Err(OpenFailure::Unavailable {
        reason: CameraUnavailableReason::PermissionDenied,
    }));
    rig.send(CameraCommand::Open { target: wide() });
    rig.send(CameraCommand::Configure { mode: mode_raw10_60() });
    rig.fake.take_calls();
    rig.feed(BackendEvent::CameraAvailability {
        camera: camera_id("2"),
        availability: CameraAvailability::Available,
    });
    rig.advance(Duration::from_secs(3600));
    assert!(rig.fake.take_calls().is_empty());
    assert_eq!(rig.engine.next_deadline(), None);

    // Разрешение выдали, приложение вернулось на передний план.
    assert_eq!(rig.send(CameraCommand::EnterForeground), CommandOutcome::Accepted);
    assert_eq!(rig.state_name(), "opened");
    assert_eq!(
        recovered_events(&rig.fresh_events()),
        vec![(CameraUnavailableReason::PermissionDenied, 1)]
    );
}

#[test]
fn backend_failure_on_open_is_treated_as_device_failure_with_delayed_retry() {
    let mut rig = Rig::new();
    rig.fake
        .push_open_result(Err(OpenFailure::Backend(crate::backend::BackendFailure {
            operation: "ACameraManager_openCamera",
            code: -10004,
            detail: "ERROR_CAMERA_DEVICE".to_owned(),
        })));
    rig.send(CameraCommand::Open { target: wide() });
    assert_eq!(
        rig.state(),
        recoverable(
            CameraUnavailableReason::DeviceFailure,
            RecoveryPlan::RetryAfterDelay {
                attempt: 1,
                delay_ms: 200
            }
        )
    );
    rig.advance(Duration::from_millis(200));
    assert_eq!(rig.state_name(), "opened");
}

#[test]
fn late_disconnect_of_a_previous_device_does_not_touch_the_new_one() {
    let mut rig = Rig::new();
    rig.start_streaming_on_wide();
    let old_generation = rig.fake.current_device_generation().expect("устройство открыто");
    rig.fake.disconnect_current_device();
    rig.pump();
    rig.feed(BackendEvent::CameraAvailability {
        camera: camera_id("2"),
        availability: CameraAvailability::Available,
    });
    assert_eq!(rig.state_name(), "streaming");
    let stale_before = rig.engine.status().counters.stale_backend_events;
    rig.fake.take_calls();

    // Запоздалое «отключено» от закрытого поколения.
    rig.feed(BackendEvent::DeviceDisconnected { device: old_generation });
    rig.feed(BackendEvent::DeviceError {
        device: old_generation,
        error: DeviceErrorKind::CameraService,
    });
    assert_eq!(rig.state_name(), "streaming");
    assert!(rig.fake.is_device_open());
    assert!(rig.fake.take_calls().is_empty());
    assert_eq!(rig.engine.status().counters.stale_backend_events, stale_before + 2);
    assert_eq!(
        rig.fake.current_device_generation(),
        Some(DeviceGeneration(old_generation.0 + 1))
    );
}

#[test]
fn stream_configuration_failure_keeps_the_device_opened_and_drops_the_desired_stream() {
    let mut rig = Rig::new();
    rig.send(CameraCommand::Open { target: wide() });
    rig.fresh_events();
    rig.fake.push_configure_failure(ConfigureFailure::RejectedByHal {
        detail: "STREAM_CONFIGURE_FAIL".to_owned(),
    });
    rig.send(CameraCommand::Configure { mode: mode_raw10_60() });
    let events = rig.fresh_events();
    assert!(events.iter().any(|event| matches!(
        event,
        CameraEvent::StreamConfigurationFailed {
            failure: ConfigureFailure::RejectedByHal { .. },
            ..
        }
    )));
    assert_eq!(rig.state_name(), "opened");
    assert!(rig.fake.is_device_open(), "устройство остаётся открытым");
    assert!(rig.engine.status().configured.is_none());
    assert!(rig.engine.status().last_unavailable.is_none(), "это не недоступность");
    assert_eq!(rig.engine.next_deadline(), None, "повторов нет");
    // Желаемый режим сброшен: поток без нового режима не стартует.
    assert_eq!(
        rig.send(CameraCommand::StartStreaming {
            intent: CaptureIntent::Preview
        }),
        CommandOutcome::InvalidState(crate::command::InvalidStateReason::NoStreamModeRequested)
    );
}

#[test]
fn backend_declared_unsupported_stream_is_not_unavailability_either() {
    let mut rig = Rig::new();
    rig.send(CameraCommand::Open { target: wide() });
    rig.fake.push_configure_failure(ConfigureFailure::Unsupported(
        UnsupportedStream::FrameRateNotAchievable {
            requested: mode_raw10_60().frame_rate,
        },
    ));
    rig.send(CameraCommand::Configure { mode: mode_raw10_60() });
    assert_eq!(rig.state_name(), "opened");
    assert_eq!(rig.engine.status().counters.recovery_attempts, 0);
}

#[test]
fn request_failure_with_disconnect_is_eviction() {
    let mut rig = Rig::new();
    rig.send(CameraCommand::Open { target: wide() });
    rig.send(CameraCommand::Configure { mode: mode_raw10_60() });
    rig.fake.push_request_result(Err(RequestFailure::DeviceDisconnected));
    rig.send(CameraCommand::StartStreaming {
        intent: CaptureIntent::Preview,
    });
    assert_eq!(
        rig.state(),
        recoverable(CameraUnavailableReason::Evicted, RecoveryPlan::WaitForCameraAvailable)
    );
    assert!(!rig.fake.is_device_open());
}

#[test]
fn unexpected_session_close_retries_after_the_backoff_delay_and_counts_an_attempt() {
    let mut rig = Rig::new();
    rig.start_streaming_on_wide();
    rig.fake.close_current_session_from_platform();
    rig.pump();
    // Не мгновенно: движок ждёт задержку из `retry_backoff_ms`.
    assert_eq!(
        rig.state(),
        recoverable(
            CameraUnavailableReason::DeviceFailure,
            RecoveryPlan::RetryAfterDelay {
                attempt: 1,
                delay_ms: 200
            }
        )
    );
    assert_eq!(call_names(&rig.fake.take_calls()), vec!["close_device"]);
    rig.advance(Duration::from_millis(199));
    assert_eq!(rig.state_name(), "error", "раньше срока повтора нет");

    rig.advance(Duration::from_millis(1));
    assert_eq!(rig.state_name(), "streaming");
    assert_eq!(rig.engine.status().stream.session, Some(SessionGeneration(2)));
    assert_eq!(
        call_names(&rig.fake.take_calls()),
        vec!["open_device", "configure_session", "set_repeating_request"]
    );
    assert_eq!(
        recovered_events(&rig.fresh_events()),
        vec![(CameraUnavailableReason::DeviceFailure, 1)]
    );
    assert_eq!(rig.engine.status().counters.recovery_attempts, 1);
}

#[test]
fn new_open_command_cancels_waiting_for_the_old_camera() {
    let mut rig = Rig::new();
    rig.fake.push_open_result(Err(OpenFailure::Unavailable {
        reason: CameraUnavailableReason::InUseByAnotherClient,
    }));
    rig.send(CameraCommand::Open { target: wide() });
    assert_eq!(rig.state_name(), "error");
    // Пользователь выбрал фронт — не ждём основную.
    assert_eq!(
        rig.send(CameraCommand::Open {
            target: CameraTarget::Role(LensRole::Front)
        }),
        CommandOutcome::Accepted
    );
    assert_eq!(rig.state_name(), "opened");
    assert_eq!(rig.fake.open_camera(), Some(camera_id("1")));
}

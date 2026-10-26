//! Передний план и фон, каталог, отказ бэкенда, паника и учёт кадров.

use std::time::Duration;

use camera_contract::availability::CameraUnavailableReason;
use camera_contract::capture_result::FrameNumber;
use camera_contract::fixtures::camera_id;

use super::*;
use crate::backend::{BackendFailure, CaptureFailureReason, SessionGeneration};
use crate::command::InvalidStateReason;
use crate::fake::BackendMethod;

fn catalog_failure() -> BackendFailure {
    BackendFailure {
        operation: "ACameraManager_getCameraIdList",
        code: -1,
        detail: "служба камеры недоступна".to_owned(),
    }
}

#[test]
fn commands_in_background_do_not_open_the_camera_and_enter_foreground_brings_it_up() {
    let mut rig = Rig::with_config(CameraEngineConfig::default(), AppPresence::Background);
    assert_eq!(
        rig.state(),
        CameraState::Closed {
            reason: ClosedReason::AppInBackground
        }
    );
    assert_eq!(
        rig.send(CameraCommand::Open { target: wide() }),
        CommandOutcome::Accepted
    );
    rig.send(CameraCommand::Configure { mode: mode_raw10_60() });
    rig.send(CameraCommand::StartStreaming {
        intent: CaptureIntent::Preview,
    });
    assert!(rig.fake.take_calls().is_empty(), "в фоне ни одного вызова бэкенда");
    assert_eq!(
        rig.state(),
        CameraState::Closed {
            reason: ClosedReason::AppInBackground
        }
    );

    assert_eq!(rig.send(CameraCommand::EnterForeground), CommandOutcome::Accepted);
    assert_eq!(rig.state_name(), "streaming");
    assert_eq!(rig.engine.status().presence, AppPresence::Foreground);
    assert_eq!(
        call_names(&rig.fake.take_calls()),
        vec!["open_device", "configure_session", "set_repeating_request"]
    );
    assert_eq!(
        rig.send(CameraCommand::EnterForeground),
        CommandOutcome::AlreadyInState,
        "повторный возврат — no-op"
    );
}

#[test]
fn enter_background_while_streaming_closes_the_device_and_foreground_resumes_the_same_stream() {
    let mut rig = Rig::new();
    rig.start_streaming_on_wide();
    assert_eq!(rig.send(CameraCommand::EnterBackground), CommandOutcome::Accepted);
    assert_eq!(
        rig.state(),
        CameraState::Closed {
            reason: ClosedReason::AppInBackground
        }
    );
    assert_eq!(call_names(&rig.fake.take_calls()), vec!["close_device"]);
    assert!(!rig.fake.is_device_open());
    assert_eq!(rig.send(CameraCommand::EnterBackground), CommandOutcome::AlreadyInState);

    // Желаемое сохранено: возврат поднимает тот же поток на той же камере.
    rig.send(CameraCommand::EnterForeground);
    assert_eq!(rig.state_name(), "streaming");
    assert_eq!(rig.fake.open_camera(), Some(camera_id("2")));
    assert_eq!(
        rig.engine.status().configured.map(|c| c.stream.raw_format),
        Some(RawFormat::Raw10)
    );
    assert_eq!(rig.engine.status().counters.devices_opened, 2);
}

#[test]
fn availability_signal_in_background_is_ignored_and_foreground_recovers_the_pending_error() {
    let mut rig = Rig::new();
    rig.start_streaming_on_wide();
    rig.fake.disconnect_current_device();
    rig.pump();
    assert_eq!(rig.state_name(), "error");
    rig.send(CameraCommand::EnterBackground);
    rig.fake.take_calls();
    rig.fresh_events();

    rig.feed(BackendEvent::CameraAvailability {
        camera: camera_id("2"),
        availability: crate::backend::CameraAvailability::Available,
    });
    rig.advance(Duration::from_secs(10));
    assert!(rig.fake.take_calls().is_empty(), "в фоне попыток нет");
    assert_eq!(
        rig.state(),
        CameraState::Closed {
            reason: ClosedReason::AppInBackground
        }
    );

    rig.send(CameraCommand::EnterForeground);
    assert_eq!(rig.state_name(), "streaming");
    let events = rig.fresh_events();
    assert!(events.contains(&CameraEvent::Recovered {
        reason: CameraUnavailableReason::Evicted,
        attempts: 1
    }));
}

#[test]
fn enter_foreground_resets_the_consecutive_attempt_counter() {
    let mut rig = Rig::new();
    rig.start_streaming_on_wide();
    rig.fake
        .fail_current_device(crate::backend::DeviceErrorKind::CameraDevice);
    rig.pump();
    // Две неудачные попытки подряд.
    for delay_ms in [200_u64, 500] {
        rig.fake.push_open_result(Err(crate::backend::OpenFailure::Unavailable {
            reason: CameraUnavailableReason::DeviceFailure,
        }));
        rig.advance(Duration::from_millis(delay_ms));
    }
    assert_eq!(rig.engine.status().counters.recovery_attempts, 2);
    rig.send(CameraCommand::EnterForeground);
    // Попытка после возврата — снова первая подряд: Recovered{attempts: 1}.
    assert!(rig.fresh_events().contains(&CameraEvent::Recovered {
        reason: CameraUnavailableReason::DeviceFailure,
        attempts: 1
    }));
    assert_eq!(
        rig.engine.status().counters.recovery_attempts,
        3,
        "общий счётчик не обнуляется"
    );
}

#[test]
fn catalog_failure_is_fatal_commands_fail_and_foreground_rereads_the_catalog() {
    let fake = FakeBackendControl::new();
    fake.set_catalog_failure(Some(catalog_failure()));
    let mut rig = Rig::over_fake(CameraEngineConfig::default(), AppPresence::Foreground, fake);
    rig.engine.load_catalog(rig.now);
    assert_eq!(
        rig.state(),
        CameraState::Error {
            error: CameraError::Fatal {
                cause: FatalCause::CatalogUnavailable {
                    failure: catalog_failure()
                }
            }
        }
    );
    assert!(rig.engine.status().catalog.cameras().is_empty());
    assert_eq!(
        rig.send(CameraCommand::Open { target: wide() }),
        CommandOutcome::InvalidState(InvalidStateReason::EngineFailed)
    );

    // Служба поднялась; возврат на передний план читает каталог заново.
    rig.fake.set_catalog_failure(None);
    assert_eq!(rig.send(CameraCommand::EnterForeground), CommandOutcome::Accepted);
    assert_eq!(
        rig.state(),
        CameraState::Closed {
            reason: ClosedReason::NotRequested
        }
    );
    assert_eq!(rig.engine.status().catalog.cameras().len(), 4);
    assert_eq!(
        rig.send(CameraCommand::Open { target: wide() }),
        CommandOutcome::Accepted
    );
    assert_eq!(rig.state_name(), "opened");
}

#[test]
fn failed_backend_is_fatal_and_enter_foreground_does_not_resurrect_it() {
    let mut engine = CameraEngine::failed_backend(
        CameraEngineConfig::default(),
        AppPresence::Foreground,
        BackendFailure {
            operation: "factory",
            code: 1,
            detail: "нет libcamera2ndk".to_owned(),
        },
    );
    let now = Instant::now();
    engine.handle_command(CommandId(1), CameraCommand::EnterForeground, now);
    engine.handle_command(CommandId(2), CameraCommand::Open { target: wide() }, now);
    let events = engine.drain_events();
    let failed = CommandOutcome::InvalidState(InvalidStateReason::EngineFailed);
    assert_eq!(
        events.iter().find_map(|e| e.command_outcome(CommandId(1))),
        Some(&failed)
    );
    assert_eq!(
        events.iter().find_map(|e| e.command_outcome(CommandId(2))),
        Some(&failed)
    );
    assert!(matches!(
        engine.status().state,
        CameraState::Error {
            error: CameraError::Fatal {
                cause: FatalCause::BackendUnavailable { .. }
            }
        }
    ));
}

#[test]
fn role_classifier_diagnostics_are_kept_for_the_report() {
    let mut rig = Rig::new();
    assert!(rig.engine.role_diagnostics().is_empty());
    let status = rig.engine.status();
    assert_eq!(
        status.catalog.role_of(&camera_id("3")),
        Some(camera_contract::ids::LensRole::UltraWide)
    );
    rig.engine.shutdown();
}

#[test]
fn current_session_results_feed_cadence_and_foreign_sessions_are_stale() {
    let mut rig = Rig::new();
    rig.start_streaming_on_wide();
    rig.fake.complete_frames(60, Duration::from_nanos(16_666_667));
    rig.pump();
    let stats = rig.engine.status().stream;
    assert_eq!(stats.results_completed, 60);
    assert_eq!(stats.intervals, 59);
    assert_eq!(stats.gaps, 0);
    let fps = stats.measured_frames_per_second.expect("интервалы есть");
    assert!((fps - 60.0).abs() < 0.01, "{fps}");

    // Результаты чужой (старой) сессии и сбой запроса чужой сессии не попадают в темп.
    let stale_before = rig.engine.status().counters.stale_backend_events;
    let foreign = SessionGeneration(99);
    rig.feed(BackendEvent::CaptureFailed {
        session: foreign,
        frame_number: FrameNumber(1),
        reason: CaptureFailureReason::Error,
    });
    assert_eq!(rig.engine.status().stream.capture_failures, 0);
    assert_eq!(rig.engine.status().counters.stale_backend_events, stale_before + 1);
    rig.feed(BackendEvent::CaptureFailed {
        session: SessionGeneration(1),
        frame_number: FrameNumber(2),
        reason: CaptureFailureReason::Flushed,
    });
    assert_eq!(rig.engine.status().stream.captures_flushed, 1);
}

#[test]
fn new_session_resets_the_stream_stats() {
    let mut rig = Rig::new();
    rig.start_streaming_on_wide();
    rig.fake.complete_frames(10, Duration::from_nanos(16_666_667));
    rig.pump();
    assert_eq!(rig.engine.status().stream.results_completed, 10);
    rig.send(CameraCommand::Configure { mode: mode_raw16_30() });
    let stats = rig.engine.status().stream;
    assert_eq!(stats.session, Some(SessionGeneration(2)));
    assert_eq!(stats.results_completed, 0);
    assert_eq!(stats.target_frame_duration.map(|d| d.as_nanos()), Some(33_333_333));
}

#[test]
fn panic_in_backend_marks_engine_panicked_closes_device_and_commands_get_engine_failed() {
    let mut rig = Rig::new();
    rig.send(CameraCommand::Open { target: wide() });
    rig.fake.push_panic_on(BackendMethod::ConfigureSession);
    let id = CommandId(500);
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rig.engine
            .handle_command(id, CameraCommand::Configure { mode: mode_raw10_60() }, rig.now);
    }));
    assert!(outcome.is_err(), "fake запаниковал внутри итерации");
    rig.engine.fail_after_panic("тест".to_owned());
    assert!(matches!(
        rig.state(),
        CameraState::Error {
            error: CameraError::Fatal {
                cause: FatalCause::EnginePanicked { .. }
            }
        }
    ));
    assert!(!rig.fake.is_device_open());
    assert_eq!(rig.engine.next_deadline(), None);
    // Итог команды, на которой упали, уже опубликован до действий; дальше все команды — EngineFailed.
    rig.engine.answer_if_unanswered(id, "configure");
    let answers = rig
        .engine
        .drain_events()
        .iter()
        .filter(|e| e.command_outcome(id).is_some())
        .count();
    assert_eq!(answers, 1, "ровно один итог");
    rig.engine.handle_command(CommandId(501), CameraCommand::Close, rig.now);
    let events = rig.engine.drain_events();
    assert_eq!(
        events.iter().find_map(|e| e.command_outcome(CommandId(501))),
        Some(&CommandOutcome::InvalidState(InvalidStateReason::EngineFailed))
    );
}

//! Валидация команд и их явные итоги; неудачная команда не меняет желаемое и реальное.

use camera_contract::catalog::TargetUnavailable;
use camera_contract::fixtures::{camera_id, main_wide_camera, oneplus11_visible_cameras};
use camera_contract::ids::LensRole;
use camera_contract::raw_format::RawFormat;

use super::*;
use crate::backend::{BackendFailure, UnsupportedStream};
use crate::command::{InvalidStateReason, UnsupportedCommand};

#[test]
fn reopening_the_same_camera_is_already_in_state_and_touches_nothing() {
    let mut rig = Rig::new();
    rig.send(CameraCommand::Open { target: wide() });
    rig.fresh_events();
    rig.fake.take_calls();
    let counters_before = rig.engine.status().counters;

    // Та же роль и та же камера по ID — оба раза no-op с явным итогом.
    assert_eq!(
        rig.send(CameraCommand::Open { target: wide() }),
        CommandOutcome::AlreadyInState
    );
    assert_eq!(
        rig.send(CameraCommand::Open {
            target: CameraTarget::Camera(camera_id("2"))
        }),
        CommandOutcome::AlreadyInState
    );
    assert!(rig.fake.take_calls().is_empty(), "ни одного лишнего вызова бэкенда");
    assert!(rig.fresh_state_names().is_empty());
    let counters_after = rig.engine.status().counters;
    assert_eq!(counters_after.devices_opened, counters_before.devices_opened);
    assert_eq!(counters_after.commands_handled, counters_before.commands_handled + 2);
}

#[test]
fn open_of_unknown_camera_is_unsupported_and_leaves_desired_untouched() {
    let mut rig = Rig::new();
    let outcome = rig.send(CameraCommand::Open {
        target: CameraTarget::Camera(camera_id("42")),
    });
    assert_eq!(
        outcome,
        CommandOutcome::Unsupported(UnsupportedCommand::Target(TargetUnavailable::UnknownCamera(camera_id(
            "42"
        ))))
    );
    // Желаемое не появилось: configure по-прежнему «нет камеры».
    assert_eq!(
        rig.send(CameraCommand::Configure { mode: mode_raw10_60() }),
        CommandOutcome::InvalidState(InvalidStateReason::NoCameraRequested)
    );
    assert!(rig.fake.take_calls().is_empty());
}

#[test]
fn open_of_missing_tele_role_is_unsupported() {
    let mut rig = Rig::new();
    assert_eq!(
        rig.send(CameraCommand::Open {
            target: CameraTarget::Role(LensRole::Tele)
        }),
        CommandOutcome::Unsupported(UnsupportedCommand::Target(TargetUnavailable::RoleNotPresent(
            LensRole::Tele
        )))
    );
    assert_eq!(rig.state_name(), "closed");
}

#[test]
fn configure_and_start_streaming_require_their_prerequisites() {
    let mut rig = Rig::new();
    assert_eq!(
        rig.send(CameraCommand::Configure { mode: mode_raw10_60() }),
        CommandOutcome::InvalidState(InvalidStateReason::NoCameraRequested)
    );
    rig.send(CameraCommand::Open { target: wide() });
    assert_eq!(
        rig.send(CameraCommand::StartStreaming {
            intent: CaptureIntent::Preview
        }),
        CommandOutcome::InvalidState(InvalidStateReason::NoStreamModeRequested)
    );
    assert_eq!(
        rig.send(CameraCommand::SwitchTo { role: LensRole::Front }),
        CommandOutcome::Accepted,
        "switch_to с камерой, но без режима допустим"
    );
}

#[test]
fn switch_to_without_any_camera_is_invalid_state() {
    let mut rig = Rig::new();
    assert_eq!(
        rig.send(CameraCommand::SwitchTo { role: LensRole::Wide }),
        CommandOutcome::InvalidState(InvalidStateReason::NoCameraRequested)
    );
}

#[test]
fn switch_to_unavailable_tele_is_unsupported_and_changes_neither_desired_nor_real_state() {
    let mut rig = Rig::new();
    rig.start_streaming_on_wide();
    let status_before = rig.engine.status();
    assert_eq!(
        rig.send(CameraCommand::SwitchTo { role: LensRole::Tele }),
        CommandOutcome::Unsupported(UnsupportedCommand::Target(TargetUnavailable::RoleNotPresent(
            LensRole::Tele
        )))
    );
    assert!(rig.fake.take_calls().is_empty());
    assert!(rig.fresh_state_names().is_empty());
    let status_after = rig.engine.status();
    assert_eq!(status_after.state, status_before.state);
    assert_eq!(status_after.configured, status_before.configured);
    assert_eq!(rig.fake.open_camera(), Some(camera_id("2")));
    // Желаемое на месте: ещё один switch_to на ту же основную камеру — «уже так».
    assert_eq!(
        rig.send(CameraCommand::SwitchTo { role: LensRole::Wide }),
        CommandOutcome::AlreadyInState
    );
}

#[test]
fn repeated_configure_start_and_stop_report_already_in_state() {
    let mut rig = Rig::new();
    rig.send(CameraCommand::Open { target: wide() });
    assert_eq!(
        rig.send(CameraCommand::StopStreaming),
        CommandOutcome::AlreadyInState,
        "поток и не просили"
    );
    rig.send(CameraCommand::Configure { mode: mode_raw10_60() });
    assert_eq!(
        rig.send(CameraCommand::Configure { mode: mode_raw10_60() }),
        CommandOutcome::AlreadyInState
    );
    rig.send(CameraCommand::StartStreaming {
        intent: CaptureIntent::Preview,
    });
    assert_eq!(
        rig.send(CameraCommand::StartStreaming {
            intent: CaptureIntent::Preview
        }),
        CommandOutcome::AlreadyInState
    );
    // Другой интент — не no-op: запрос заменяется.
    assert_eq!(
        rig.send(CameraCommand::StartStreaming {
            intent: CaptureIntent::VideoRecord
        }),
        CommandOutcome::Accepted
    );
}

#[test]
fn configure_with_format_the_camera_does_not_offer_is_unsupported() {
    let fake = FakeBackendControl::new();
    let mut cameras = oneplus11_visible_cameras();
    let mut raw16_only = main_wide_camera();
    raw16_only
        .capabilities
        .raw_streams
        .retain(|option| option.format == RawFormat::Raw16);
    cameras.retain(|camera| camera.id != raw16_only.id);
    cameras.push(raw16_only);
    fake.set_cameras(cameras);
    let mut rig = Rig::over_fake(CameraEngineConfig::default(), AppPresence::Foreground, fake);
    rig.engine.load_catalog(rig.now);
    rig.send(CameraCommand::Open { target: wide() });
    rig.fake.take_calls();
    assert_eq!(
        rig.send(CameraCommand::Configure { mode: mode_raw10_60() }),
        CommandOutcome::Unsupported(UnsupportedCommand::Stream(UnsupportedStream::FormatNotOffered {
            format: RawFormat::Raw10
        }))
    );
    assert!(rig.fake.take_calls().is_empty());
}

#[test]
fn switch_to_a_camera_that_lacks_the_desired_format_is_unsupported_and_keeps_the_old_camera() {
    let fake = FakeBackendControl::new();
    let mut cameras = oneplus11_visible_cameras();
    for camera in &mut cameras {
        if camera.id == camera_id("3") {
            camera
                .capabilities
                .raw_streams
                .retain(|option| option.format == RawFormat::Raw16);
        }
    }
    fake.set_cameras(cameras);
    let mut rig = Rig::over_fake(CameraEngineConfig::default(), AppPresence::Foreground, fake);
    rig.engine.load_catalog(rig.now);
    rig.start_streaming_on_wide();
    assert_eq!(
        rig.send(CameraCommand::SwitchTo {
            role: LensRole::UltraWide
        }),
        CommandOutcome::Unsupported(UnsupportedCommand::Stream(UnsupportedStream::FormatNotOffered {
            format: RawFormat::Raw10
        }))
    );
    assert_eq!(rig.state_name(), "streaming");
    assert!(rig.fake.take_calls().is_empty());
}

#[test]
fn every_command_gets_exactly_one_outcome_event_and_counts_in_status() {
    let mut rig = Rig::new();
    let before = rig.engine.status().counters.commands_handled;
    rig.send(CameraCommand::Open { target: wide() });
    rig.send(CameraCommand::Open { target: wide() });
    rig.send(CameraCommand::Close);
    // `send` сам проверяет «ровно один итог»; здесь — счётчик.
    assert_eq!(rig.engine.status().counters.commands_handled, before + 3);
}

#[test]
fn outcome_is_published_before_the_state_changes_it_caused() {
    let mut rig = Rig::new();
    rig.send(CameraCommand::Open { target: wide() });
    let events = rig.fresh_events();
    assert!(matches!(
        events.first(),
        Some(CameraEvent::CommandHandled { name: "open", .. })
    ));
    assert!(matches!(events.get(1), Some(CameraEvent::StateChanged { .. })));
}

#[test]
fn commands_in_fatal_state_answer_engine_failed_except_close_and_background() {
    let fake = FakeBackendControl::new();
    fake.set_catalog_failure(Some(BackendFailure {
        operation: "ACameraManager_getCameraIdList",
        code: -1,
        detail: "нет службы".to_owned(),
    }));
    let mut rig = Rig::over_fake(CameraEngineConfig::default(), AppPresence::Foreground, fake);
    rig.engine.load_catalog(rig.now);
    assert!(matches!(
        rig.state(),
        CameraState::Error {
            error: CameraError::Fatal {
                cause: FatalCause::CatalogUnavailable { .. }
            }
        }
    ));
    let failed = CommandOutcome::InvalidState(InvalidStateReason::EngineFailed);
    assert_eq!(rig.send(CameraCommand::Open { target: wide() }), failed);
    assert_eq!(rig.send(CameraCommand::Configure { mode: mode_raw10_60() }), failed);
    assert_eq!(rig.send(CameraCommand::SwitchTo { role: LensRole::Wide }), failed);
    assert_eq!(rig.send(CameraCommand::EnterBackground), CommandOutcome::Accepted);
    assert_eq!(rig.send(CameraCommand::Close), CommandOutcome::Accepted);
}

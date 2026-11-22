//! Тесты вспышки в сервисе снимка (14d) на НАСТОЯЩЕЙ службе записи: режим × совет камеры → план рендеру и команда
//! камере; порядок «`Begin`, потом камера»; отказ доставки команды → единственный итог экрану.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "в тестах падение — способ сообщить о провале"
)]

use camera_contract::flash::FlashShotId;
use camera_core::CommandRejected;
use camera_core::flash_status::{FlashAdvice, FlashAdviceUnavailable};
use camera_screen::ScreenAction;
use camera_screen::settings::FlashMode;
use camera_screen::shutter::{CaptureFailureReason, CaptureOutcome, ShotRequestId};
use still_contract::develop::DevelopEvent;
use still_contract::frame_choice::NoFrameReason;
use still_contract::plan::{FrameSelection, StillId};

use super::*;
use crate::camera_screen_input::ScreenInput;
use crate::flash_still_relay::FlashStillRelay;
use crate::still_test_support::{LoggedCommand, StillRig, shot};

fn shot_with_flash(request: u64, mode: FlashMode) -> ShotRequest {
    ShotRequest {
        flash: mode,
        ..shot(request)
    }
}

fn press(rig: &StillRig, mode: FlashMode) {
    let outcome = rig
        .capture
        .submit(shot_with_flash(1, mode), MonotonicNanos::from_nanos(0));
    assert_eq!(outcome, SubmitOutcome::Accepted);
}

#[test]
fn off_takes_the_shown_frame_and_never_touches_the_flash_camera() {
    let rig = StillRig::start();
    rig.flash_camera.set_advice(FlashAdvice::Fire);
    press(&rig, FlashMode::Off);
    assert_eq!(rig.render.plans()[0].selection, FrameSelection::ShownAtPress);
    assert!(rig.flash_camera.shots().is_empty(), "камере команды нет");
    assert_eq!(rig.flash_camera.status_reads(), 0, "Off не читает совет");
    assert_eq!(rig.render.command_log(), vec![(LoggedCommand::Begin, StillId(1))]);
}

#[test]
fn on_sends_begin_with_the_flash_frame_selection_and_then_the_camera_command() {
    let rig = StillRig::start();
    press(&rig, FlashMode::On);
    assert_eq!(
        rig.render.plans()[0].selection,
        FrameSelection::FlashFrame { shot: FlashShotId(1) },
        "номер вспышки = номер снимка службы"
    );
    assert_eq!(rig.flash_camera.shots(), vec![FlashShotId(1)]);
    assert_eq!(rig.flash_camera.status_reads(), 0, "On не читает совет");
    assert_eq!(rig.render.command_log(), vec![(LoggedCommand::Begin, StillId(1))]);
    assert_eq!(rig.capture_counters().flash_mode_on, 1);
}

#[test]
fn auto_fires_only_when_the_camera_advises_and_counts_every_reason() {
    let cases = [
        (FlashAdvice::Fire, true),
        (FlashAdvice::NotNeeded, false),
        (
            FlashAdvice::Unavailable {
                reason: FlashAdviceUnavailable::NoMeasurement,
            },
            false,
        ),
    ];
    for (advice, expect_flash) in cases {
        let rig = StillRig::start();
        rig.flash_camera.set_advice(advice);
        press(&rig, FlashMode::Auto);
        let selection = rig.render.plans()[0].selection;
        assert_eq!(
            selection == FrameSelection::FlashFrame { shot: FlashShotId(1) },
            expect_flash,
            "{advice:?}"
        );
        assert_eq!(rig.flash_camera.shots().len(), usize::from(expect_flash), "{advice:?}");
        let counters = rig.capture_counters();
        assert_eq!(
            (
                counters.flash_auto_fired,
                counters.flash_auto_not_needed,
                counters.flash_auto_advice_unavailable
            ),
            match advice {
                FlashAdvice::Fire => (1, 0, 0),
                FlashAdvice::NotNeeded => (0, 1, 0),
                FlashAdvice::Unavailable { .. } => (0, 0, 1),
            }
        );
        let routes = rig.capture_flash_routes();
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].uses_flash, expect_flash);
        assert_eq!(routes[0].mode, FlashModeName::Auto);
    }
}

#[test]
fn a_command_the_camera_did_not_accept_fails_the_still_through_the_render_with_one_outcome() {
    let mut rig = StillRig::start();
    rig.flash_camera.reject_commands(CommandRejected::QueueFull);
    press(&rig, FlashMode::On);

    // Рендер получил Begin ПЕРВЫМ и затем срыв (иначе он проигнорировал бы срыв).
    assert_eq!(
        rig.render.command_log(),
        vec![
            (LoggedCommand::Begin, StillId(1)),
            (LoggedCommand::FlashFailed, StillId(1))
        ]
    );
    assert_eq!(
        rig.render.flash_failures(),
        vec![(StillId(1), FlashStillFailure::CommandNotDelivered)]
    );
    assert_eq!(rig.capture_counters().flash_command_rejected, 1);

    // Рендер отвечает единственным NotSecured: экран получает ровно один итог «ошибка камеры».
    rig.render.emit(
        StillId(1),
        DevelopEvent::NotSecured {
            id: StillId(1),
            reason: NoFrameReason::FlashNotCaptured(FlashStillFailure::CommandNotDelivered),
        },
    );
    assert_eq!(
        rig.wait_screen_inputs(1),
        vec![ScreenInput::Action(ScreenAction::CaptureFinished {
            request: ShotRequestId(1),
            outcome: CaptureOutcome::Failed(CaptureFailureReason::Camera),
        })]
    );
    std::thread::sleep(std::time::Duration::from_millis(50));
    assert!(rig.screen_inputs().is_empty(), "второго итога нет");
}

#[test]
fn a_render_that_did_not_take_begin_gets_no_flash_command_for_the_camera() {
    let rig = StillRig::start_with(
        crate::photo_config::PhotoConfig::default(),
        crate::still_test_support::RenderMode::Unavailable,
    );
    press(&rig, FlashMode::On);
    assert!(
        rig.flash_camera.shots().is_empty(),
        "без Begin камеру не просим: ждать кадр некому"
    );
    assert_eq!(rig.capture_counters().render_refused, 1);
}

#[test]
fn camera_failure_from_the_relay_reaches_the_render_with_the_id_the_service_gave() {
    let rig = StillRig::start();
    press(&rig, FlashMode::On);
    // Камера позже сообщает срыв наблюдателю: мост подключён службой при запуске.
    let relay: &FlashStillRelay = &rig.flash_relay;
    camera_core::flash_status::FlashStillObserver::flash_still_finished(
        relay,
        &camera_core::flash_status::FlashStillReport::Failed {
            shot: rig.flash_camera.shots()[0],
            failure: FlashStillFailure::PreflashNotMeasured,
        },
    );
    assert_eq!(
        rig.render.flash_failures(),
        vec![(StillId(1), FlashStillFailure::PreflashNotMeasured)]
    );
}

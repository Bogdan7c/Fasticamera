//! Решения по снимку со вспышкой: причины отказов (и командой, и итогом снимка), взаимное
//! исключение с серией, совет для Auto, работа без наблюдателя.

use camera_contract::capabilities::{CameraCapabilities, RequestCapability};
use camera_contract::control::{ExposureBias, ExposureTime, Iso};
use camera_contract::flash::{FlashStillFailure, FlashUnit};
use camera_contract::three_a::{BracketExposure, ExposureBracket};
use std::time::Duration;

use super::flash_support::*;
use super::three_a_support::*;
use super::*;
use crate::command::{InvalidStateReason, UnsupportedCommand, UnsupportedThreeA};
use crate::flash_status::{FlashAdvice, FlashAdviceUnavailable, FlashPhase, FlashStillReport};

fn one_report(events: &[CameraEvent]) -> FlashStillReport {
    let reports = flash_reports(events);
    assert_eq!(reports.len(), 1, "ровно один итог на команду, а пришло {reports:?}");
    reports[0]
}

/// Итог команды и итог снимка идут в таком порядке: сначала `CommandHandled`, потом `FlashStillFinished`.
fn assert_handled_before_report(events: &[CameraEvent]) {
    let handled = events
        .iter()
        .position(|event| matches!(event, CameraEvent::CommandHandled { .. }))
        .expect("ответ на команду");
    let report = events
        .iter()
        .position(|event| matches!(event, CameraEvent::FlashStillFinished { .. }))
        .expect("итог снимка");
    assert!(handled < report, "итог снимка раньше ответа на команду: {events:?}");
}

fn expect_refusal(flash: &mut FlashRig, outcome: CommandOutcome, failure: FlashStillFailure) {
    flash.rig.fresh_events();
    let actual = flash.capture(SHOT);
    assert_eq!(actual, outcome);
    let events = flash.rig.fresh_events();
    assert_eq!(one_report(&events), FlashStillReport::Failed { shot: SHOT, failure });
    assert_handled_before_report(&events);
    assert_eq!(
        flash.observer.reports().last(),
        Some(&FlashStillReport::Failed { shot: SHOT, failure })
    );
    assert_eq!(flash.phase(), FlashPhase::Idle, "отказ не запускает последовательность");
}

#[test]
fn not_streaming_is_refused_by_command_and_by_report() {
    let mut flash = FlashRig::measured(8.0);
    flash.rig.send(CameraCommand::StopStreaming);
    expect_refusal(
        &mut flash,
        CommandOutcome::InvalidState(InvalidStateReason::NotStreaming),
        FlashStillFailure::NotStreaming,
    );
}

#[test]
fn camera_without_flash_unit_is_refused() {
    let mut flash = FlashRig::with_edited_wide(|camera| camera.capabilities.flash = FlashUnit::Absent);
    expect_refusal(
        &mut flash,
        CommandOutcome::Unsupported(UnsupportedCommand::ThreeA(UnsupportedThreeA::FlashUnit)),
        FlashStillFailure::NoFlashUnit,
    );
}

#[test]
fn camera_without_manual_sensor_is_refused() {
    let mut flash = FlashRig::with_edited_wide(|camera: &mut camera_contract::capabilities::CameraDescriptor| {
        let capabilities: &mut CameraCapabilities = &mut camera.capabilities;
        capabilities
            .request_capabilities
            .retain(|capability| *capability != RequestCapability::ManualSensor);
    });
    // Без ручного сенсора наш AE вообще не строит план: движок деградирует до автомата, команда
    // обязана отказать именно по ручному сенсору.
    flash.rig.fresh_events();
    let outcome = flash.capture(SHOT);
    assert_eq!(
        outcome,
        CommandOutcome::Unsupported(UnsupportedCommand::ThreeA(UnsupportedThreeA::ManualSensor))
    );
    assert_eq!(
        one_report(&flash.rig.fresh_events()),
        FlashStillReport::Failed {
            shot: SHOT,
            failure: FlashStillFailure::NeedsManualSensor
        }
    );
}

#[test]
fn vendor_exposure_is_refused_as_needing_own_exposure() {
    let mut rig = streaming_rig();
    rig.fresh_events();
    let outcome = rig.send(CameraCommand::CaptureFlashStill { shot: SHOT });
    assert_eq!(
        outcome,
        CommandOutcome::Unsupported(UnsupportedCommand::ThreeA(UnsupportedThreeA::FlashNeedsOwnExposure))
    );
    let events = rig.fresh_events();
    assert_eq!(
        one_report(&events),
        FlashStillReport::Failed {
            shot: SHOT,
            failure: FlashStillFailure::NeedsOwnExposure
        }
    );
    assert_handled_before_report(&events);
    assert!(
        !rig.fake
            .calls()
            .iter()
            .any(|call| matches!(call, BackendCall::SetRepeatingRequest { controls, .. } if controls.flash != camera_contract::flash::FlashControl::Off)),
        "отказ не зажигает светодиод"
    );
}

#[test]
fn second_flash_while_one_runs_is_refused_and_the_first_still_finishes() {
    let mut flash = FlashRig::measured(8.0);
    assert_eq!(flash.capture(SHOT), CommandOutcome::Accepted);
    let second = camera_contract::flash::FlashShotId(SHOT.0 + 1);
    flash.rig.fresh_events();
    assert_eq!(
        flash.capture(second),
        CommandOutcome::InvalidState(InvalidStateReason::FlashInProgress)
    );
    assert_eq!(
        one_report(&flash.rig.fresh_events()),
        FlashStillReport::Failed {
            shot: second,
            failure: FlashStillFailure::AlreadyInProgress
        }
    );
    assert_eq!(
        flash.phase(),
        FlashPhase::Preflash {
            shot: SHOT,
            remeasures: 0
        },
        "первый снимок не тронут"
    );
}

fn tiny_bracket() -> ExposureBracket {
    let frames = vec![BracketExposure {
        exposure_time: ExposureTime::new(Duration::from_millis(4)).expect("выдержка"),
        iso: Iso::new(100).expect("ISO"),
    }];
    ExposureBracket::new(frames, 32).expect("серия")
}

#[test]
fn bracket_and_flash_refuse_each_other() {
    // Вспышка идёт — серия отказана.
    let mut flash = FlashRig::measured(8.0);
    flash.capture(SHOT);
    assert_eq!(
        flash.rig.send(CameraCommand::CaptureBracket {
            bracket: tiny_bracket()
        }),
        CommandOutcome::InvalidState(InvalidStateReason::FlashInProgress)
    );
    // Серия идёт — вспышка отказана (и командой, и итогом).
    let mut flash = FlashRig::measured(8.0);
    assert_eq!(
        flash.rig.send(CameraCommand::CaptureBracket {
            bracket: tiny_bracket()
        }),
        CommandOutcome::Accepted
    );
    expect_refusal(
        &mut flash,
        CommandOutcome::InvalidState(InvalidStateReason::BracketInProgress),
        FlashStillFailure::AlreadyInProgress,
    );
}

#[test]
fn advice_follows_the_scene_threshold_without_user_bias() {
    let threshold = 2.0_f64.powf(5.5);
    let advice = |flash: &FlashRig| flash.rig.engine.status().three_a.flash.advice;

    let mut flash = FlashRig::unmeasured();
    assert_eq!(
        advice(&flash),
        FlashAdvice::Unavailable {
            reason: FlashAdviceUnavailable::NoMeasurement
        }
    );
    flash.measure(8.0);
    assert_eq!(advice(&flash), FlashAdvice::NotNeeded);
    // Ровно на пороге вспыхиваем (≥), чуть светлее — нет.
    flash.measure(threshold);
    assert_eq!(advice(&flash), FlashAdvice::Fire);
    flash.measure(threshold * 0.99);
    assert_eq!(advice(&flash), FlashAdvice::NotNeeded);
    // Компенсация пользователя совет не двигает.
    flash.rig.send(CameraCommand::SetExposureBias {
        bias: ExposureBias::from_ev(2.0).expect("EV"),
    });
    assert_eq!(advice(&flash), FlashAdvice::NotNeeded);
    flash.rig.send(CameraCommand::StopStreaming);
    assert_eq!(
        advice(&flash),
        FlashAdvice::Unavailable {
            reason: FlashAdviceUnavailable::NotStreaming
        }
    );
}

#[test]
fn advice_reports_missing_unit_and_vendor_exposure() {
    let flash = FlashRig::with_edited_wide(|camera| camera.capabilities.flash = FlashUnit::Absent);
    let status = flash.rig.engine.status().three_a.flash;
    assert_eq!(status.unit, FlashUnit::Absent);
    assert_eq!(
        status.advice,
        FlashAdvice::Unavailable {
            reason: FlashAdviceUnavailable::NoFlashUnit
        }
    );
    let rig = streaming_rig();
    let status = rig.engine.status().three_a.flash;
    assert_eq!(status.unit, FlashUnit::Present);
    assert_eq!(
        status.advice,
        FlashAdvice::Unavailable {
            reason: FlashAdviceUnavailable::NotOwnExposure
        }
    );
}

#[test]
fn engine_without_observer_still_reports_through_events() {
    let mut flash = FlashRig::without_observer(8.0);
    flash.rig.fresh_events();
    flash.capture(SHOT);
    flash.publish_good_samples(8.0, 4.0);
    flash.rig.advance(Duration::from_millis(100));
    flash
        .rig
        .fake
        .complete_flash_still(SHOT, camera_contract::flash::FlashResult::UNREPORTED);
    flash.rig.pump();
    let report = one_report(&flash.rig.fresh_events());
    assert!(matches!(report, FlashStillReport::Fired { shot, .. } if shot == SHOT));
    assert!(
        flash.observer.reports().is_empty(),
        "наблюдатель не подключён — он ничего не видел"
    );
}

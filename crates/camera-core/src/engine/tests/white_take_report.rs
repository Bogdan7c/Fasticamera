//! Отчёт о своей команде «взять белую» (`WhiteStatus::last_take`): все исходы, замещение, открытие приложения,
//! отказ без заявки.

use camera_contract::white::{
    WhiteConfidence, WhiteOrigin, WhiteSourceFailure, WhiteSourceId, WhiteSourceOutcome, WhiteTransition,
    WhiteUnavailableReason, WhiteUnknownReason,
};

use super::white_support::*;
use super::*;
use crate::command::InvalidStateReason;
use crate::white::{WhiteCancelCause, WhiteRequestOutcome, WhiteRequestState, WhiteTakeReport, WhiteTakeState};

fn take() -> CameraCommand {
    CameraCommand::TakeWhite {
        source: WhiteSourceId::PHONE,
        transition: WhiteTransition::Immediate,
    }
}

fn report(rig: &Rig) -> Option<WhiteTakeReport> {
    rig.white_status().last_take
}

/// Стенд: идёт поток, кадр был, заявка открытия (№1) уже получила ответ — отчётов о командах ещё нет.
fn rig_after_opening() -> (Rig, WhiteOutside) {
    let (mut rig, outside) = white_rig_streaming();
    rig.frame_after(Duration::ZERO);
    rig.deliver_answer(&outside, ready(1, setting(3_700.0, 0.0)));
    assert_eq!(report(&rig), None, "заявка открытия отчёт команды не заводит");
    (rig, outside)
}

#[test]
fn an_accepted_take_is_waiting_and_carries_its_own_command_number() {
    let (mut rig, _outside) = rig_after_opening();
    assert_eq!(rig.send(take()), CommandOutcome::Accepted);
    assert_eq!(
        report(&rig),
        Some(WhiteTakeReport {
            command: rig.last_command_id(),
            state: WhiteTakeState::Waiting
        })
    );
}

#[test]
fn every_source_outcome_reaches_the_report_of_its_command() {
    let cases: [(WhiteSourceOutcome, WhiteRequestOutcome); 5] = [
        (
            WhiteSourceOutcome::Ready {
                illuminant: camera_contract::white::SceneIlluminant::Uniform(setting(3_100.0, 0.0)),
                confidence: WhiteConfidence::FULL,
            },
            WhiteRequestOutcome::Applied {
                confidence: WhiteConfidence::FULL,
            },
        ),
        (WhiteSourceOutcome::Busy, WhiteRequestOutcome::Busy),
        (
            WhiteSourceOutcome::Unknown {
                reason: WhiteUnknownReason::SceneNotMeasurable,
            },
            WhiteRequestOutcome::Unknown {
                reason: WhiteUnknownReason::SceneNotMeasurable,
            },
        ),
        (
            WhiteSourceOutcome::Unavailable {
                reason: WhiteUnavailableReason::AutomationSilent,
            },
            WhiteRequestOutcome::Unavailable {
                reason: WhiteUnavailableReason::AutomationSilent,
            },
        ),
        (
            WhiteSourceOutcome::Failed {
                failure: WhiteSourceFailure::Executor,
            },
            WhiteRequestOutcome::Failed {
                failure: WhiteSourceFailure::Executor,
            },
        ),
    ];
    for (source_outcome, expected) in cases {
        let (mut rig, outside) = rig_after_opening();
        rig.send(take());
        let command = rig.last_command_id();
        rig.deliver_answer(&outside, answer(2, source_outcome));
        assert_eq!(
            report(&rig),
            Some(WhiteTakeReport {
                command,
                state: WhiteTakeState::Finished(expected)
            }),
            "{source_outcome:?}"
        );
    }
}

#[test]
fn a_take_without_an_answer_times_out_into_its_report() {
    let (mut rig, _outside) = rig_after_opening();
    rig.send(take());
    let command = rig.last_command_id();
    rig.advance(Duration::from_secs(4));
    assert_eq!(
        report(&rig),
        Some(WhiteTakeReport {
            command,
            state: WhiteTakeState::Finished(WhiteRequestOutcome::TimedOut)
        })
    );
}

#[test]
fn a_new_take_replaces_the_report_and_cancels_the_old_request() {
    let (mut rig, _outside) = rig_after_opening();
    rig.send(take());
    rig.fresh_events();
    rig.send(take());
    let second = rig.last_command_id();

    assert_eq!(
        report(&rig),
        Some(WhiteTakeReport {
            command: second,
            state: WhiteTakeState::Waiting
        }),
        "хранится последний отчёт"
    );
    let results = finished_requests(&rig.fresh_events());
    assert_eq!(
        results.iter().map(|result| result.result).collect::<Vec<_>>(),
        vec![WhiteRequestOutcome::Cancelled {
            by: WhiteCancelCause::ReplacedByNewRequest
        }],
        "прежняя заявка закрыта как замещённая"
    );
}

#[test]
fn a_manual_white_closes_the_report_as_cancelled_by_hand() {
    let (mut rig, _outside) = rig_after_opening();
    rig.send(take());
    let command = rig.last_command_id();
    rig.send(CameraCommand::SetWhite {
        setting: setting(4_400.0, 0.0),
        transition: WhiteTransition::Immediate,
    });
    assert_eq!(
        report(&rig),
        Some(WhiteTakeReport {
            command,
            state: WhiteTakeState::Finished(WhiteRequestOutcome::Cancelled {
                by: WhiteCancelCause::ManualWhite
            })
        })
    );
}

#[test]
fn going_to_background_closes_the_report_as_cancelled_by_background() {
    let (mut rig, _outside) = rig_after_opening();
    rig.send(take());
    let command = rig.last_command_id();
    rig.send(CameraCommand::EnterBackground);
    assert_eq!(
        report(&rig),
        Some(WhiteTakeReport {
            command,
            state: WhiteTakeState::Finished(WhiteRequestOutcome::Cancelled {
                by: WhiteCancelCause::Background
            })
        })
    );
}

#[test]
fn the_end_of_the_stream_reissues_the_request_but_the_report_keeps_waiting() {
    let (mut rig, outside) = rig_after_opening();
    rig.send(take());
    let command = rig.last_command_id();
    let before = outside.directive().request.map(|request| request.id);

    rig.send(CameraCommand::Close);

    let after = outside.directive().request.map(|request| request.id);
    assert_ne!(before, after, "заявка переиздана с новым номером");
    assert!(after.is_some());
    assert_eq!(
        report(&rig),
        Some(WhiteTakeReport {
            command,
            state: WhiteTakeState::Waiting
        })
    );
}

#[test]
fn the_opening_request_and_its_answer_leave_the_report_of_a_finished_take_alone() {
    let (mut rig, outside) = rig_after_opening();
    rig.send(take());
    let command = rig.last_command_id();
    rig.deliver_answer(&outside, ready(2, setting(3_100.0, 0.0)));
    let finished = report(&rig);
    assert_eq!(
        finished.map(|report| (report.command, matches!(report.state, WhiteTakeState::Finished(_)))),
        Some((command, true))
    );

    // Приложение ушло в фон и вернулось: открытие завело заявку (№3), поток снова идёт, источник ответил.
    rig.send(CameraCommand::EnterBackground);
    rig.send(CameraCommand::EnterForeground);
    // Желаемое сохранилось: поток поднимается сам.
    assert_eq!(rig.state_name(), "streaming");
    rig.frame_after(Duration::ZERO);
    assert!(matches!(rig.white_status().request, WhiteRequestState::Waiting { .. }));
    assert_eq!(report(&rig), finished, "заявка открытия отчёт не трогает");
    rig.deliver_answer(&outside, ready(3, setting(3_300.0, 0.0)));
    assert_eq!(rig.held().origin, WhiteOrigin::Source(WhiteSourceId::PHONE));
    assert_eq!(report(&rig), finished, "и её ответ — тоже");
}

#[test]
fn a_take_without_a_stream_is_not_asked_and_says_why_in_the_published_status() {
    let (mut rig, outside) = white_rig();
    let generation = outside.generation();
    assert_eq!(
        rig.send(take()),
        CommandOutcome::InvalidState(InvalidStateReason::NotStreaming)
    );
    assert_eq!(
        report(&rig),
        Some(WhiteTakeReport {
            command: rig.last_command_id(),
            state: WhiteTakeState::NotAsked {
                reason: InvalidStateReason::NotStreaming
            }
        })
    );
    assert_eq!(rig.white_status().request, WhiteRequestState::Idle { last: None });
    assert_eq!(outside.generation(), generation, "источнику ничего не передавали");
}

#[test]
fn a_take_in_fatal_is_not_asked_with_the_engine_failed_reason() {
    let fake = FakeBackendControl::new();
    fake.set_catalog_failure(Some(crate::backend::BackendFailure {
        operation: "ACameraManager_getCameraIdList",
        code: -1,
        detail: "служба камеры недоступна".to_owned(),
    }));
    let (white, _outside) = white_wiring(crate::white::WhiteMemoryLoad::NothingRemembered);
    let config = CameraEngineConfig::default();
    let wiring = ThreeAWiring::vendor(&config.three_a).with_white(white);
    let mut rig = Rig::over_fake_with_wiring(config, AppPresence::Foreground, fake, wiring);
    rig.engine.load_catalog(rig.now);

    assert_eq!(
        rig.send(take()),
        CommandOutcome::InvalidState(InvalidStateReason::EngineFailed)
    );
    assert_eq!(
        report(&rig).map(|report| report.state),
        Some(WhiteTakeState::NotAsked {
            reason: InvalidStateReason::EngineFailed
        })
    );
}

#[test]
fn a_refused_take_does_not_get_overwritten_by_the_older_requests_late_outcome() {
    let (mut rig, _outside) = rig_after_opening();
    rig.send(take());
    rig.send(CameraCommand::Close);
    // Потока нет, старая заявка ещё ждёт; новая команда отклонена.
    rig.send(take());
    let refused = rig.last_command_id();

    rig.advance(Duration::from_secs(4));

    assert_eq!(
        report(&rig),
        Some(WhiteTakeReport {
            command: refused,
            state: WhiteTakeState::NotAsked {
                reason: InvalidStateReason::NotStreaming
            }
        }),
        "срок старой заявки вышел, но последний отчёт — про последнюю команду"
    );
}

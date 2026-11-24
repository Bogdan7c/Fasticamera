//! Ответы источника белой и команда «взять белую»: все исходы, гонки, рука сильнее источника.

use camera_contract::white::{
    WhileAwaitingWhite, WhiteConfidence, WhiteOrigin, WhiteRequestId, WhiteSourceFailure, WhiteSourceId,
    WhiteSourceOutcome, WhiteTransition, WhiteUnavailableReason, WhiteUnknownReason,
};

use super::white_support::*;
use super::*;
use crate::white::{WhiteCancelCause, WhiteRequestOutcome, WhiteRequestResult, WhiteRequestState};

#[test]
fn a_ready_answer_applies_the_source_white_remembers_it_and_reports_both_events() {
    let (mut rig, outside) = white_rig_streaming();
    rig.frame_after(Duration::ZERO);
    rig.fresh_events();

    let confident = WhiteSourceOutcome::Ready {
        illuminant: camera_contract::white::SceneIlluminant::Uniform(setting(3_700.0, 0.012)),
        confidence: WhiteConfidence::from_fraction(0.8),
    };
    rig.deliver_answer(&outside, answer(1, confident));

    let held = rig.held();
    assert_eq!(held.setting, setting(3_700.0, 0.012));
    assert_eq!(held.origin, WhiteOrigin::Source(WhiteSourceId::PHONE));
    let events = rig.fresh_events();
    assert_eq!(white_changes(&events), vec![held]);
    assert_eq!(
        finished_requests(&events),
        vec![WhiteRequestResult {
            request: WhiteRequestId(1),
            source: WhiteSourceId::PHONE,
            result: WhiteRequestOutcome::Applied {
                confidence: WhiteConfidence::from_fraction(0.8)
            },
        }]
    );
    let saved = outside.saved();
    assert_eq!(saved.len(), 1, "память получила ровно одну запись");
    assert_eq!(saved[0].setting, setting(3_700.0, 0.012));
    assert_eq!(saved[0].origin, crate::white::RememberedOrigin::Source);
    assert_eq!(rig.white_status().memory.saves_requested, 1);
    // Для заявки открытия переход плавный: рендер сам сделает мгновенно, если кадры ещё скрыты.
    assert_eq!(
        outside.directive().transition,
        WhiteTransition::Over(Duration::from_millis(300))
    );
}

#[test]
fn every_refusal_of_the_source_keeps_the_white_and_stays_distinguishable() {
    let refusals = [
        (WhiteSourceOutcome::Busy, WhiteRequestOutcome::Busy),
        (
            WhiteSourceOutcome::Unknown {
                reason: WhiteUnknownReason::NoFreshMeasurement,
            },
            WhiteRequestOutcome::Unknown {
                reason: WhiteUnknownReason::NoFreshMeasurement,
            },
        ),
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
                reason: WhiteUnavailableReason::NotRegistered,
            },
            WhiteRequestOutcome::Unavailable {
                reason: WhiteUnavailableReason::NotRegistered,
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
    let mut seen = Vec::new();
    for (source_outcome, expected) in refusals {
        let (mut rig, outside) = white_rig_streaming();
        rig.frame_after(Duration::ZERO);
        let before = rig.held();
        let generation_before = outside.generation();
        rig.fresh_events();

        rig.deliver_answer(&outside, answer(1, source_outcome));

        let events = rig.fresh_events();
        let results = finished_requests(&events);
        assert_eq!(results.len(), 1, "{expected:?}");
        assert_eq!(results[0].result, expected);
        assert_eq!(rig.held(), before, "отказ белую не меняет: {expected:?}");
        assert!(white_changes(&events).is_empty());
        assert!(outside.saved().is_empty(), "отказ в память не пишется");
        assert_eq!(outside.directive().white, before.setting);
        assert_eq!(outside.directive().request, None, "заявка закрыта и снята с указания");
        assert_eq!(
            rig.white_status().request,
            WhiteRequestState::Idle { last: Some(results[0]) }
        );
        assert!(
            outside.generation() > generation_before,
            "рендер узнал, что заявка снята"
        );
        assert!(!seen.contains(&expected), "исходы различимы");
        seen.push(expected);
    }
    assert_eq!(seen.len(), 6);
}

#[test]
fn an_answer_with_a_foreign_number_is_dropped_and_the_wait_continues() {
    let (mut rig, outside) = white_rig_streaming();
    rig.frame_after(Duration::ZERO);
    let before = rig.held();

    rig.deliver_answer(&outside, ready(999, setting(2_500.0, 0.0)));

    assert_eq!(rig.held(), before);
    assert_eq!(rig.white_status().stale_answers, 1);
    assert!(
        matches!(rig.white_status().request, WhiteRequestState::Waiting { .. }),
        "ждём свой ответ"
    );
    // Свой ответ после чужого всё равно принимается.
    rig.deliver_answer(&outside, ready(1, setting(4_000.0, 0.0)));
    assert_eq!(rig.held().setting, setting(4_000.0, 0.0));
}

#[test]
fn an_answer_with_nothing_awaited_is_stale() {
    let (mut rig, outside) = white_rig();
    rig.deliver_answer(&outside, ready(1, setting(2_500.0, 0.0)));
    assert_eq!(rig.held().origin, WhiteOrigin::Fallback);
    assert_eq!(rig.white_status().stale_answers, 1);
}

#[test]
fn hand_beats_source_a_manual_command_during_the_wait_cancels_it_and_the_late_answer_is_dropped() {
    let (mut rig, outside) = white_rig_streaming();
    rig.frame_after(Duration::ZERO);
    rig.fresh_events();

    assert_eq!(
        rig.send(CameraCommand::SetWhite {
            setting: setting(5_000.0, 0.0),
            transition: WhiteTransition::Immediate
        }),
        CommandOutcome::Accepted
    );
    let events = rig.fresh_events();
    assert_eq!(
        finished_requests(&events)
            .iter()
            .map(|result| result.result)
            .collect::<Vec<_>>(),
        vec![WhiteRequestOutcome::Cancelled {
            by: WhiteCancelCause::ManualWhite
        }]
    );
    let directive = outside.directive();
    assert_eq!(directive.white, setting(5_000.0, 0.0));
    assert_eq!(directive.request, None, "рендеру велено прекратить спрашивать");
    assert_eq!(rig.engine.next_deadline(), None, "срок снят");

    // Телефон всё-таки ответил на отменённую заявку.
    rig.deliver_answer(&outside, ready(1, setting(3_000.0, 0.0)));
    assert_eq!(
        rig.held().setting,
        setting(5_000.0, 0.0),
        "опоздавший ответ не применён"
    );
    assert_eq!(rig.held().origin, WhiteOrigin::Manual);
    assert_eq!(rig.white_status().stale_answers, 1);
}

#[test]
fn the_same_manual_white_during_a_wait_still_cancels_the_wait() {
    let (mut rig, outside) = white_rig_streaming();
    rig.send(CameraCommand::EnterBackground);
    rig.send(CameraCommand::SetWhite {
        setting: setting(5_000.0, 0.0),
        transition: WhiteTransition::Immediate,
    });
    rig.send(CameraCommand::EnterForeground);
    // Рука сильнее: заявки при возврате нет. Заводим её командой и повторяем то же ручное значение.
    assert_eq!(outside.directive().request, None);
    rig.frame_after(Duration::ZERO);
    rig.send(CameraCommand::TakeWhite {
        source: WhiteSourceId::PHONE,
        transition: WhiteTransition::Immediate,
    });
    assert!(outside.directive().request.is_some());

    assert_eq!(
        rig.send(CameraCommand::SetWhite {
            setting: setting(5_000.0, 0.0),
            transition: WhiteTransition::Immediate
        }),
        CommandOutcome::Accepted,
        "то же значение, но заявка шла: это принятая команда, а не «уже так»"
    );
    assert_eq!(outside.directive().request, None);
}

#[test]
fn take_white_needs_a_live_stream_and_changes_nothing_without_one() {
    let (mut rig, outside) = white_rig();
    let generation = outside.generation();
    assert_eq!(
        rig.send(CameraCommand::TakeWhite {
            source: WhiteSourceId::PHONE,
            transition: WhiteTransition::Immediate
        }),
        CommandOutcome::InvalidState(crate::command::InvalidStateReason::NotStreaming)
    );
    assert_eq!(outside.generation(), generation);
    assert_eq!(rig.white_status().request, WhiteRequestState::Idle { last: None });
}

#[test]
fn take_white_keeps_frames_visible_and_applies_the_transition_the_command_asked_for() {
    let (mut rig, outside) = white_rig_streaming();
    rig.frame_after(Duration::ZERO);
    rig.deliver_answer(&outside, ready(1, setting(3_700.0, 0.0)));
    rig.fresh_events();

    assert_eq!(
        rig.send(CameraCommand::TakeWhite {
            source: WhiteSourceId::PHONE,
            transition: WhiteTransition::Over(Duration::from_secs(2))
        }),
        CommandOutcome::Accepted
    );
    let request = outside.directive().request.expect("заявка выдана");
    assert_eq!(request.id, WhiteRequestId(2));
    assert_eq!(
        request.while_awaiting,
        WhileAwaitingWhite::KeepShowing,
        "картинка видна, пока ждём"
    );
    // Кадры уже идут: срок считается от выдачи команды.
    assert_eq!(rig.engine.next_deadline(), Some(rig.now + Duration::from_secs(3)));

    rig.deliver_answer(&outside, ready(2, setting(6_500.0, -0.01)));
    assert_eq!(outside.directive().white, setting(6_500.0, -0.01));
    assert_eq!(
        outside.directive().transition,
        WhiteTransition::Over(Duration::from_secs(2))
    );
    assert_eq!(rig.held().origin, WhiteOrigin::Source(WhiteSourceId::PHONE));
}

#[test]
fn a_new_take_white_replaces_the_waiting_one_and_the_old_answer_is_foreign() {
    let (mut rig, outside) = white_rig_streaming();
    rig.frame_after(Duration::ZERO);
    rig.fresh_events();
    let take = CameraCommand::TakeWhite {
        source: WhiteSourceId::PHONE,
        transition: WhiteTransition::Immediate,
    };
    rig.send(take.clone());
    rig.send(take);

    let events = rig.fresh_events();
    let results = finished_requests(&events);
    assert_eq!(
        results
            .iter()
            .map(|result| (result.request.0, result.result))
            .collect::<Vec<_>>(),
        vec![
            (
                1,
                WhiteRequestOutcome::Cancelled {
                    by: WhiteCancelCause::ReplacedByNewRequest
                }
            ),
            (
                2,
                WhiteRequestOutcome::Cancelled {
                    by: WhiteCancelCause::ReplacedByNewRequest
                }
            )
        ]
    );
    assert_eq!(
        outside.directive().request.map(|request| request.id),
        Some(WhiteRequestId(3))
    );
    rig.deliver_answer(&outside, ready(2, setting(2_500.0, 0.0)));
    assert_eq!(rig.white_status().stale_answers, 1);
    assert_eq!(rig.held().origin, WhiteOrigin::Fallback);
}

#[test]
fn the_engine_does_not_judge_unknown_sources_the_renderer_answers_for_them() {
    let (mut rig, outside) = white_rig_streaming();
    rig.frame_after(Duration::ZERO);
    let neural = WhiteSourceId::new("neural");
    rig.send(CameraCommand::TakeWhite {
        source: neural,
        transition: WhiteTransition::Immediate,
    });
    assert_eq!(outside.directive().request.map(|request| request.source), Some(neural));

    rig.fresh_events();
    rig.deliver_answer(
        &outside,
        answer(
            2,
            WhiteSourceOutcome::Unavailable {
                reason: WhiteUnavailableReason::NotRegistered,
            },
        ),
    );
    let results = finished_requests(&rig.fresh_events());
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].source, neural);
    assert_eq!(
        results[0].result,
        WhiteRequestOutcome::Unavailable {
            reason: WhiteUnavailableReason::NotRegistered
        }
    );
}

#[test]
fn a_phone_answer_replaces_a_manual_white_when_the_user_asks_for_it() {
    let (mut rig, outside) = white_rig_streaming();
    rig.frame_after(Duration::ZERO);
    rig.send(CameraCommand::SetWhite {
        setting: setting(5_000.0, 0.0),
        transition: WhiteTransition::Immediate,
    });
    rig.send(CameraCommand::TakeWhite {
        source: WhiteSourceId::PHONE,
        transition: WhiteTransition::Immediate,
    });
    rig.deliver_answer(&outside, ready(2, setting(3_100.0, 0.0)));
    assert_eq!(
        rig.held().origin,
        WhiteOrigin::Source(WhiteSourceId::PHONE),
        "долгое нажатие возвращает телефон"
    );
    assert_eq!(rig.held().setting, setting(3_100.0, 0.0));
    let saved = outside.saved();
    assert_eq!(
        saved.last().map(|record| record.origin),
        Some(crate::white::RememberedOrigin::Source)
    );
}

#[test]
fn a_taken_white_without_a_new_stream_times_out_instead_of_waiting_for_the_next_opening() {
    // Команда «взять белую», ответа нет, камеру закрыли: без срока заявка дождалась бы следующего открытия и
    // перебила бы то, что человек выставил рукой.
    let (mut rig, outside) = white_rig_streaming();
    rig.frame_after(Duration::ZERO);
    rig.deliver_answer(&outside, ready(1, setting(3_700.0, 0.0)));
    rig.send(CameraCommand::TakeWhite {
        source: WhiteSourceId::PHONE,
        transition: WhiteTransition::Immediate,
    });
    let pressed = rig.now;
    rig.send(CameraCommand::Close);
    assert_eq!(
        rig.engine.next_deadline(),
        Some(pressed + Duration::from_secs(3)),
        "срок команды идёт от нажатия и переживает конец потока"
    );
    rig.fresh_events();
    rig.advance(Duration::from_secs(4));
    let results = finished_requests(&rig.fresh_events());
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].result, WhiteRequestOutcome::TimedOut);
    assert_eq!(outside.directive().request, None);
}

#[test]
fn a_taken_white_is_timed_from_the_command_even_before_the_first_frame() {
    let (mut rig, _outside) = white_rig_streaming();
    rig.send(CameraCommand::TakeWhite {
        source: WhiteSourceId::PHONE,
        transition: WhiteTransition::Immediate,
    });
    let pressed = rig.now;
    assert_eq!(rig.engine.next_deadline(), Some(pressed + Duration::from_secs(3)));
    // Первый кадр срок команды не сдвигает.
    rig.frame_after(Duration::from_secs(1));
    assert_eq!(rig.engine.next_deadline(), Some(pressed + Duration::from_secs(3)));
}

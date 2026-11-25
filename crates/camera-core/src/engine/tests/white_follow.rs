//! Слежение за белым источника (WB44–WB56): ход слежения, мёртвая зона и выключение — через команды, кадры и события
//! движка. Рендер заменён чтением указаний и ящиком ответов. Блокировка, поток, фон и память — `white_follow_life`.

use camera_contract::control::WhiteBalanceSetting;
use camera_contract::white::{
    WhileAwaitingWhite, WhiteAdjustment, WhiteOrigin, WhiteSourceOutcome, WhiteTransition, WhiteUnknownReason,
};

use super::white_follow_support::*;
use super::white_support::*;
use super::*;
use crate::white::{WhiteFollowRecord, WhiteFollowState, WhiteKeeping, WhiteRequestOutcome, WhiteTakeState};

// ---- Без «Auto» ничего не меняется ----

#[test]
fn without_auto_hundreds_of_frames_ask_nobody_and_publish_nothing() {
    let mut follow = FollowRig::streaming();
    let generation = follow.outside.generation();
    follow.frames(300);
    follow.lock();
    follow.frames(30);
    follow.unlock();
    follow.frames(30);

    assert_eq!(follow.outside.generation(), generation, "указание не публиковалось");
    assert_eq!(follow.state(), WhiteFollowState::Off);
    assert_eq!(follow.asked(), 0);
    assert_eq!(follow.rig.held(), phone(setting(4_000.0, 0.0)));
    assert!(white_changes(&follow.rig.fresh_events()).is_empty());
    assert!(follow.outside.saved_follows().is_empty());
}

#[test]
fn the_ae_af_lock_without_auto_does_not_cancel_the_opening_request_or_a_take() {
    let mut follow = FollowRig::over(white_rig_streaming());
    follow.frames(1);
    let opening = follow.request().expect("заявка открытия");
    follow.lock();
    follow.frames(1);
    assert_eq!(follow.request(), Some(opening), "открытие ждёт, как и раньше");
    follow.answer(setting(3_300.0, 0.0));
    assert_eq!(follow.rig.held(), phone(setting(3_300.0, 0.0)));

    follow.rig.send(CameraCommand::TakeWhite {
        source: PHONE,
        transition: WhiteTransition::Immediate,
    });
    follow.frames(1);
    follow.answer(setting(5_000.0, 0.0));
    assert_eq!(
        follow.rig.held(),
        phone(setting(5_000.0, 0.0)),
        "«взять» под блокировкой работает"
    );
}

// ---- Включение и ход слежения ----

#[test]
fn auto_asks_from_the_next_frame_without_hiding_frames_and_moves_the_white_smoothly() {
    let mut follow = FollowRig::streaming();
    assert_eq!(follow.follow(), CommandOutcome::Accepted);
    assert_eq!(follow.state(), FOLLOWING);
    assert_eq!(follow.request(), None, "команда сама источника не спрашивает");

    follow.frames(1);
    let request = follow.request().expect("первый кадр спросил");
    assert_eq!(request.source, PHONE);
    assert_eq!(request.while_awaiting, WhileAwaitingWhite::KeepShowing);
    follow.answer(setting(5_600.0, 0.004));

    let directive = follow.outside.directive();
    assert_eq!(directive.white, setting(5_600.0, 0.004));
    assert_eq!(directive.transition, WhiteTransition::Over(Duration::from_millis(300)));
    assert_eq!(directive.request, None);
    assert_eq!(follow.rig.held(), phone(setting(5_600.0, 0.004)));
    assert_eq!(
        white_changes(&follow.rig.fresh_events()),
        vec![phone(setting(5_600.0, 0.004))]
    );
}

#[test]
fn a_second_auto_tap_is_already_in_state_and_saves_nothing_more() {
    let mut follow = FollowRig::following();
    let generation = follow.outside.generation();
    assert_eq!(follow.follow(), CommandOutcome::AlreadyInState);
    assert_eq!(follow.outside.generation(), generation);
    assert_eq!(follow.outside.saved_follows(), vec![followed()]);
    assert!(follow_changes(&follow.rig.fresh_events()).is_empty());
}

#[test]
fn the_source_is_asked_by_frame_timestamps_no_more_often_than_the_interval() {
    let mut follow = FollowRig::following();
    follow.frames(1);
    follow.answer(setting(4_000.0, 0.0));
    assert_eq!(follow.asked(), 1);

    // Метка кадра стоит — часы движка идут: спрашивать незачем, света не прибавилось.
    follow.rig.now += Duration::from_secs(10);
    follow.rig.engine.on_timer(follow.rig.now);
    assert_eq!(
        follow.rig.engine.next_deadline(),
        None,
        "слежение поток движка не будит"
    );

    follow.frames(ASK_EVERY_FRAMES - 1);
    assert_eq!(follow.request(), None, "рано");
    follow.frames(1);
    assert!(follow.request().is_some(), "ровно через период");
    assert_eq!(follow.asked(), 2);
    // Пока ответа нет, второй раз не спрашиваем.
    let id = follow.request().map(|request| request.id);
    follow.frames(ASK_EVERY_FRAMES * 3);
    assert_eq!(follow.request().map(|request| request.id), id);
}

#[test]
fn a_silent_source_is_asked_again_after_the_answer_timeout_and_the_white_stays() {
    let mut follow = FollowRig::following();
    follow.frames(1);
    let first = follow.request().expect("спросили");
    // Срок ответа 3 с — 180 кадров.
    follow.frames(179);
    assert_eq!(follow.request(), Some(first), "срок ещё не вышел");
    follow.frames(1);
    let second = follow.request().expect("спросили заново");
    assert!(second.id > first.id);
    assert_eq!(
        follow.rig.white_status().follow.last_refusal,
        Some(WhiteRequestOutcome::TimedOut)
    );
    // Опоздавший ответ на первый вопрос — чужой.
    follow
        .rig
        .deliver_answer(&follow.outside, ready(first.id.0, setting(7_000.0, 0.0)));
    assert_eq!(follow.rig.held(), phone(setting(4_000.0, 0.0)));
    assert_eq!(follow.rig.white_status().stale_answers, 1);
}

#[test]
fn a_refusal_keeps_the_white_and_the_next_question_comes_by_the_pace() {
    let mut follow = FollowRig::following();
    follow.frames(1);
    let generation_white = follow.outside.directive().white;
    follow.refuse(WhiteSourceOutcome::Unknown {
        reason: WhiteUnknownReason::NoFreshMeasurement,
    });
    let status = follow.rig.white_status();
    assert_eq!(status.held, phone(setting(4_000.0, 0.0)));
    assert_eq!(follow.outside.directive().white, generation_white);
    assert_eq!(
        status.follow.last_refusal,
        Some(WhiteRequestOutcome::Unknown {
            reason: WhiteUnknownReason::NoFreshMeasurement
        })
    );
    assert_eq!(follow.state(), FOLLOWING, "отказ слежения не выключает");

    follow.round(setting(3_000.0, 0.0));
    let status = follow.rig.white_status();
    assert_eq!(status.held, phone(setting(3_000.0, 0.0)));
    assert_eq!(status.follow.last_refusal, None, "ответ снял отказ");
}

#[test]
fn the_questions_are_invisible_outside_no_request_no_take_report_no_events() {
    let mut follow = FollowRig::following();
    let before = follow.rig.white_status();
    follow.frames(1);
    assert!(follow.request().is_some(), "вопрос у рендера есть");
    let waiting = follow.rig.white_status();
    assert_eq!(waiting.request, before.request, "снаружи «ждём ответа» не видно");
    assert_eq!(waiting.last_take, None);

    for kelvin in [4_100.0, 5_000.0, 3_000.0] {
        follow.round(setting(kelvin, 0.0));
    }
    follow.refuse_next();
    let after = follow.rig.white_status();
    assert_eq!(after.request, before.request, "итог прошлой (настоящей) заявки цел");
    assert_eq!(after.last_take, None);
    let events = follow.rig.fresh_events();
    assert!(
        finished_requests(&events).is_empty(),
        "события «заявка закрыта» нет: {events:?}"
    );
    assert_eq!(white_changes(&events).len(), 3, "событий столько, сколько переездов");
}

// ---- Мёртвая зона ----

#[test]
fn jitter_inside_the_dead_zone_changes_nothing_and_a_step_outside_moves_exactly_to_the_answer() {
    let mut follow = FollowRig::following();
    // 4000 К = 250 майред; 4,9 майреда и 0,0019 Duv — внутри зоны по обеим осям, в обе стороны.
    let inside = [(245.1, 0.0019), (254.9, -0.0019), (250.0, 0.0)];
    for (mired, duv) in inside {
        follow.round(setting(1.0e6 / mired, duv));
        assert_eq!(follow.rig.held(), phone(setting(4_000.0, 0.0)), "{mired} / {duv}");
        assert_eq!(follow.outside.directive().white, setting(4_000.0, 0.0));
    }
    assert!(white_changes(&follow.rig.fresh_events()).is_empty());
    assert_eq!(follow.rig.white_status().follow.moved, 0);
    assert_eq!(follow.outside.saved().len(), 1, "память писалась только открытием");

    // Шаг только по температуре, потом только по оттенку.
    let warmer = setting(1.0e6 / 255.1, 0.0);
    follow.round(warmer);
    assert_eq!(follow.rig.held(), phone(warmer));
    let greener = WhiteBalanceSetting {
        tint: setting(4_000.0, 0.0021).tint,
        ..warmer
    };
    follow.round(greener);
    assert_eq!(follow.rig.held(), phone(greener));
    assert_eq!(follow.rig.white_status().follow.moved, 2);
}

#[test]
fn slow_drift_is_measured_from_the_held_white_so_it_cannot_creep_away_unnoticed() {
    let mut follow = FollowRig::following();
    // По 2 майреда за вопрос: каждый шаг мал, но от действующей белой набегает больше зоны.
    for step in 1..=3 {
        follow.round(setting(1.0e6 / (250.0 + 2.0 * f64::from(step)), 0.0));
    }
    assert_eq!(follow.rig.held(), phone(setting(1.0e6 / 256.0, 0.0)));
}

#[test]
fn the_first_answer_after_a_manual_white_is_taken_even_when_the_numbers_are_close() {
    let mut follow = FollowRig::streaming();
    follow.rig.send(CameraCommand::SetWhite {
        setting: setting(5_600.0, 0.003),
        transition: WhiteTransition::Immediate,
    });
    follow.follow();
    assert_eq!(follow.rig.held().origin, WhiteOrigin::Manual, "до ответа белая прежняя");
    follow.round(setting(5_610.0, 0.003));
    assert_eq!(follow.rig.held(), phone(setting(5_610.0, 0.003)));
}

#[test]
fn an_answer_beyond_the_limits_is_clamped_once_and_then_rests() {
    let mut follow = FollowRig::following();
    for _ in 0..3 {
        follow.round(setting(20_000.0, 0.0));
    }
    assert_eq!(follow.rig.held(), phone(setting(12_000.0, 0.0)));
    let adjusted = follow
        .rig
        .fresh_events()
        .iter()
        .filter(|event| matches!(event, CameraEvent::ControlsAdjusted { .. }))
        .count();
    assert_eq!(adjusted, 1, "прижим сообщён один раз, а не на каждый вопрос");
}

// ---- Выключение ----

/// Слежение выключено действием `turn_off`; вопрос, висевший у источника, снят, и поздний ответ белую не трогает.
fn assert_turned_off_by(turn_off: CameraCommand) {
    let mut follow = FollowRig::following();
    follow.frames(1);
    let asked = follow.request().expect("вопрос висит").id.0;
    assert_eq!(
        follow.rig.send(turn_off.clone()),
        CommandOutcome::Accepted,
        "{turn_off:?}"
    );
    assert_eq!(follow.state(), WhiteFollowState::Off, "{turn_off:?}");
    let held = follow.rig.held();
    follow
        .rig
        .deliver_answer(&follow.outside, ready(asked, setting(9_000.0, 0.0)));
    assert_eq!(follow.rig.held(), held, "{turn_off:?}: поздний ответ слежения чужой");
    assert_eq!(
        follow.outside.saved_follows().last(),
        Some(&WhiteFollowRecord::default()),
        "{turn_off:?}: выключение запомнено"
    );
    assert_eq!(
        follow_changes(&follow.rig.fresh_events()),
        vec![WhiteKeeping::Hold],
        "{turn_off:?}"
    );
    let asked_total = follow.asked();
    follow.frames(60);
    assert_eq!(follow.asked(), asked_total, "{turn_off:?}: больше не спрашиваем");
}

#[test]
fn every_human_action_with_the_white_turns_auto_off() {
    assert_turned_off_by(CameraCommand::SetWhite {
        setting: setting(5_600.0, 0.003),
        transition: WhiteTransition::Immediate,
    });
    assert_turned_off_by(CameraCommand::AdjustWhite {
        adjustment: WhiteAdjustment::Temperature(setting(4_200.0, 0.0).temperature),
        transition: WhiteTransition::Immediate,
    });
    assert_turned_off_by(CameraCommand::TakeWhite {
        source: PHONE,
        transition: WhiteTransition::Immediate,
    });
}

#[test]
fn the_same_manual_white_again_still_turns_auto_off() {
    let mut follow = FollowRig::streaming();
    let manual = CameraCommand::SetWhite {
        setting: setting(5_600.0, 0.003),
        transition: WhiteTransition::Immediate,
    };
    follow.rig.send(manual.clone());
    follow.follow();
    assert_eq!(follow.rig.send(manual), CommandOutcome::Accepted);
    assert_eq!(follow.state(), WhiteFollowState::Off);
    assert_eq!(follow.rig.held().origin, WhiteOrigin::Manual);
}

#[test]
fn a_take_that_fails_leaves_auto_off_and_the_white_as_it_was() {
    let mut follow = FollowRig::following();
    follow.round(setting(3_000.0, 0.0));
    follow.rig.send(CameraCommand::TakeWhite {
        source: PHONE,
        transition: WhiteTransition::Immediate,
    });
    follow.refuse(WhiteSourceOutcome::Busy);
    assert_eq!(follow.state(), WhiteFollowState::Off);
    assert_eq!(follow.rig.held(), phone(setting(3_000.0, 0.0)));
    assert_eq!(
        follow.rig.white_status().last_take.map(|take| take.state),
        Some(WhiteTakeState::Finished(WhiteRequestOutcome::Busy))
    );
}

#[test]
fn a_take_rejected_without_a_stream_does_not_turn_auto_off() {
    let mut follow = FollowRig::following();
    follow.rig.send(CameraCommand::StopStreaming);
    assert!(matches!(
        follow.rig.send(CameraCommand::TakeWhite {
            source: PHONE,
            transition: WhiteTransition::Immediate,
        }),
        CommandOutcome::InvalidState(_)
    ));
    assert_eq!(follow.state(), FOLLOWING, "просьбу не приняли — она ничего не изменила");
}

#[test]
fn auto_replaces_a_waiting_take_and_its_report_is_closed_as_cancelled() {
    let mut follow = FollowRig::streaming();
    follow.rig.send(CameraCommand::TakeWhite {
        source: PHONE,
        transition: WhiteTransition::Immediate,
    });
    follow.follow();
    assert!(matches!(
        follow.rig.white_status().last_take.map(|take| take.state),
        Some(WhiteTakeState::Finished(WhiteRequestOutcome::Cancelled { .. }))
    ));
    follow.round(setting(3_000.0, 0.0));
    assert_eq!(follow.state(), FOLLOWING);
    assert_eq!(follow.rig.held(), phone(setting(3_000.0, 0.0)));
}

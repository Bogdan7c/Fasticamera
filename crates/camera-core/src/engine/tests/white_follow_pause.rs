//! Пауза слежения за белым на время секции CALIBRATE (14n, решение 289): второй замок рядом с блокировкой AE/AF.
//! Пауза молчит слежение, но не выключает «Auto» и не пишет в память; руку она не останавливает.

use camera_contract::white::{WhileAwaitingWhite, WhiteTransition};

use super::white_follow_support::*;
use super::white_support::*;
use super::*;
use crate::white::{WhiteCancelCause, WhiteFollowPause, WhiteFollowState, WhiteRequestOutcome, WhiteRequestState};

const PAUSED: WhiteFollowState = WhiteFollowState::Paused { source: PHONE };

fn set_pause(follow: &mut FollowRig, pause: WhiteFollowPause) -> CommandOutcome {
    follow.rig.send(CameraCommand::SetWhiteFollowPause { pause })
}

#[test]
fn a_paused_auto_asks_nobody_for_hundreds_of_frames_and_unpausing_moves_the_white_to_the_answer() {
    let mut follow = FollowRig::following();
    follow.round(setting(3_000.0, 0.0));
    assert_eq!(
        set_pause(&mut follow, WhiteFollowPause::Paused),
        CommandOutcome::Accepted
    );
    assert_eq!(follow.state(), PAUSED);
    let (generation, asked) = (follow.outside.generation(), follow.asked());

    follow.frames(300);
    assert_eq!(follow.request(), None, "на паузе рендер вопроса не видит");
    assert_eq!(follow.asked(), asked, "ни одного вопроса источнику");
    assert_eq!(follow.outside.generation(), generation, "ни одного нового указания");
    assert_eq!(follow.rig.held(), phone(setting(3_000.0, 0.0)));

    assert_eq!(
        set_pause(&mut follow, WhiteFollowPause::Running),
        CommandOutcome::Accepted
    );
    assert_eq!(follow.state(), FOLLOWING);
    follow.frames(1);
    follow.answer(setting(5_000.0, 0.0));
    assert_eq!(
        follow.rig.held(),
        phone(setting(5_000.0, 0.0)),
        "после паузы слежение спрашивает с ближайшего кадра и белая едет за ответом"
    );
}

#[test]
fn a_pause_is_level_triggered_and_repeats_are_already_in_state() {
    let mut follow = FollowRig::following();
    assert_eq!(
        set_pause(&mut follow, WhiteFollowPause::Running),
        CommandOutcome::AlreadyInState,
        "пауза не стояла: снимать нечего"
    );
    assert_eq!(
        set_pause(&mut follow, WhiteFollowPause::Paused),
        CommandOutcome::Accepted
    );
    assert_eq!(
        set_pause(&mut follow, WhiteFollowPause::Paused),
        CommandOutcome::AlreadyInState
    );
    assert_eq!(
        set_pause(&mut follow, WhiteFollowPause::Running),
        CommandOutcome::Accepted
    );
    assert_eq!(
        set_pause(&mut follow, WhiteFollowPause::Running),
        CommandOutcome::AlreadyInState
    );
}

#[test]
fn a_pause_does_not_turn_auto_off_and_writes_nothing_to_the_memory_or_the_events() {
    let mut follow = FollowRig::following();
    let saved = (follow.outside.saved().len(), follow.outside.saved_follows().len());
    set_pause(&mut follow, WhiteFollowPause::Paused);
    follow.frames(30);
    set_pause(&mut follow, WhiteFollowPause::Running);
    assert_eq!(follow.state(), FOLLOWING, "режим «Auto» остался включённым");
    assert_eq!(
        (follow.outside.saved().len(), follow.outside.saved_follows().len()),
        saved,
        "память не тронута"
    );
    let events = follow.rig.fresh_events();
    assert!(
        follow_changes(&events).is_empty(),
        "события «слежение изменилось» нет: {events:?}"
    );
}

#[test]
fn a_pause_set_without_auto_changes_nothing_until_auto_is_turned_on() {
    let mut follow = FollowRig::streaming();
    assert_eq!(
        set_pause(&mut follow, WhiteFollowPause::Paused),
        CommandOutcome::Accepted
    );
    assert_eq!(
        follow.state(),
        WhiteFollowState::Off,
        "без «Auto» слежения нет, а значит и паузы"
    );
    follow.follow();
    assert_eq!(follow.state(), PAUSED, "включённый на паузе «Auto» сразу молчит");
    follow.frames(60);
    assert_eq!(follow.asked(), 0);
}

#[test]
fn a_question_waiting_when_the_pause_lands_is_dropped_and_a_late_answer_does_not_move_the_white() {
    let mut follow = FollowRig::following();
    follow.frames(1);
    let asked = follow.request().expect("вопрос висит").id.0;
    set_pause(&mut follow, WhiteFollowPause::Paused);
    assert_eq!(follow.request(), None, "вопрос снят сразу");
    follow
        .rig
        .deliver_answer(&follow.outside, ready(asked, setting(9_000.0, 0.0)));
    assert_eq!(
        follow.rig.held(),
        phone(setting(4_000.0, 0.0)),
        "запоздалый ответ чужой"
    );
}

#[test]
fn an_opening_request_waiting_when_the_pause_lands_is_cancelled_by_the_pause_cause() {
    let mut follow = FollowRig::following();
    follow.round(setting(3_000.0, 0.0));
    follow.rig.send(CameraCommand::EnterBackground);
    follow.rig.send(CameraCommand::EnterForeground);
    let opening = follow
        .request()
        .expect("возврат с «Auto» открывает заявку и прячет кадры");
    assert_eq!(opening.while_awaiting, WhileAwaitingWhite::HideFrames);

    set_pause(&mut follow, WhiteFollowPause::Paused);
    assert_eq!(follow.request(), None, "заявку снимают, источник её больше не видит");
    match follow.rig.white_status().request {
        WhiteRequestState::Idle { last: Some(result) } => assert_eq!(
            result.result,
            WhiteRequestOutcome::Cancelled {
                by: WhiteCancelCause::FollowPaused
            }
        ),
        other => panic!("ждали закрытую заявку, а вышло {other:?}"),
    }
    assert_eq!(follow.rig.held(), phone(setting(3_000.0, 0.0)), "белая цела");
}

#[test]
fn the_pause_and_the_ae_af_lock_are_independent_locks() {
    let mut follow = FollowRig::following();
    follow.round(setting(3_000.0, 0.0));
    follow.lock();
    assert_eq!(follow.state(), HELD_BY_LOCK);
    set_pause(&mut follow, WhiteFollowPause::Paused);
    assert_eq!(follow.state(), PAUSED, "при обоих замках показывается пауза");

    follow.unlock();
    follow.frames(60);
    assert_eq!(follow.state(), PAUSED, "блокировку сняли, пауза держит");
    assert_eq!(follow.request(), None);
    assert_eq!(follow.rig.held(), phone(setting(3_000.0, 0.0)));

    follow.lock();
    set_pause(&mut follow, WhiteFollowPause::Running);
    assert_eq!(follow.state(), HELD_BY_LOCK, "паузу сняли, блокировка держит");
    follow.frames(60);
    assert_eq!(follow.request(), None);

    follow.unlock();
    follow.frames(1);
    assert!(follow.request().is_some(), "оба замка сняты: слежение спрашивает");
}

#[test]
fn a_manual_white_under_the_pause_still_works_and_turns_auto_off() {
    let mut follow = FollowRig::following();
    set_pause(&mut follow, WhiteFollowPause::Paused);
    follow.rig.send(CameraCommand::SetWhite {
        setting: setting(3_200.0, 0.0),
        transition: WhiteTransition::Immediate,
    });
    assert_eq!(follow.rig.held().setting, setting(3_200.0, 0.0));
    assert_eq!(follow.state(), WhiteFollowState::Off);
}

#[test]
fn the_pause_survives_the_background_and_the_foreground_asks_nobody() {
    let mut follow = FollowRig::following();
    follow.round(setting(3_000.0, 0.0));
    set_pause(&mut follow, WhiteFollowPause::Paused);
    follow.rig.send(CameraCommand::EnterBackground);
    let generation = follow.outside.generation();
    follow.rig.send(CameraCommand::EnterForeground);
    assert_eq!(follow.request(), None, "возврат на паузе не открывает заявку");
    follow.frames(60);
    assert_eq!(follow.request(), None);
    assert_eq!(follow.outside.generation(), generation, "кадры не скрыты, белая цела");
    assert_eq!(follow.state(), PAUSED);
    assert_eq!(follow.rig.held(), phone(setting(3_000.0, 0.0)));
    set_pause(&mut follow, WhiteFollowPause::Running);
    follow.frames(1);
    assert!(follow.request().is_some());
}

#[test]
fn the_pause_is_accepted_in_the_background_and_without_a_stream() {
    let mut follow = FollowRig::following();
    follow.rig.send(CameraCommand::EnterBackground);
    assert_eq!(
        set_pause(&mut follow, WhiteFollowPause::Paused),
        CommandOutcome::Accepted
    );
    assert_eq!(follow.state(), PAUSED);
}

#[test]
fn the_pause_survives_a_lens_switch() {
    let mut follow = FollowRig::following();
    follow.round(setting(3_000.0, 0.0));
    set_pause(&mut follow, WhiteFollowPause::Paused);
    follow.rig.send(CameraCommand::SwitchTo {
        role: camera_contract::ids::LensRole::UltraWide,
    });
    follow.frames(60);
    assert_eq!(follow.state(), PAUSED);
    assert_eq!(follow.request(), None, "новая камера на паузе тоже не спрашивается");
    assert_eq!(follow.rig.held(), phone(setting(3_000.0, 0.0)));
}

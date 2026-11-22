//! Миниатюра в редьюсере (14g): картинка ставится только итогом «сохранён» своего снимка, порядок решает
//! приход итогов, проваленный снимок прежнюю картинку не трогает, без картинки — серый диск.

use crate::action::ScreenAction;
use crate::effect::Effect;
use crate::outcome::{ActionOutcome, Rejection};
use crate::shutter::{CaptureFailureReason, CaptureOutcome, ShotRequestId, ThumbnailState};
use crate::test_support::{Screen, at_ms, test_picture};

fn id(number: u64) -> ShotRequestId {
    ShotRequestId(number)
}

/// Нажимает затвор и берёт кадр: снимок ждёт сохранения.
fn shoot(screen: &mut Screen, at: i64) -> crate::shutter::ShotRequest {
    let transition = screen.step(ScreenAction::ShutterTapped { at: at_ms(at) });
    let Some(Effect::Capture(shot)) = transition.effects.first().cloned() else {
        panic!("нажатие должно дать снимок: {:?}", transition.outcome);
    };
    screen.step(ScreenAction::CaptureSecured { request: shot.request });
    shot
}

#[test]
fn saved_with_a_picture_puts_exactly_that_picture_into_the_thumbnail() {
    let mut screen = Screen::ready();
    let shot = shoot(&mut screen, 0);
    let picture = test_picture(10);

    let transition = screen.finish_saved_with_picture(shot.request, picture.clone());

    assert_eq!(transition.outcome, ActionOutcome::Applied);
    assert_eq!(
        screen.state.thumbnail(),
        ThumbnailState::Last {
            shot: shot.request,
            picture: Some(picture)
        }
    );
}

#[test]
fn saved_without_a_picture_gives_the_grey_disc_state_and_counts_it() {
    let mut screen = Screen::ready();
    let shot = shoot(&mut screen, 0);

    screen.step(ScreenAction::CaptureFinished {
        request: shot.request,
        outcome: CaptureOutcome::Saved,
    });

    assert_eq!(
        screen.state.thumbnail(),
        ThumbnailState::Last {
            shot: shot.request,
            picture: None
        }
    );
    assert_eq!(screen.state.thumbnail_counters().saved_without_picture, 1);
}

#[test]
fn a_failed_shot_keeps_the_previous_picture_and_does_not_start_a_fade() {
    let mut screen = Screen::ready();
    let first = shoot(&mut screen, 0);
    let picture = test_picture(10);
    screen.finish_saved_with_picture(first.request, picture.clone());
    screen.step(ScreenAction::Tick(at_ms(100)));
    screen.step(ScreenAction::Tick(at_ms(400)));
    let second = shoot(&mut screen, 500);

    screen.step(ScreenAction::CaptureFinished {
        request: second.request,
        outcome: CaptureOutcome::Failed(CaptureFailureReason::Processing),
    });

    assert_eq!(
        screen.state.thumbnail(),
        ThumbnailState::Last {
            shot: first.request,
            picture: Some(picture)
        },
        "картинка прежнего снимка осталась"
    );
    let blend = screen.state.thumbnail_blend(at_ms(600));
    assert_eq!((blend.underlay.is_none(), blend.overlay_opacity), (true, 1.0));
}

#[test]
fn the_picture_belongs_to_the_shot_whose_saved_arrived_last_not_the_one_pressed_last() {
    let mut screen = Screen::ready();
    let first = shoot(&mut screen, 0);
    let second = shoot(&mut screen, 100);
    let first_picture = test_picture(10);
    let second_picture = test_picture(20);

    // Итоги в обратном порядке: второй сохранился раньше первого.
    screen.finish_saved_with_picture(second.request, second_picture);
    screen.finish_saved_with_picture(first.request, first_picture.clone());

    assert_eq!(
        screen.state.thumbnail(),
        ThumbnailState::Last {
            shot: id(1),
            picture: Some(first_picture)
        },
        "последним сохранился первый снимок — его картинка в миниатюре"
    );
}

#[test]
fn a_stale_saved_with_a_picture_is_rejected_and_the_picture_is_not_shown() {
    let mut screen = Screen::ready();

    let transition = screen.finish_saved_with_picture(id(77), test_picture(1));

    assert_eq!(
        transition.outcome,
        ActionOutcome::Rejected(Rejection::StaleCaptureResult)
    );
    assert_eq!(screen.state.thumbnail(), ThumbnailState::Empty);
    assert_eq!(screen.state.thumbnail_counters().pictures_rejected_with_stale_result, 1);
}

#[test]
fn the_press_moment_of_the_shot_reaches_the_timing_and_the_first_drawn_frame_closes_it() {
    let mut screen = Screen::ready();
    let shot = shoot(&mut screen, 250);
    screen.finish_saved_with_picture(shot.request, test_picture(10));

    screen.step(ScreenAction::Tick(at_ms(580)));

    let timing = screen.state.picture_timing().expect("замер есть");
    assert_eq!(timing.pressed_at, at_ms(250));
    assert_eq!(timing.press_to_picture_ms(), Some(330.0));
}

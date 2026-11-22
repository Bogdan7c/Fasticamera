//! Тесты трекера миниатюры: растворение по времени с точными числами, защёлка старта на первом кадре,
//! «две картинки максимум», замер «нажатие → картинка» и учёт потерь.

use crate::shutter::{ShotRequestId, ThumbnailState};
use crate::test_support::{at_ms, test_picture};
use crate::thumbnail::ThumbnailTracker;

const FADE_MS: u32 = 150;

fn tracker() -> ThumbnailTracker {
    ThumbnailTracker::new(FADE_MS, 4)
}

fn id(number: u64) -> ShotRequestId {
    ShotRequestId(number)
}

/// Принимает итог снимка с картинкой `seed` и возвращает новое состояние миниатюры.
fn save(
    tracker: &mut ThumbnailTracker,
    current: &ThumbnailState,
    request: u64,
    seed: u8,
) -> (ThumbnailState, ui_draw::picture::UiPicture) {
    let picture = test_picture(seed);
    tracker.stage_incoming(id(request), picture.clone());
    (tracker.on_saved(id(request), current), picture)
}

#[test]
fn the_first_picture_fades_in_over_the_glass_with_exact_fractions_and_no_underlay() {
    let mut tracker = tracker();
    let (state, picture) = save(&mut tracker, &ThumbnailState::Empty, 1, 10);
    assert_eq!(
        state,
        ThumbnailState::Last {
            shot: id(1),
            picture: Some(picture)
        }
    );
    // Кадра ещё не было: картинка прозрачна, под ней ничего.
    let waiting = tracker.blend(at_ms(0));
    assert_eq!((waiting.underlay.is_none(), waiting.overlay_opacity), (true, 0.0));

    // Первый кадр: старт защёлкнут по его времени (600 000 мс), а не по моменту прихода итога.
    tracker.on_tick(at_ms(600_000));
    let fractions: Vec<f32> = [0, 75, 150]
        .into_iter()
        .map(|elapsed| tracker.blend(at_ms(600_000 + elapsed)).overlay_opacity)
        .collect();
    assert_eq!(fractions, [0.0, 0.5, 1.0], "начало, середина и конец 150 мс");
    assert!(
        tracker.blend(at_ms(600_075)).underlay.is_none(),
        "первая картинка: под ней стекло"
    );
}

#[test]
fn a_tick_after_the_end_settles_the_fade_and_later_ticks_do_not_restart_it() {
    let mut tracker = tracker();
    let (_state, _picture) = save(&mut tracker, &ThumbnailState::Empty, 1, 10);
    tracker.on_tick(at_ms(1_000));
    tracker.on_tick(at_ms(1_150));
    tracker.on_tick(at_ms(5_000));
    let settled = tracker.blend(at_ms(5_000));
    assert_eq!((settled.underlay.is_none(), settled.overlay_opacity), (true, 1.0));
    assert_eq!(tracker.counters().pictures_shown, 1, "картинка показана один раз");
}

#[test]
fn a_new_picture_dissolves_from_the_old_one_which_stays_fully_underneath() {
    let mut tracker = tracker();
    let (first_state, first) = save(&mut tracker, &ThumbnailState::Empty, 1, 10);
    tracker.on_tick(at_ms(0));
    tracker.on_tick(at_ms(200));
    let (second_state, second) = save(&mut tracker, &first_state, 2, 20);
    assert_eq!(
        second_state,
        ThumbnailState::Last {
            shot: id(2),
            picture: Some(second)
        }
    );
    tracker.on_tick(at_ms(1_000));
    let middle = tracker.blend(at_ms(1_075));
    assert_eq!(middle.underlay, Some(&first), "под новой лежит прежняя картинка");
    assert_eq!(middle.overlay_opacity, 0.5);
}

#[test]
fn a_new_saved_during_a_fade_makes_the_fading_in_picture_the_underlay_and_restarts_the_fade() {
    let mut tracker = tracker();
    let (first_state, _first) = save(&mut tracker, &ThumbnailState::Empty, 1, 10);
    tracker.on_tick(at_ms(0));
    tracker.on_tick(at_ms(200));
    let (second_state, second) = save(&mut tracker, &first_state, 2, 20);
    tracker.on_tick(at_ms(1_000));
    assert_eq!(tracker.blend(at_ms(1_075)).overlay_opacity, 0.5, "вторая на полпути");

    // Третья приходит посреди растворения второй: «старой» становится ВТОРАЯ (скачок осознан), отсчёт заново.
    let (_third_state, third) = save(&mut tracker, &second_state, 3, 30);
    tracker.on_tick(at_ms(1_080));
    let restarted = tracker.blend(at_ms(1_080));
    assert_eq!(restarted.underlay, Some(&second));
    assert_eq!(restarted.overlay_opacity, 0.0);
    let later = tracker.blend(at_ms(1_080 + 150));
    assert_eq!((later.underlay.is_none(), later.overlay_opacity), (true, 1.0));
    assert_ne!(second, third);
}

#[test]
fn a_picture_that_was_never_drawn_is_not_used_as_the_underlay() {
    let mut tracker = tracker();
    let (first_state, first) = save(&mut tracker, &ThumbnailState::Empty, 1, 10);
    tracker.on_tick(at_ms(0));
    tracker.on_tick(at_ms(300));
    // Два итога подряд, пока экрана на поверхности не было: второй ни разу не рисовался.
    let (second_state, _second) = save(&mut tracker, &first_state, 2, 20);
    let (_third_state, _third) = save(&mut tracker, &second_state, 3, 30);
    tracker.on_tick(at_ms(900_000));
    assert_eq!(
        tracker.blend(at_ms(900_000)).underlay,
        Some(&first),
        "растворяемся из той, что пользователь видел"
    );
}

#[test]
fn saved_without_a_picture_gives_the_grey_disc_state_counts_it_and_ends_any_fade() {
    let mut tracker = tracker();
    let (first_state, _first) = save(&mut tracker, &ThumbnailState::Empty, 1, 10);
    tracker.on_tick(at_ms(0));

    let state = tracker.on_saved(id(2), &first_state);

    assert_eq!(
        state,
        ThumbnailState::Last {
            shot: id(2),
            picture: None
        }
    );
    assert_eq!(tracker.counters().saved_without_picture, 1);
    assert!(tracker.blend(at_ms(10)).underlay.is_none());
    assert_eq!(tracker.timing(), None);
}

#[test]
fn a_picture_staged_for_another_shot_is_not_shown_and_is_counted() {
    let mut tracker = tracker();
    tracker.stage_incoming(id(5), test_picture(1));

    let state = tracker.on_saved(id(6), &ThumbnailState::Empty);

    assert_eq!(
        state,
        ThumbnailState::Last {
            shot: id(6),
            picture: None
        }
    );
    assert_eq!(tracker.counters().pictures_rejected_with_stale_result, 1);
    assert!(!tracker.discard_incoming(), "ящик после итога пуст");
}

#[test]
fn leftover_picture_after_a_rejected_result_is_discarded_with_a_count() {
    let mut tracker = tracker();
    tracker.stage_incoming(id(9), test_picture(1));
    assert!(tracker.discard_incoming());
    assert!(!tracker.discard_incoming());
    assert_eq!(tracker.counters().pictures_rejected_with_stale_result, 1);
}

#[test]
fn press_to_picture_is_measured_from_the_press_to_the_first_drawn_frame() {
    let mut tracker = tracker();
    tracker.note_press(id(1), at_ms(1_000));
    let (_state, _picture) = save(&mut tracker, &ThumbnailState::Empty, 1, 10);
    let before_draw = tracker.timing().expect("нажатие известно");
    assert_eq!(
        before_draw.press_to_picture_ms(),
        None,
        "пока не нарисована, времени нет"
    );

    tracker.on_tick(at_ms(1_330));

    let timing = tracker.timing().expect("времена");
    assert_eq!(
        (timing.shot, timing.pressed_at, timing.shown_at),
        (id(1), at_ms(1_000), Some(at_ms(1_330)))
    );
    assert_eq!(timing.press_to_picture_ms(), Some(330.0));
}

#[test]
fn press_log_is_bounded_and_counts_forgotten_presses() {
    let mut tracker = ThumbnailTracker::new(FADE_MS, 2);
    tracker.note_press(id(1), at_ms(10));
    tracker.note_press(id(2), at_ms(20));
    tracker.note_press(id(3), at_ms(30));
    assert_eq!(tracker.counters().press_times_forgotten, 1);

    // Забытый снимок №1 всё равно показывается, но без замера; №3 помнится.
    let (state, _picture) = save(&mut tracker, &ThumbnailState::Empty, 1, 10);
    tracker.on_tick(at_ms(100));
    assert_eq!(tracker.timing(), None);
    let (_state, _picture) = save(&mut tracker, &state, 3, 30);
    tracker.on_tick(at_ms(900));
    assert_eq!(tracker.timing().expect("№3").press_to_picture_ms(), Some(870.0));
}

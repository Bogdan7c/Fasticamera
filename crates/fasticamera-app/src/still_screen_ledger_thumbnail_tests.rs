//! Миниатюра в таблице снимков (14g): держится до итога своего снимка и уходит экрану ОДНИМ сообщением вместе
//! с «сохранено»; `Failed`, поздняя, вытесненная и чужой эпохи миниатюры выбрасываются со счётчиком.

use camera_contract::geometry::PixelSize;
use still_contract::thumbnail::StillThumbnail;

use super::*;

/// Миниатюра 2×1 px, все байты равны `fill`.
fn thumbnail(id: u64, fill: u8) -> StillThumbnail {
    StillThumbnail::new(StillId(id), PixelSize::new(2, 1), vec![fill; 8]).unwrap()
}

/// Сообщение «сохранено + картинка»: номер запроса и байты картинки.
fn saved_with_picture(input: &ScreenInput) -> Option<(u64, Vec<u8>)> {
    match input {
        ScreenInput::SavedWithPicture { request, picture } => Some((request.0, picture.rgba8().to_vec())),
        ScreenInput::Touch(_) | ScreenInput::Action(_) | ScreenInput::SystemGestureZones(_) => None,
    }
}

#[test]
fn a_held_thumbnail_leaves_with_saved_as_one_message_and_secured_does_not_take_it() {
    let (ledger, slot, mut receiver) = ledger_with_screen(8);
    ledger.register(StillId(7), ShotRequestId(3), slot.current_epoch());
    ledger.hold_thumbnail(thumbnail(7, 42));

    ledger.report_phase(StillPhase::Secured { id: StillId(7) });
    assert_eq!(
        drain(&mut receiver).len(),
        1,
        "Secured — одно действие, картинка осталась в таблице"
    );
    ledger.report_phase(saved_phase(7));

    let inputs = drain(&mut receiver);
    assert_eq!(inputs.len(), 1, "картинка и «сохранено» — одно сообщение: {inputs:?}");
    assert_eq!(saved_with_picture(&inputs[0]), Some((3, vec![42; 8])));
    let counters = ledger.counters();
    assert_eq!(
        (
            counters.thumbnails_held,
            counters.thumbnails_delivered,
            counters.saved_delivered
        ),
        (1, 1, 1)
    );
    assert_eq!(ledger.registered_count(), 0);
}

#[test]
fn saved_without_a_thumbnail_goes_as_a_plain_action_so_the_screen_falls_back_to_the_grey_disc() {
    let (ledger, slot, mut receiver) = ledger_with_screen(8);
    ledger.register(StillId(1), ShotRequestId(1), slot.current_epoch());

    ledger.report_phase(saved_phase(1));

    assert_eq!(drain(&mut receiver), [finished(1, CaptureOutcome::Saved)]);
    assert_eq!(ledger.counters().thumbnails_delivered, 0);
}

#[test]
fn a_failed_still_drops_its_thumbnail_and_the_screen_gets_no_picture() {
    let (ledger, slot, mut receiver) = ledger_with_screen(8);
    ledger.register(StillId(2), ShotRequestId(2), slot.current_epoch());
    ledger.hold_thumbnail(thumbnail(2, 9));

    ledger.report_phase(failed_phase(2, StillFailure::Develop("gpu".to_owned())));

    assert_eq!(
        drain(&mut receiver),
        [finished(2, CaptureOutcome::Failed(CaptureFailureReason::Processing))]
    );
    let counters = ledger.counters();
    assert_eq!(
        (counters.thumbnails_dropped_failed, counters.thumbnails_delivered),
        (1, 0)
    );
}

#[test]
fn a_late_or_unknown_thumbnail_is_dropped_with_a_count_and_never_stored() {
    let (ledger, slot, mut receiver) = ledger_with_screen(8);
    // Снимок не регистрировался.
    ledger.hold_thumbnail(thumbnail(50, 1));
    // Снимок уже закончился: миниатюра пришла позже итога.
    ledger.register(StillId(5), ShotRequestId(5), slot.current_epoch());
    ledger.report_phase(saved_phase(5));
    ledger.hold_thumbnail(thumbnail(5, 2));
    // Следующий снимок с тем же номером службы не получает чужую картинку.
    ledger.register(StillId(5), ShotRequestId(6), slot.current_epoch());
    ledger.report_phase(saved_phase(5));

    let inputs = drain(&mut receiver);
    assert_eq!(
        inputs,
        [finished(5, CaptureOutcome::Saved), finished(6, CaptureOutcome::Saved)]
    );
    let counters = ledger.counters();
    assert_eq!((counters.thumbnails_dropped_late, counters.thumbnails_held), (2, 0));
}

#[test]
fn evicting_a_registration_takes_its_thumbnail_with_it() {
    let (ledger, slot, mut receiver) = ledger_with_screen(1);
    ledger.register(StillId(1), ShotRequestId(1), slot.current_epoch());
    ledger.hold_thumbnail(thumbnail(1, 11));

    ledger.register(StillId(2), ShotRequestId(2), slot.current_epoch());
    ledger.report_phase(saved_phase(2));

    assert_eq!(ledger.counters().thumbnails_dropped_evicted, 1);
    assert_eq!(
        drain(&mut receiver),
        [finished(2, CaptureOutcome::Saved)],
        "чужая картинка к №2 не прилипла"
    );
}

#[test]
fn unregister_takes_the_thumbnail_with_it() {
    let (ledger, slot, _receiver) = ledger_with_screen(8);
    ledger.register(StillId(1), ShotRequestId(1), slot.current_epoch());
    ledger.hold_thumbnail(thumbnail(1, 11));

    assert!(ledger.unregister(StillId(1)));

    assert_eq!(ledger.counters().thumbnails_dropped_evicted, 1);
}

#[test]
fn a_thumbnail_for_a_replaced_screen_is_not_stored_and_a_picture_in_flight_is_not_delivered_to_the_new_screen() {
    let (ledger, slot, mut old_receiver) = ledger_with_screen(8);
    ledger.register(StillId(1), ShotRequestId(1), slot.current_epoch());
    ledger.register(StillId(2), ShotRequestId(2), slot.current_epoch());
    ledger.hold_thumbnail(thumbnail(2, 7));
    // Экран переустановлен: у него свой канал и новая эпоха.
    let (sender, mut new_receiver) = screen_input_channel(16);
    slot.attach(sender.injector());
    // Миниатюра снимка №1 пришла уже после смены экрана.
    ledger.hold_thumbnail(thumbnail(1, 8));

    ledger.report_phase(saved_phase(1));
    ledger.report_phase(saved_phase(2));

    assert!(
        drain(&mut new_receiver).is_empty(),
        "новый экран не получает ни картинок, ни итогов старого"
    );
    assert!(drain(&mut old_receiver).is_empty());
    let counters = ledger.counters();
    // №1: отброшена при приходе; №2: хранилась, но итог ушёл бы чужому экрану.
    assert_eq!(counters.thumbnails_dropped_stale_screen, 2);
    assert_eq!((counters.thumbnails_delivered, counters.dropped_stale_screen), (0, 2));
}

#[test]
fn a_picture_lost_with_a_destroyed_screen_channel_is_counted() {
    let (ledger, slot, receiver) = ledger_with_screen(8);
    ledger.register(StillId(1), ShotRequestId(1), slot.current_epoch());
    ledger.hold_thumbnail(thumbnail(1, 3));
    // Экран уничтожен без смены эпохи (рендер снял источник сам): канал закрыт, адрес в ячейке остался.
    drop(receiver);

    ledger.report_phase(saved_phase(1));

    let counters = ledger.counters();
    assert_eq!(
        (counters.thumbnails_lost_with_phase, counters.thumbnails_delivered),
        (1, 0)
    );
    assert_eq!(counters.lost_in_channel, 1);
}

#[test]
fn a_second_thumbnail_for_the_same_still_replaces_the_first_and_is_counted() {
    let (ledger, slot, mut receiver) = ledger_with_screen(8);
    ledger.register(StillId(1), ShotRequestId(1), slot.current_epoch());
    ledger.hold_thumbnail(thumbnail(1, 1));
    ledger.hold_thumbnail(thumbnail(1, 2));

    ledger.report_phase(saved_phase(1));

    let inputs = drain(&mut receiver);
    assert_eq!(
        saved_with_picture(&inputs[0]),
        Some((1, vec![2; 8])),
        "осталась последняя"
    );
    assert_eq!(ledger.counters().thumbnails_replaced, 1);
}

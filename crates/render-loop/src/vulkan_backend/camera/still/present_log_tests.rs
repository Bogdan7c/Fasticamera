//! Тесты разбора «что было на экране в момент нажатия»: доказательство только по соседним записям драйвера.

use super::*;

fn stamp(nanos: i64) -> SensorTimestamp {
    SensorTimestamp::from_nanos(nanos)
}

fn at(millis: i64) -> MonotonicNanos {
    MonotonicNanos::from_nanos(millis * 1_000_000)
}

/// Журнал: показы 10..=14, кадры А (показы 10, 11), Б (12, 13 — перекомпоновка), В (14); записи драйвера
/// по `timings` (present_id, время в мс).
fn log_with(timings: &[(u32, i64)]) -> PresentLog {
    let mut log = PresentLog::new(64);
    for (present_id, content) in [
        (10, ShownContent::Live(stamp(1))),
        (11, ShownContent::Live(stamp(1))),
        (12, ShownContent::Live(stamp(2))),
        (13, ShownContent::Live(stamp(2))),
        (14, ShownContent::Live(stamp(3))),
    ] {
        log.record_shown(ShownRecord { present_id, content });
    }
    for (present_id, millis) in timings {
        log.record_timing(*present_id, at(*millis));
    }
    log
}

#[test]
fn press_between_two_adjacent_records_proves_the_frame_shown_before_it() {
    // Показ 12 на экране с 100 мс, показ 13 — с 116 мс; нажатие в 110 мс → на экране кадр Б.
    let log = log_with(&[(10, 68), (11, 84), (12, 100), (13, 116), (14, 132)]);
    assert_eq!(
        log.resolve(at(110)),
        PressResolution::Proven {
            content: ShownContent::Live(stamp(2)),
            // Кадр Б впервые показан показом 12 (100 мс), а не перекомпоновкой 13.
            first_shown_at: at(100)
        }
    );
}

#[test]
fn a_press_exactly_on_a_present_belongs_to_that_present() {
    let log = log_with(&[(11, 84), (12, 100), (13, 116)]);
    // actual_present == press: показ 12 уже на экране.
    assert_eq!(
        log.resolve(at(100)),
        PressResolution::Proven {
            content: ShownContent::Live(stamp(2)),
            first_shown_at: at(100)
        }
    );
}

#[test]
fn records_only_up_to_the_press_keep_waiting() {
    // Последняя запись (13 в 116 мс) раньше нажатия (120): показ 14 мог появиться раньше нажатия, но записи ещё нет.
    let log = log_with(&[(11, 84), (12, 100), (13, 116)]);
    assert_eq!(log.resolve(at(120)), PressResolution::Waiting);
}

#[test]
fn no_timing_records_at_all_means_waiting_not_a_guess() {
    assert_eq!(log_with(&[]).resolve(at(100)), PressResolution::Waiting);
}

#[test]
fn a_hole_between_before_and_after_cannot_be_proven() {
    // Записи 12 (100 мс) и 14 (132 мс): 13 потеряна, нажатие в 110 мс могло прийтись на любой.
    let log = log_with(&[(12, 100), (14, 132)]);
    assert_eq!(
        log.resolve(at(110)),
        PressResolution::Unprovable(Unprovable::GapInTimingRecords)
    );
}

#[test]
fn all_records_after_the_press_cannot_be_proven() {
    let log = log_with(&[(12, 100), (13, 116)]);
    assert_eq!(
        log.resolve(at(50)),
        PressResolution::Unprovable(Unprovable::PressBeforeKnownTimings)
    );
}

#[test]
fn wrapping_present_ids_are_adjacent() {
    let mut log = PresentLog::new(8);
    log.record_shown(ShownRecord {
        present_id: u32::MAX,
        content: ShownContent::Live(stamp(9)),
    });
    log.record_shown(ShownRecord {
        present_id: 0,
        content: ShownContent::Live(stamp(10)),
    });
    log.record_timing(u32::MAX, at(10));
    log.record_timing(0, at(30));
    assert_eq!(
        log.resolve(at(20)),
        PressResolution::Proven {
            content: ShownContent::Live(stamp(9)),
            first_shown_at: at(10)
        }
    );
}

#[test]
fn a_held_picture_on_screen_is_proven_as_not_live() {
    let mut log = PresentLog::new(8);
    log.record_shown(ShownRecord {
        present_id: 1,
        content: ShownContent::NotLive,
    });
    log.record_shown(ShownRecord {
        present_id: 2,
        content: ShownContent::Live(stamp(1)),
    });
    log.record_timing(1, at(10));
    log.record_timing(2, at(30));
    assert_eq!(
        log.resolve(at(20)),
        PressResolution::Proven {
            content: ShownContent::NotLive,
            first_shown_at: at(10)
        }
    );
}

#[test]
fn a_present_forgotten_by_capacity_is_reported_not_guessed() {
    let mut log = PresentLog::new(2);
    for present_id in 1..=4 {
        log.record_shown(ShownRecord {
            present_id,
            content: ShownContent::Live(stamp(i64::from(present_id))),
        });
    }
    // Записи драйвера (вместимость тоже 2) — для показов 1 и 2, а журнал показов помнит только 3 и 4.
    log.record_timing(1, at(10));
    log.record_timing(2, at(30));
    log.record_timing(3, at(50));
    // Вытеснена запись 1: в `timings` остались 2 и 3; нажатие между ними, содержимое показа 2 забыто.
    assert_eq!(
        log.resolve(at(40)),
        PressResolution::Unprovable(Unprovable::ShownContentForgotten)
    );
}

#[test]
fn restart_forgets_everything_and_latest_shown_follows_the_newest_present() {
    let mut log = log_with(&[(12, 100), (13, 116)]);
    assert_eq!(log.latest_shown(), Some(ShownContent::Live(stamp(3))));
    log.restart();
    assert_eq!(log.latest_shown(), None);
    assert_eq!(log.resolve(at(110)), PressResolution::Waiting);
}

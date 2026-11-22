//! Журнал фаз: порядок, курсор, адреса файлов, видимая потеря при переполнении.

use still_contract::phase::{SavedStill, StillFailure, StillPhase};

use super::*;

fn saved(id: u64, with_raw: bool) -> StillPhase {
    StillPhase::Saved {
        id: StillId(id),
        files: SavedStill {
            primary: format!("content://media/{id}"),
            raw: with_raw.then(|| format!("content://media/{id}-raw")),
        },
    }
}

#[test]
fn an_empty_log_gives_nothing_and_loses_nothing() {
    let log = StillPhaseLog::new(4);
    let batch = log.entries_after(PhaseCursor::default());
    assert!(batch.entries.is_empty());
    assert_eq!(batch.lost_entries, 0);
    assert_eq!(log.cursor_now(), PhaseCursor(0));
}

#[test]
fn phases_keep_their_order_addresses_and_request_numbers() {
    let log = StillPhaseLog::new(8);
    log.record(ShotRequestId(5), &StillPhase::Secured { id: StillId(1) });
    log.record(ShotRequestId(5), &saved(1, true));
    log.record(
        ShotRequestId(6),
        &StillPhase::Failed {
            id: StillId(2),
            failure: StillFailure::QueueFull,
        },
    );

    let batch = log.entries_after(PhaseCursor::default());
    let kinds: Vec<(u64, u64, &PhaseKind)> = batch
        .entries
        .iter()
        .map(|entry| (entry.seq, entry.request.0, &entry.kind))
        .collect();
    assert_eq!(kinds[0], (1, 5, &PhaseKind::Secured));
    assert_eq!(
        kinds[1],
        (
            2,
            5,
            &PhaseKind::Saved {
                primary: "content://media/1".to_owned(),
                raw: Some("content://media/1-raw".to_owned()),
            }
        )
    );
    assert_eq!(kinds[2].0, 3);
    assert!(matches!(kinds[2].2, PhaseKind::Failed { failure } if failure.contains("очередь")));
    assert_eq!(batch.entries[0].still, StillId(1));
    assert!(batch.entries[0].at <= batch.entries[1].at, "время записи не идёт назад");
}

#[test]
fn a_cursor_skips_what_the_reader_has_already_seen() {
    let log = StillPhaseLog::new(8);
    log.record(ShotRequestId(1), &StillPhase::Secured { id: StillId(1) });
    let first = log.entries_after(PhaseCursor::default());
    let cursor = first.cursor_after(PhaseCursor::default());
    assert_eq!(cursor, PhaseCursor(1));

    log.record(ShotRequestId(1), &saved(1, false));
    let second = log.entries_after(cursor);
    assert_eq!(second.entries.len(), 1);
    assert!(matches!(second.entries[0].kind, PhaseKind::Saved { raw: None, .. }));
    assert_eq!(log.cursor_now(), PhaseCursor(2));
    // Пустая порция не двигает курсор.
    assert_eq!(
        log.entries_after(PhaseCursor(2)).cursor_after(PhaseCursor(2)),
        PhaseCursor(2)
    );
}

#[test]
fn overflow_evicts_the_oldest_and_tells_the_reader_how_many_it_missed() {
    let log = StillPhaseLog::new(2);
    for id in 1..=5 {
        log.record(ShotRequestId(id), &StillPhase::Secured { id: StillId(id) });
    }
    let batch = log.entries_after(PhaseCursor::default());
    assert_eq!(
        batch.entries.iter().map(|entry| entry.seq).collect::<Vec<_>>(),
        vec![4, 5]
    );
    assert_eq!(batch.lost_entries, 3, "записи 1..=3 вытеснены до чтения");

    // Читатель, успевший до вытеснения, потерь не видит.
    assert_eq!(log.entries_after(PhaseCursor(3)).lost_entries, 0);
}

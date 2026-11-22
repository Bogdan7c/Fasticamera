//! Тесты таблицы «снимок → экран»: перевод фаз службы в действия экрана, эпоха экрана, учёт, границы.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "в тестах падение — способ сообщить о провале"
)]

use camera_screen::shutter::CaptureFailureReason;
use still_contract::frame_choice::NoFrameReason;
use still_contract::phase::SavedStill;

use super::*;
use crate::camera_screen_input::{ScreenInput, ScreenInputReceiver, screen_input_channel};

/// Таблица с подключённым экраном (эпоха 1).
fn ledger_with_screen(capacity: usize) -> (StillScreenLedger, Arc<CaptureCompletionSlot>, ScreenInputReceiver) {
    let slot = Arc::new(CaptureCompletionSlot::default());
    let (sender, receiver) = screen_input_channel(16);
    slot.attach(sender.injector());
    // Писатель не нужен после выдачи адреса, но его канал должен жить: держим получателя.
    drop(sender);
    (StillScreenLedger::new(Arc::clone(&slot), capacity), slot, receiver)
}

fn drain(receiver: &mut ScreenInputReceiver) -> Vec<ScreenInput> {
    std::iter::from_fn(|| receiver.try_next()).collect()
}

fn saved_phase(id: u64) -> StillPhase {
    StillPhase::Saved {
        id: StillId(id),
        files: SavedStill {
            primary: "content://media/1".to_owned(),
            raw: None,
        },
    }
}

fn failed_phase(id: u64, failure: StillFailure) -> StillPhase {
    StillPhase::Failed {
        id: StillId(id),
        failure,
    }
}

fn finished(request: u64, outcome: CaptureOutcome) -> ScreenInput {
    ScreenInput::Action(ScreenAction::CaptureFinished {
        request: ShotRequestId(request),
        outcome,
    })
}

#[test]
fn secured_then_saved_reach_the_screen_with_the_screen_request_number() {
    let (ledger, slot, mut receiver) = ledger_with_screen(8);
    // Номер снимка службы (77) и номер запроса экрана (3) — разные: таблица их склеивает.
    ledger.register(StillId(77), ShotRequestId(3), slot.current_epoch());

    ledger.report_phase(StillPhase::Secured { id: StillId(77) });
    ledger.report_phase(saved_phase(77));

    assert_eq!(
        drain(&mut receiver),
        vec![
            ScreenInput::Action(ScreenAction::CaptureSecured {
                request: ShotRequestId(3)
            }),
            finished(3, CaptureOutcome::Saved),
        ]
    );
    let counters = ledger.counters();
    assert_eq!((counters.secured_delivered, counters.saved_delivered), (1, 1));
    assert_eq!(ledger.registered_count(), 0, "итоговая фаза снимает запись");
}

#[test]
fn secured_keeps_the_registration_until_the_final_phase() {
    let (ledger, slot, mut receiver) = ledger_with_screen(8);
    ledger.register(StillId(1), ShotRequestId(1), slot.current_epoch());
    ledger.report_phase(StillPhase::Secured { id: StillId(1) });
    assert_eq!(ledger.registered_count(), 1);
    drain(&mut receiver);
    // Итог после захвата не теряется: запись ещё на месте.
    ledger.report_phase(failed_phase(1, StillFailure::Store("диск полон".to_owned())));
    assert_eq!(
        drain(&mut receiver),
        vec![finished(1, CaptureOutcome::Failed(CaptureFailureReason::Storage))]
    );
}

#[test]
fn failure_reasons_map_consciously() {
    let cases = [
        (StillFailure::QueueFull, CaptureFailureReason::Processing),
        (StillFailure::ServiceUnavailable, CaptureFailureReason::Camera),
        (
            StillFailure::NoFrame(NoFrameReason::RingEmpty),
            CaptureFailureReason::Camera,
        ),
        (
            StillFailure::Develop("gpu".to_owned()),
            CaptureFailureReason::Processing,
        ),
        (
            StillFailure::Encode("heif".to_owned()),
            CaptureFailureReason::Processing,
        ),
        (StillFailure::Store("io".to_owned()), CaptureFailureReason::Storage),
    ];
    for (failure, expected) in cases {
        assert_eq!(capture_failure_reason(&failure), expected, "{failure:?}");
    }
}

#[test]
fn phase_of_the_previous_screen_is_not_delivered_to_the_new_one() {
    let (ledger, slot, mut old_receiver) = ledger_with_screen(8);
    // Снимок обещан старому экрану (эпоха 1)...
    ledger.register(StillId(5), ShotRequestId(1), slot.current_epoch());
    // ...экран переустановили: у нового свои номера, и его запрос №1 — другой снимок.
    let (new_sender, mut new_receiver) = screen_input_channel(16);
    slot.attach(new_sender.injector());

    ledger.report_phase(saved_phase(5));

    assert!(
        drain(&mut new_receiver).is_empty(),
        "новый экран не получает чужой итог"
    );
    assert!(drain(&mut old_receiver).is_empty(), "старый канал уже не адресат");
    let counters = ledger.counters();
    assert_eq!(counters.dropped_stale_screen, 1);
    assert_eq!(counters.saved_delivered, 0);
    assert_eq!(ledger.registered_count(), 0, "запись снята, итог не повторится");
}

#[test]
fn phase_without_a_screen_is_counted_not_swallowed() {
    let (ledger, slot, _receiver) = ledger_with_screen(8);
    let epoch_before_detach = slot.current_epoch();
    ledger.register(StillId(1), ShotRequestId(1), epoch_before_detach);
    // Экран снят и (новой эпохой) ещё не поставлен: эпоха сменилась — это «экран сменился».
    slot.detach();
    ledger.report_phase(saved_phase(1));
    assert_eq!(ledger.counters().dropped_stale_screen, 1);

    // Запрос, поставленный уже при снятом экране: эпоха та же, адреса нет.
    ledger.register(StillId(2), ShotRequestId(2), slot.current_epoch());
    ledger.report_phase(saved_phase(2));
    assert_eq!(ledger.counters().dropped_no_screen, 1);
}

#[test]
fn unknown_still_is_counted_and_touches_nothing() {
    let (ledger, slot, mut receiver) = ledger_with_screen(8);
    ledger.register(StillId(1), ShotRequestId(1), slot.current_epoch());
    ledger.report_phase(saved_phase(999));
    assert_eq!(ledger.counters().unknown_still, 1);
    assert!(drain(&mut receiver).is_empty());
    assert_eq!(
        ledger.registered_count(),
        1,
        "чужая фаза не трогает записи других снимков"
    );
}

#[test]
fn second_final_phase_for_the_same_still_is_not_delivered_twice() {
    let (ledger, slot, mut receiver) = ledger_with_screen(8);
    ledger.register(StillId(1), ShotRequestId(1), slot.current_epoch());
    ledger.report_phase(saved_phase(1));
    ledger.report_phase(failed_phase(1, StillFailure::ServiceUnavailable));
    assert_eq!(drain(&mut receiver), vec![finished(1, CaptureOutcome::Saved)]);
    assert_eq!(ledger.counters().unknown_still, 1);
}

#[test]
fn full_table_evicts_the_oldest_registration_with_a_counter() {
    let (ledger, slot, mut receiver) = ledger_with_screen(2);
    let epoch = slot.current_epoch();
    assert_eq!(
        ledger.register(StillId(1), ShotRequestId(1), epoch),
        RegisterOutcome::Registered
    );
    assert_eq!(
        ledger.register(StillId(2), ShotRequestId(2), epoch),
        RegisterOutcome::Registered
    );
    assert_eq!(
        ledger.register(StillId(3), ShotRequestId(3), epoch),
        RegisterOutcome::RegisteredEvictingOldest { evicted: StillId(1) }
    );
    assert_eq!(ledger.registered_count(), 2);
    assert_eq!(ledger.counters().evicted_registrations, 1);

    ledger.report_phase(saved_phase(1));
    assert_eq!(
        ledger.counters().unknown_still,
        1,
        "вытесненный снимок стал неизвестным"
    );
    ledger.report_phase(saved_phase(3));
    assert_eq!(drain(&mut receiver), vec![finished(3, CaptureOutcome::Saved)]);
}

#[test]
fn unregister_removes_only_the_named_still() {
    let (ledger, slot, _receiver) = ledger_with_screen(8);
    let epoch = slot.current_epoch();
    ledger.register(StillId(1), ShotRequestId(1), epoch);
    ledger.register(StillId(2), ShotRequestId(2), epoch);
    assert!(ledger.unregister(StillId(1)));
    assert!(!ledger.unregister(StillId(1)), "повторное снятие — no-op");
    assert_eq!(ledger.registered_count(), 1);
}

#[test]
fn closed_channel_is_counted_as_lost() {
    let (ledger, slot, receiver) = ledger_with_screen(8);
    ledger.register(StillId(1), ShotRequestId(1), slot.current_epoch());
    drop(receiver);
    ledger.report_phase(saved_phase(1));
    assert_eq!(ledger.counters().lost_in_channel, 1);
}

#[test]
fn the_phase_log_gets_the_address_and_the_screen_request_number_of_every_known_phase() {
    use crate::still_phase_log::{PhaseCursor, PhaseKind};
    let (ledger, slot, _receiver) = ledger_with_screen(8);
    ledger.register(StillId(77), ShotRequestId(3), slot.current_epoch());

    ledger.report_phase(StillPhase::Secured { id: StillId(77) });
    ledger.report_phase(saved_phase(77));
    // Снимок, которого в таблице нет, в журнал не попадает (он «неизвестный», а не чей-то итог).
    ledger.report_phase(saved_phase(99));

    let batch = ledger.phase_log().entries_after(PhaseCursor::default());
    assert_eq!(batch.entries.len(), 2, "{:?}", batch.entries);
    assert!(
        batch
            .entries
            .iter()
            .all(|entry| entry.request == ShotRequestId(3) && entry.still == StillId(77))
    );
    assert_eq!(batch.entries[0].kind, PhaseKind::Secured);
    assert_eq!(
        batch.entries[1].kind,
        PhaseKind::Saved {
            primary: "content://media/1".to_owned(),
            raw: None
        }
    );
}

#[test]
fn a_phase_for_a_stale_screen_is_still_logged_because_the_file_exists_regardless_of_the_screen() {
    use crate::still_phase_log::{PhaseCursor, PhaseKind};
    let (ledger, slot, _receiver) = ledger_with_screen(8);
    ledger.register(StillId(5), ShotRequestId(1), slot.current_epoch());
    slot.detach();

    ledger.report_phase(saved_phase(5));

    let batch = ledger.phase_log().entries_after(PhaseCursor::default());
    assert!(matches!(batch.entries.as_slice(), [entry] if matches!(entry.kind, PhaseKind::Saved { .. })));
    assert_eq!(
        ledger.counters().dropped_stale_screen,
        1,
        "экрану фаза не доставлена, но файл учтён"
    );
}

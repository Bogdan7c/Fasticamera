//! Допуск снимков и очередь событий рендера.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение — способ сообщить о провале"
)]

mod service_support;

use photo_output::service::{AdmissionRefused, DevelopSubmitError, StillServiceConfig};
use service_support::{Rig, fast_config};
use still_contract::develop::DevelopEvent;
use still_contract::frame_choice::NoFrameReason;
use still_contract::phase::StillFailure;
use still_contract::plan::{PrimaryFormat, RawOutput, StillId};

#[test]
fn eleventh_still_is_refused_until_one_is_saved() {
    let rig = Rig::start();
    let tickets: Vec<_> = (1..=10)
        .map(|id| rig.handle.try_admit(StillId(id)).expect("первые десять допущены"))
        .collect();
    assert_eq!(rig.handle.stats().pending_now, 10);

    let refused = rig.handle.try_admit(StillId(11)).unwrap_err();

    assert_eq!(refused, AdmissionRefused::QueueFull { limit: 10 });
    assert_eq!(StillFailure::from(refused), StillFailure::QueueFull);
    let stats = rig.handle.stats();
    assert_eq!(
        (stats.accepted, stats.admission_refusals, stats.pending_now),
        (10, 1, 10)
    );

    // Отдаём службе билет первого снимка и доводим его до Saved: слот освобождается.
    let mut tickets = tickets.into_iter();
    rig.handle.begin_still(tickets.next().unwrap()).unwrap();
    rig.secure(1, PrimaryFormat::Heif, RawOutput::Skip);
    rig.send(DevelopEvent::PrimaryRendered { id: StillId(1) });
    rig.wait_final(1);

    let eleventh = rig.handle.try_admit(StillId(11));
    assert!(eleventh.is_ok(), "после Saved снова можно");
    // Остальные девять снимков отказом не тронуты.
    assert_eq!(rig.handle.stats().pending_now, 10);
}

#[test]
fn failed_still_also_frees_its_slot() {
    let rig = Rig::start_with(StillServiceConfig {
        max_pending_stills: 1,
        ..fast_config()
    });
    rig.admit(1);
    assert!(rig.handle.try_admit(StillId(2)).is_err());

    rig.send(DevelopEvent::NotSecured {
        id: StillId(1),
        reason: NoFrameReason::RingEmpty,
    });
    rig.wait_final(1);

    assert!(rig.handle.try_admit(StillId(2)).is_ok());
}

#[test]
fn ticket_dropped_by_the_app_frees_its_slot() {
    let rig = Rig::start_with(StillServiceConfig {
        max_pending_stills: 1,
        ..fast_config()
    });
    let ticket = rig.handle.try_admit(StillId(1)).unwrap();
    assert_eq!(rig.handle.stats().pending_now, 1);

    drop(ticket); // например, план не ушёл рендеру

    assert_eq!(rig.handle.stats().pending_now, 0);
    assert!(rig.handle.try_admit(StillId(2)).is_ok());
}

#[test]
fn duplicate_still_id_is_refused_without_touching_the_first() {
    let rig = Rig::start();
    let _first = rig.handle.try_admit(StillId(1)).unwrap();

    let refused = rig.handle.try_admit(StillId(1)).unwrap_err();

    assert_eq!(refused, AdmissionRefused::DuplicateId(StillId(1)));
    assert_eq!(rig.handle.stats().pending_now, 1);
}

#[test]
fn event_queue_overflow_is_an_explicit_error_not_a_block() {
    let rig = Rig::start_with(StillServiceConfig {
        event_queue_capacity: 1,
        ..fast_config()
    });
    rig.admit(1);
    // Получатель фаз «застрял»: рабочий поток встаёт внутри `report_phase(Secured)`.
    rig.phases.hold();
    rig.send(DevelopEvent::Secured {
        facts: service_support::facts_for(1, PrimaryFormat::Heif, RawOutput::Skip),
        raw: None,
    });
    rig.phases.wait_secured(StillId(1), service_support::WAIT);

    // Одно событие помещается в очередь, второе — отказ, и вызов вернулся сразу.
    let first = rig
        .handle
        .submit_develop_event(DevelopEvent::PrimaryRendered { id: StillId(1) });
    let second = rig
        .handle
        .submit_develop_event(DevelopEvent::PrimaryRendered { id: StillId(1) });

    assert_eq!(first, Ok(()));
    assert_eq!(second, Err(DevelopSubmitError::QueueFull));
    assert_eq!(rig.handle.stats().rejected_events, 1);
    rig.phases.release();
    rig.wait_final(1);
}

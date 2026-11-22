//! Срывы снимка: кодер, хранилище, рендер. Общее правило — «всё или ничего»: после срыва в галерее
//! ничего нет, черновики удалены, допуск освобождён, итог — фаза `Failed` с причиной.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение — способ сообщить о провале"
)]

mod service_support;

use service_support::{DraftState, EncoderEvent, Rig, readback_for};
use still_contract::develop::{DevelopEvent, DevelopFailure, PrimaryTargetOffer};
use still_contract::frame_choice::NoFrameReason;
use still_contract::phase::{StillFailure, StillPhase};
use still_contract::plan::{PrimaryFormat, RawOutput, StillId};

fn failure_of(phase: StillPhase) -> StillFailure {
    match phase {
        StillPhase::Failed { failure, .. } => failure,
        other => panic!("ждали Failed, пришло {other:?}"),
    }
}

/// После срыва: ничего не опубликовано и не осталось «висящих» черновиков, слот свободен.
fn assert_nothing_left_behind(rig: &Rig) {
    assert_eq!(rig.store.log.published_count(), 0, "в галерее не должно быть файлов");
    assert_eq!(rig.store.log.pending_count(), 0, "черновики не должны висеть");
    assert!(
        rig.store
            .log
            .drafts()
            .iter()
            .all(|draft| draft.state == DraftState::Discarded),
        "каждый черновик удалён явно: {:?}",
        rig.store
            .log
            .drafts()
            .iter()
            .map(|draft| &draft.state)
            .collect::<Vec<_>>()
    );
    assert_eq!(rig.handle.stats().pending_now, 0);
}

#[test]
fn encoder_that_fails_to_open_gives_unavailable_and_discards_drafts() {
    let rig = Rig::start();
    rig.heif.fail_open();
    rig.admit(1);

    let offer = rig.secure(1, PrimaryFormat::Heif, RawOutput::Dng);

    assert!(
        matches!(offer, PrimaryTargetOffer::Unavailable { id } if id == StillId(1)),
        "{offer:?}"
    );
    let failure = failure_of(rig.wait_final(1));
    assert!(
        matches!(&failure, StillFailure::Encode(text) if text.contains("кодер занят")),
        "{failure:?}"
    );
    assert_eq!(
        rig.store.log.drafts().len(),
        2,
        "основной и DNG были созданы, потом удалены"
    );
    assert_nothing_left_behind(&rig);
    assert_eq!(rig.handle.stats().failed_encode, 1);
    assert_eq!(rig.handle.stats().discarded_drafts, 2);
}

#[test]
fn encoder_without_input_surface_is_aborted() {
    let rig = Rig::start();
    rig.heif.withhold_surface();
    rig.admit(1);

    let offer = rig.secure(1, PrimaryFormat::Heif, RawOutput::Skip);

    assert!(matches!(offer, PrimaryTargetOffer::Unavailable { .. }));
    assert!(matches!(failure_of(rig.wait_final(1)), StillFailure::Encode(_)));
    assert_eq!(rig.heif.events(), vec![EncoderEvent::Aborted]);
    assert_nothing_left_behind(&rig);
}

#[test]
fn failed_draft_creation_discards_the_one_already_created() {
    let rig = Rig::start();
    rig.store.fail_create_for("DNG");
    rig.admit(1);

    let offer = rig.secure(1, PrimaryFormat::Heif, RawOutput::Dng);

    assert!(matches!(offer, PrimaryTargetOffer::Unavailable { .. }));
    let failure = failure_of(rig.wait_final(1));
    assert!(
        matches!(&failure, StillFailure::Store(text) if text.contains("диск полон")),
        "{failure:?}"
    );
    assert_eq!(rig.store.log.drafts().len(), 1);
    assert!(
        rig.heif.opens().is_empty(),
        "кодер не открывают, если хранилище отказало"
    );
    assert_nothing_left_behind(&rig);
}

#[test]
fn unknown_camera_fails_the_still_before_any_draft_exists() {
    let rig = Rig::start();
    rig.cameras.forget_all_cameras();
    rig.admit(1);

    let offer = rig.secure(1, PrimaryFormat::Jpeg, RawOutput::Skip);

    assert!(matches!(offer, PrimaryTargetOffer::Unavailable { .. }));
    assert!(matches!(failure_of(rig.wait_final(1)), StillFailure::Store(_)));
    assert!(rig.store.log.drafts().is_empty());
    assert_eq!(rig.handle.stats().pending_now, 0);
}

#[test]
fn not_secured_fails_with_no_frame_and_creates_no_drafts() {
    let rig = Rig::start();
    rig.admit(1);

    rig.send(DevelopEvent::NotSecured {
        id: StillId(1),
        reason: NoFrameReason::RingEmpty,
    });

    assert_eq!(
        failure_of(rig.wait_final(1)),
        StillFailure::NoFrame(NoFrameReason::RingEmpty)
    );
    assert!(rig.store.log.drafts().is_empty());
    assert!(rig.heif.opens().is_empty());
    let stats = rig.handle.stats();
    assert_eq!((stats.failed_no_frame, stats.pending_now), (1, 0));
    assert_eq!(rig.phases.of(StillId(1)).len(), 1, "Secured не было — только итог");
}

#[test]
fn render_failure_after_secured_aborts_encoder_and_discards_everything() {
    let rig = Rig::start();
    rig.admit(1);
    rig.secure(1, PrimaryFormat::Heif, RawOutput::Dng);

    rig.send(DevelopEvent::Failed {
        id: StillId(1),
        failure: DevelopFailure::Gpu("device lost".to_owned()),
    });

    let failure = failure_of(rig.wait_final(1));
    assert!(
        matches!(&failure, StillFailure::Develop(text) if text.contains("device lost")),
        "{failure:?}"
    );
    assert_eq!(rig.heif.events(), vec![EncoderEvent::Aborted]);
    assert_eq!(rig.store.log.drafts().len(), 2);
    assert_nothing_left_behind(&rig);
    // Фаза Secured была, но Saved не будет никогда.
    assert!(
        rig.phases
            .of(StillId(1))
            .iter()
            .all(|phase| !matches!(phase, StillPhase::Saved { .. }))
    );
}

#[test]
fn encoder_finish_failure_discards_files() {
    let rig = Rig::start();
    rig.heif.fail_finish();
    rig.admit(1);
    rig.secure(1, PrimaryFormat::Heif, RawOutput::Dng);

    rig.send(DevelopEvent::PrimaryRendered { id: StillId(1) });

    assert!(matches!(failure_of(rig.wait_final(1)), StillFailure::Encode(_)));
    assert_nothing_left_behind(&rig);
}

#[test]
fn primary_publish_failure_leaves_no_dng_in_the_gallery() {
    let rig = Rig::start();
    rig.store.fail_publish_for("HEIC");
    rig.admit(1);
    rig.secure(1, PrimaryFormat::Heif, RawOutput::Dng);
    service_support::wait_until("DNG записан", || {
        rig.store.log.by_extension("DNG").is_some_and(|d| !d.bytes.is_empty())
    });

    rig.send(DevelopEvent::PrimaryRendered { id: StillId(1) });

    let failure = failure_of(rig.wait_final(1));
    assert!(
        matches!(&failure, StillFailure::Store(text) if text.contains("основного")),
        "{failure:?}"
    );
    assert_eq!(rig.store.log.published_count(), 0, "DNG тоже не опубликован");
    assert_eq!(rig.store.log.by_extension("DNG").unwrap().state, DraftState::Discarded);
    assert_eq!(rig.handle.stats().raw_lost_after_primary_published, 0);
    assert_eq!(rig.handle.stats().pending_now, 0);
}

#[test]
fn dng_publish_failure_after_primary_still_saves_the_photo_without_raw() {
    let rig = Rig::start();
    rig.store.fail_publish_for("DNG");
    rig.admit(1);
    rig.secure(1, PrimaryFormat::Heif, RawOutput::Dng);
    rig.send(DevelopEvent::PrimaryRendered { id: StillId(1) });

    let phase = rig.wait_final(1);

    // Основной файл уже в галерее и откатить его нельзя: экран получает Saved без raw, потеря учтена.
    let StillPhase::Saved { files, .. } = phase else {
        panic!("ждали Saved, пришло {phase:?}");
    };
    assert_eq!(
        files.primary,
        format!("content://fake/{}.HEIC", service_support::FixedClock::STEM)
    );
    assert_eq!(files.raw, None);
    assert_eq!(rig.store.log.by_extension("HEIC").unwrap().state, DraftState::Published);
    let stats = rig.handle.stats();
    assert_eq!(
        (stats.raw_lost_after_primary_published, stats.saved, stats.pending_now),
        (1, 1, 0)
    );
}

#[test]
fn dng_write_failure_fails_the_still_and_aborts_the_encoder() {
    let rig = Rig::start();
    rig.formats.fail_dng();
    rig.admit(1);

    // Цель уже ушла рендеру (DNG пишется параллельно с проявлением), но снимок срывается.
    let offer = rig.secure(1, PrimaryFormat::Heif, RawOutput::Dng);
    assert!(matches!(offer, PrimaryTargetOffer::Ready { .. }));

    let failure = failure_of(rig.wait_final(1));
    assert!(
        matches!(&failure, StillFailure::Store(text) if text.contains("DNG")),
        "{failure:?}"
    );
    assert_eq!(rig.heif.events(), vec![EncoderEvent::Aborted]);
    assert_nothing_left_behind(&rig);

    // Опоздавший итог рендера по сорванному снимку ничего не публикует.
    rig.send(DevelopEvent::PrimaryRendered { id: StillId(1) });
    service_support::wait_until("опоздавшее событие учтено", || {
        rig.handle.stats().late_events == 1
    });
    assert_eq!(rig.store.log.published_count(), 0);
}

#[test]
fn jpeg_compression_failure_discards_files() {
    let rig = Rig::start();
    rig.jpeg.fail();
    rig.admit(1);
    rig.secure(1, PrimaryFormat::Jpeg, RawOutput::Dng);

    rig.send(DevelopEvent::PrimaryReadback {
        id: StillId(1),
        image: readback_for(9),
    });

    assert!(matches!(failure_of(rig.wait_final(1)), StillFailure::Encode(_)));
    assert_nothing_left_behind(&rig);
}

#[test]
fn render_that_cannot_take_the_target_fails_the_still_as_unavailable() {
    let rig = Rig::start();
    rig.targets.make_render_unavailable();
    rig.admit(1);

    rig.handle
        .submit_develop_event(DevelopEvent::Secured {
            facts: service_support::facts_for(1, PrimaryFormat::Heif, RawOutput::Skip),
            raw: None,
        })
        .unwrap();

    assert_eq!(failure_of(rig.wait_final(1)), StillFailure::ServiceUnavailable);
    assert_eq!(rig.heif.events(), vec![EncoderEvent::Aborted]);
    assert_nothing_left_behind(&rig);
}

#[test]
fn event_of_the_wrong_kind_fails_the_still_instead_of_publishing() {
    let rig = Rig::start();
    rig.admit(1);
    rig.secure(1, PrimaryFormat::Heif, RawOutput::Skip);

    // Для HEIF должен прийти `PrimaryRendered`, а не картинка в памяти.
    rig.send(DevelopEvent::PrimaryReadback {
        id: StillId(1),
        image: readback_for(0),
    });

    assert!(matches!(failure_of(rig.wait_final(1)), StillFailure::Develop(_)));
    assert_eq!(rig.heif.events(), vec![EncoderEvent::Aborted]);
    assert_nothing_left_behind(&rig);
}

#[test]
fn dng_in_plan_without_raw_samples_fails_the_still() {
    let rig = Rig::start();
    rig.admit(1);

    rig.handle
        .submit_develop_event(DevelopEvent::Secured {
            facts: service_support::facts_for(1, PrimaryFormat::Heif, RawOutput::Dng),
            raw: None,
        })
        .unwrap();

    assert!(matches!(failure_of(rig.wait_final(1)), StillFailure::Develop(_)));
    assert!(rig.store.log.drafts().is_empty());
}

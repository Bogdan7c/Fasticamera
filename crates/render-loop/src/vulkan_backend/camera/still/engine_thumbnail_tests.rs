//! Миниатюра в движке снимков (14g) на поддельном GPU: приходит ровно один раз и СТРОГО до итога, на обоих путях
//! (память для JPEG, поверхность кодера HEIF); нет миниатюры — итог тот же и виден счётчик; потеря GPU на
//! проявлении — ни миниатюры, ни второго итога.

use camera_contract::capture_result::SensorTimestamp;
use shell_contract::orientation::DeviceOrientation;
use still_contract::develop::{DevelopEvent, DevelopFailure, PrimaryTarget, PrimaryTargetOffer};
use still_contract::plan::StillId;

use super::fake_gpu::FAKE_THUMBNAIL_SIZE;
use super::test_support::{Arrival, EngineRig, MAIN, Sink, StreamRig, engine_rig, fake_encoder_surface, ms, plan};
use crate::backend::PastPresentation;
use crate::config::StillDevelopConfig;
use crate::still::{StillCommand, StillCommandOutcome};

/// Движок с 8 показанными кадрами, каждый доказан записью показа (как в соседних тестах протокола).
fn proven_rig() -> EngineRig {
    let mut stream = StreamRig::with_ring(MAIN);
    let frames: Vec<SensorTimestamp> = stream.offer_many(8);
    let mut rig = engine_rig(StillDevelopConfig::default(), stream);
    let feed = rig.stream.feed_for(&frames, 0);
    rig.engine.ingest_feed(feed);
    let timings: Vec<PastPresentation> = (0..8)
        .map(|index| PastPresentation {
            present_id: index,
            frame_index: u64::from(index),
            actual_present: ms(10 * i64::from(index)),
        })
        .collect();
    rig.engine.ingest_presentations(&timings);
    rig
}

fn offer(rig: &mut EngineRig, id: u64, target: PrimaryTarget) {
    let outcome = rig
        .engine
        .apply_command(StillCommand::Target(PrimaryTargetOffer::Ready {
            id: StillId(id),
            target,
        }));
    assert!(matches!(outcome, StillCommandOutcome::Accepted));
}

/// Позиции событий в порядке прихода: `Secured`, затем что угодно.
fn arrival_kinds(sink: &Sink) -> Vec<&'static str> {
    sink.arrivals()
        .iter()
        .map(|arrival| match arrival {
            Arrival::Thumbnail(_) => "thumbnail",
            Arrival::Event { is_final: true, .. } => "final",
            Arrival::Event { is_final: false, .. } => "secured",
        })
        .collect()
}

#[test]
fn readback_still_gets_one_thumbnail_strictly_before_its_final_event() {
    let mut rig = proven_rig();
    let sink = rig.begin(plan(1, 25));
    rig.run(6);
    offer(&mut rig, 1, PrimaryTarget::Readback);
    rig.run(10);

    assert_eq!(
        arrival_kinds(&sink),
        ["secured", "thumbnail", "final"],
        "порядок прихода"
    );
    let thumbnails = sink.take_thumbnails();
    assert_eq!(thumbnails.len(), 1, "не больше одной миниатюры на снимок");
    assert_eq!(thumbnails[0].id(), StillId(1));
    assert_eq!(thumbnails[0].size(), FAKE_THUMBNAIL_SIZE);
    assert!(
        thumbnails[0].rgba_bytes().iter().all(|byte| *byte == 1),
        "байты миниатюры дошли без потерь"
    );
    assert!(matches!(
        sink.take().last(),
        Some(DevelopEvent::PrimaryReadback { id: StillId(1), .. })
    ));
    let diagnostics = rig.engine.diagnostics();
    assert_eq!(
        (diagnostics.thumbnails_delivered, diagnostics.thumbnails_unavailable),
        (1, 0)
    );
    assert_eq!(diagnostics.outcomes.readback, 1);
    assert_eq!(rig.gpu.lock().unwrap().thumbnails_taken, 1, "GPU спросили один раз");
}

#[test]
fn encoder_still_gets_its_thumbnail_before_the_picture_is_shown_to_the_encoder() {
    let mut rig = proven_rig();
    // Показ в кодер долгий: итог придёт заметно позже готовой картинки, миниатюра обязана быть уже у приёмника.
    rig.gpu.lock().unwrap().present_polls_needed = 4;
    let sink = rig.begin(plan(1, 25));
    rig.run(6);
    offer(&mut rig, 1, PrimaryTarget::EncoderSurface(fake_encoder_surface()));
    // Шаги, пока миниатюра не придёт (проявление идёт по полосам); показ в кодер к тому времени не кончился.
    for _ in 0..10 {
        rig.run(1);
        if !sink
            .arrivals()
            .iter()
            .all(|arrival| !matches!(arrival, Arrival::Thumbnail(_)))
        {
            break;
        }
    }
    assert_eq!(sink.finals_for(StillId(1)), 0, "показ в кодер ещё идёт");
    assert_eq!(
        sink.take_thumbnails().len(),
        1,
        "миниатюра пришла, не дожидаясь показа в кодер"
    );
    rig.run(12);

    assert_eq!(arrival_kinds(&sink), ["secured", "thumbnail", "final"]);
    assert!(matches!(
        sink.take().last(),
        Some(DevelopEvent::PrimaryRendered { id: StillId(1) })
    ));
    assert_eq!(rig.engine.diagnostics().thumbnails_delivered, 1);
}

#[test]
fn missing_thumbnail_leaves_the_outcome_unchanged_and_is_counted() {
    let mut rig = proven_rig();
    rig.gpu.lock().unwrap().thumbnail_fails = true;
    let sink = rig.begin(plan(1, 25));
    rig.run(6);
    offer(&mut rig, 1, PrimaryTarget::Readback);
    rig.run(10);

    assert_eq!(arrival_kinds(&sink), ["secured", "final"], "миниатюры нет, итог один");
    assert_eq!(sink.finals_for(StillId(1)), 1);
    assert!(matches!(sink.take().last(), Some(DevelopEvent::PrimaryReadback { .. })));
    let diagnostics = rig.engine.diagnostics();
    assert_eq!(
        (diagnostics.thumbnails_delivered, diagnostics.thumbnails_unavailable),
        (0, 1)
    );
    assert_eq!(diagnostics.outcomes.readback, 1);
    assert_eq!(diagnostics.outcomes.failed_gpu, 0, "снимок без миниатюры — не сбой");
}

#[test]
fn two_stills_get_their_own_thumbnails_tagged_with_their_ids() {
    let mut rig = proven_rig();
    let first = rig.begin(plan(1, 25));
    let second = rig.begin(plan(2, 45));
    rig.run(8);
    offer(&mut rig, 1, PrimaryTarget::Readback);
    offer(&mut rig, 2, PrimaryTarget::Readback);
    rig.run(20);

    let (first_thumbnails, second_thumbnails) = (first.take_thumbnails(), second.take_thumbnails());
    assert_eq!((first_thumbnails.len(), second_thumbnails.len()), (1, 1));
    assert_eq!(first_thumbnails[0].id(), StillId(1));
    assert_eq!(second_thumbnails[0].id(), StillId(2));
    assert_ne!(
        first_thumbnails[0].rgba_bytes(),
        second_thumbnails[0].rgba_bytes(),
        "содержимое миниатюр разное: у каждого снимка своя"
    );
    assert_eq!(rig.engine.diagnostics().thumbnails_delivered, 2);
}

#[test]
fn lost_gpu_during_development_gives_no_thumbnail_and_one_failed_final() {
    let mut rig = proven_rig();
    rig.gpu.lock().unwrap().lose_develop = true;
    let sink = rig.begin(plan(1, 25));
    rig.run(6);
    offer(&mut rig, 1, PrimaryTarget::Readback);
    rig.run(6);

    assert_eq!(arrival_kinds(&sink), ["secured", "final"]);
    assert!(sink.take_thumbnails().is_empty());
    assert!(matches!(
        sink.take().last(),
        Some(DevelopEvent::Failed {
            failure: DevelopFailure::Gpu(_),
            ..
        })
    ));
    let diagnostics = rig.engine.diagnostics();
    assert_eq!(
        (diagnostics.thumbnails_delivered, diagnostics.thumbnails_unavailable),
        (0, 0)
    );
    assert_eq!(diagnostics.outcomes.failed_gpu, 1);
}

#[test]
fn development_failure_gives_no_thumbnail() {
    let mut rig = proven_rig();
    rig.gpu.lock().unwrap().fail_develop = true;
    let sink = rig.begin(plan(1, 25));
    rig.run(6);
    offer(&mut rig, 1, PrimaryTarget::Readback);
    rig.run(6);

    assert_eq!(arrival_kinds(&sink), ["secured", "final"]);
    assert_eq!(rig.engine.diagnostics().thumbnails_delivered, 0);
}

#[test]
fn the_still_rotation_from_the_frame_facts_reaches_the_gpu_for_the_thumbnail() {
    let mut rig = proven_rig();
    let upright = rig.begin(plan(1, 25));
    let mut turned_plan = plan(2, 45);
    turned_plan.orientation = DeviceOrientation::LandscapeRight;
    let turned = rig.begin(turned_plan);
    rig.run(6);
    offer(&mut rig, 1, PrimaryTarget::Readback);
    offer(&mut rig, 2, PrimaryTarget::Readback);
    rig.run(20);
    let rotations = rig.gpu.lock().unwrap().develop_rotations.clone();
    assert_eq!(rotations.len(), 2, "проявлений начато: {rotations:?}");
    let facts_rotation = |sink: &Sink| match sink.take().first() {
        Some(DevelopEvent::Secured { facts, .. }) => facts.rotation,
        other => panic!("ждали Secured: {other:?}"),
    };
    let (upright_rotation, turned_rotation) = (facts_rotation(&upright), facts_rotation(&turned));
    assert_ne!(
        upright_rotation, turned_rotation,
        "ориентация телефона меняет поворот снимка"
    );
    assert_eq!(
        rotations,
        [upright_rotation, turned_rotation],
        "GPU получил поворот фактов кадра"
    );
}

#[test]
fn an_entry_hands_over_at_most_one_thumbnail_and_none_after_its_final_event() {
    use std::sync::Arc;

    use super::entry::{Stage, StillEntry, StillOutcome};
    use super::fake_gpu::FakeGpu;
    use crate::still::StillEventSink;
    use still_contract::thumbnail::StillThumbnail;

    let thumbnail = || StillThumbnail::new(StillId(1), FAKE_THUMBNAIL_SIZE, vec![5; 4 * 3 * 4]).unwrap();
    let sink = Arc::new(Sink::default());
    let events: Arc<dyn StillEventSink> = Arc::clone(&sink) as Arc<dyn StillEventSink>;
    let mut entry: StillEntry<FakeGpu> = StillEntry::new(plan(1, 25), events, Stage::Transit);
    assert!(entry.deliver_thumbnail(thumbnail()), "первая миниатюра принята");
    assert!(!entry.deliver_thumbnail(thumbnail()), "вторая отвергнута");
    assert_eq!(sink.take_thumbnails().len(), 1, "приёмнику ушла одна");
    entry.deliver_final(StillOutcome::Rendered);
    assert_eq!(sink.finals_for(StillId(1)), 1);
}

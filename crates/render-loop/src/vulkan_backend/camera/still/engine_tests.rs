//! Тесты протокола движка снимков на настоящем кольце и поддельном GPU: выбор кадра (точный/по оценке/нет кадра),
//! история параметров, `Secured` с фактами и сырыми данными, цель, проявление, итог РОВНО один.

use std::time::Duration;

use camera_contract::capture_result::SensorTimestamp;
use raw_frame_stream::RawFrameStreamConfig;
use still_contract::develop::{DevelopEvent, DevelopFailure, PrimaryTarget, PrimaryTargetOffer};
use still_contract::frame_choice::{FrameChoice, NoFrameReason, SubstitutionReason};
use still_contract::plan::StillId;

use super::test_support::{EngineRig, MAIN, StreamRig, engine_rig, fake_encoder_surface, ms, plan, plan_with_dng};
use crate::backend::PastPresentation;
use crate::config::StillDevelopConfig;
use crate::still::{StillCommand, StillCommandOutcome};

/// Движок с 6 показанными кадрами (кадр `i` показан под номером `i`, виден на экране с `10·i` мс) и записями
/// драйвера для каждого показа: так момент нажатия доказуем.
fn proven_rig() -> (EngineRig, Vec<SensorTimestamp>) {
    let mut stream = StreamRig::with_ring(MAIN);
    let frames = stream.offer_many(6);
    let mut rig = engine_rig(StillDevelopConfig::default(), stream);
    let feed = rig.stream.feed_for(&frames, 0);
    rig.engine.ingest_feed(feed);
    let timings: Vec<PastPresentation> = (0..6)
        .map(|index| PastPresentation {
            present_id: index,
            frame_index: u64::from(index),
            actual_present: ms(10 * i64::from(index)),
        })
        .collect();
    rig.engine.ingest_presentations(&timings);
    (rig, frames)
}

fn secured_facts(events: &[DevelopEvent]) -> &still_contract::shot::StillFrameFacts {
    match events.first() {
        Some(DevelopEvent::Secured { facts, .. }) => facts,
        other => panic!("первым событием должен быть Secured, а пришло {other:?}"),
    }
}

#[test]
fn proven_press_secures_exactly_the_frame_that_was_on_screen() {
    let (mut rig, frames) = proven_rig();
    // Показы 2 (20 мс) и 3 (30 мс): нажатие в 25 мс → на экране кадр 2.
    let sink = rig.begin(plan(1, 25));
    rig.run(3);
    let events = sink.take();
    let facts = secured_facts(&events);
    assert_eq!(facts.sensor_timestamp, frames[2]);
    assert_eq!(
        facts.choice,
        FrameChoice::ShownAtPress {
            sensor_timestamp: frames[2],
            shown_at: ms(20)
        }
    );
    assert_eq!(
        rig.gpu.lock().unwrap().captured,
        vec![frames[2]],
        "копировался именно показанный кадр"
    );
    let diagnostics = rig.engine.diagnostics();
    assert_eq!(diagnostics.picks.exact, 1);
    assert_eq!(diagnostics.params.own, 1, "параметры своего кадра");
    // Нажатие в 25 мс, кадр на экране с 20 мс: стоял 5 мс.
    assert_eq!(diagnostics.exact_press_after_shown.count, 1);
    assert!((diagnostics.exact_press_after_shown.max_ms - 5.0).abs() < 1e-9);
}

#[test]
fn records_up_to_the_press_only_keep_the_still_waiting_and_then_fall_back_to_the_estimate() {
    let (mut rig, frames) = proven_rig();
    // Нажатие в 55 мс: все записи (до 50 мс) раньше нажатия — закрывающей записи ещё нет.
    let sink = rig.begin(plan(2, 55));
    rig.run(2);
    assert_eq!(sink.len(), 0, "ждём записи показа: ни Secured, ни итога");
    assert!(rig.gpu.lock().unwrap().captured.is_empty());

    // Срок `presentation_wait_ms` (80) вышел: кадр по оценке — последний показанный к приходу команды.
    rig.run(8);
    let events = sink.take();
    let facts = secured_facts(&events);
    assert_eq!(
        facts.choice,
        FrameChoice::Substituted {
            sensor_timestamp: frames[5],
            wanted: Some(frames[5]),
            reason: SubstitutionReason::PresentationTimingUnknown
        }
    );
    let diagnostics = rig.engine.diagnostics();
    assert_eq!(diagnostics.presentation.wait_expired, 1);
    assert_eq!(diagnostics.picks.substituted_timing_unknown, 1);
    assert_eq!(diagnostics.picks.exact, 0, "точным такой снимок не называется");
}

#[test]
fn late_timing_records_turn_waiting_into_a_proven_choice() {
    let (mut rig, frames) = proven_rig();
    let sink = rig.begin(plan(3, 55));
    rig.run(2);
    assert_eq!(sink.len(), 0);
    // Показ 6 (кадр, который появится на экране в 60 мс) приходит в записях позже — закрывает момент нажатия.
    rig.engine.ingest_presentations(&[PastPresentation {
        present_id: 6,
        frame_index: 6,
        actual_present: ms(60),
    }]);
    rig.run(3);
    let events = sink.take();
    let facts = secured_facts(&events);
    // На экране в 55 мс стоял показ 5 (50 мс) = кадр 5.
    assert_eq!(facts.sensor_timestamp, frames[5]);
    assert!(facts.choice.is_exact());
}

#[test]
fn no_timing_records_at_all_gives_an_estimate_choice_after_the_wait() {
    let mut stream = StreamRig::with_ring(MAIN);
    let frames = stream.offer_many(3);
    let mut rig = engine_rig(StillDevelopConfig::default(), stream);
    let feed = rig.stream.feed_for(&frames, 0);
    rig.engine.ingest_feed(feed);
    let sink = rig.begin(plan(4, 10));
    rig.run(10);
    let events = sink.take();
    let facts = secured_facts(&events);
    assert!(!facts.choice.is_exact());
    assert_eq!(facts.sensor_timestamp, frames[2]);
}

#[test]
fn nothing_shown_means_no_frame_and_a_disabled_ring_is_named() {
    // Ничего не показано: «живого кадра нет».
    let stream = StreamRig::with_ring(MAIN);
    let mut rig = engine_rig(StillDevelopConfig::default(), stream);
    let sink = rig.begin(plan(1, 10));
    rig.run(10);
    assert!(matches!(
        sink.take().as_slice(),
        [DevelopEvent::NotSecured {
            reason: NoFrameReason::NoLiveCameraFrame,
            ..
        }]
    ));
    assert_eq!(rig.engine.diagnostics().picks.no_frame_nothing_shown, 1);

    // Кольцо выключено: сразу `RingDisabled`, не дожидаясь записей.
    let disabled = StreamRig::new(
        RawFrameStreamConfig {
            snapshot_ring_max_frames: 0,
            ..RawFrameStreamConfig::default()
        },
        MAIN,
    );
    let mut rig = engine_rig(StillDevelopConfig::default(), disabled);
    let sink = rig.begin(plan(2, 10));
    assert!(matches!(
        sink.take().as_slice(),
        [DevelopEvent::NotSecured {
            reason: NoFrameReason::RingDisabled,
            ..
        }]
    ));
}

#[test]
fn a_held_picture_on_screen_at_press_is_not_a_live_camera_frame() {
    let mut stream = StreamRig::with_ring(MAIN);
    let frames = stream.offer_many(2);
    let mut rig = engine_rig(StillDevelopConfig::default(), stream);
    let mut recorder = super::feed::StillFeedRecorder::default();
    recorder.note_present(0, super::feed::ShownNow::NewLiveFrame(frames[0]));
    recorder.note_present(1, super::feed::ShownNow::NotLive);
    recorder.note_present(2, super::feed::ShownNow::NotLive);
    rig.engine.ingest_feed(recorder.take());
    rig.engine.ingest_presentations(&[
        PastPresentation {
            present_id: 0,
            frame_index: 0,
            actual_present: ms(0),
        },
        PastPresentation {
            present_id: 1,
            frame_index: 1,
            actual_present: ms(10),
        },
        PastPresentation {
            present_id: 2,
            frame_index: 2,
            actual_present: ms(20),
        },
    ]);
    // Нажатие в 15 мс: на экране показ 1 — удержанная картинка.
    let sink = rig.begin(plan(1, 15));
    rig.run(2);
    assert!(matches!(
        sink.take().as_slice(),
        [DevelopEvent::NotSecured {
            reason: NoFrameReason::NoLiveCameraFrame,
            ..
        }]
    ));
    assert_eq!(rig.engine.diagnostics().picks.no_frame_not_live_on_screen, 1);
}

#[test]
fn dng_plan_delivers_raw_samples_and_a_jpeg_plan_does_not() {
    let (mut rig, _) = proven_rig();
    let with_raw = rig.begin(plan_with_dng(1, 25));
    let without_raw = rig.begin(plan(2, 35));
    rig.run(6);
    match with_raw.take().first() {
        Some(DevelopEvent::Secured { raw: Some(raw), .. }) => {
            assert_eq!(raw.size(), super::test_support::FRAME_SIZE);
            assert_eq!(raw.sample(0, 0), Some(u16::from_le_bytes([7, 7])));
        }
        other => panic!("DNG-план обязан получить сырые отсчёты: {other:?}"),
    }
    match without_raw.take().first() {
        Some(DevelopEvent::Secured { raw: None, .. }) => {}
        other => panic!("JPEG-план без DNG не получает сырых данных: {other:?}"),
    }
}

#[test]
fn readback_target_ends_with_one_readback_event_after_secured() {
    let (mut rig, _) = proven_rig();
    let sink = rig.begin(plan(1, 25));
    rig.run(3);
    assert!(matches!(sink.take().as_slice(), [DevelopEvent::Secured { .. }]));
    let outcome = rig
        .engine
        .apply_command(StillCommand::Target(PrimaryTargetOffer::Ready {
            id: StillId(1),
            target: PrimaryTarget::Readback,
        }));
    assert!(matches!(outcome, StillCommandOutcome::Accepted));
    rig.run(10);
    let events = sink.take();
    match events.as_slice() {
        [DevelopEvent::PrimaryReadback { id, image }] => {
            assert_eq!(*id, StillId(1));
            assert_eq!(image.size(), super::test_support::FRAME_SIZE);
            assert_eq!(image.rgba_bytes()[0], 9, "байты картинки дошли до записи без потерь");
        }
        other => panic!("ожидали ровно PrimaryReadback, получили {other:?}"),
    }
    assert!(rig.engine.diagnostics().developments_completed == 1);
    assert_eq!(rig.engine.queue_len(), 0);
}

#[test]
fn encoder_surface_target_ends_with_one_rendered_event() {
    let (mut rig, _) = proven_rig();
    let sink = rig.begin(plan(1, 25));
    rig.run(3);
    sink.take();
    rig.engine
        .apply_command(StillCommand::Target(PrimaryTargetOffer::Ready {
            id: StillId(1),
            target: PrimaryTarget::EncoderSurface(fake_encoder_surface()),
        }));
    rig.run(10);
    assert!(matches!(
        sink.take().as_slice(),
        [DevelopEvent::PrimaryRendered { id: StillId(1) }]
    ));
    assert_eq!(rig.gpu.lock().unwrap().presents_started, 1);
    assert_eq!(rig.engine.diagnostics().encoder_presents_completed, 1);
}

#[test]
fn target_unavailable_ends_with_the_contract_failure_and_no_second_event() {
    let (mut rig, _) = proven_rig();
    let sink = rig.begin(plan(1, 25));
    rig.run(3);
    sink.take();
    rig.engine
        .apply_command(StillCommand::Target(PrimaryTargetOffer::Unavailable { id: StillId(1) }));
    rig.run(10);
    assert!(matches!(
        sink.take().as_slice(),
        [DevelopEvent::Failed {
            failure: DevelopFailure::TargetUnavailable,
            ..
        }]
    ));
    assert_eq!(rig.gpu.lock().unwrap().developments_started, 0, "проявлять незачем");
}

#[test]
fn a_target_for_an_unknown_or_not_waiting_still_is_handed_back_and_counted() {
    let (mut rig, _) = proven_rig();
    let outcome = rig
        .engine
        .apply_command(StillCommand::Target(PrimaryTargetOffer::Ready {
            id: StillId(99),
            target: PrimaryTarget::Readback,
        }));
    assert!(matches!(
        outcome,
        StillCommandOutcome::Unsupported(StillCommand::Target(_))
    ));
    // Снимок есть, но ещё не захвачен (Secured не было): цель рано.
    let _sink = rig.begin(plan(1, 25));
    let early = rig
        .engine
        .apply_command(StillCommand::Target(PrimaryTargetOffer::Ready {
            id: StillId(1),
            target: PrimaryTarget::Readback,
        }));
    assert!(matches!(early, StillCommandOutcome::Unsupported(_)));
    assert_eq!(rig.engine.diagnostics().targets_unexpected, 2);
}

#[test]
fn the_snapshot_share_is_returned_after_the_copy_and_never_leaked() {
    let (mut rig, _) = proven_rig();
    let sink = rig.begin(plan(1, 25));
    rig.run(3);
    sink.take();
    // Доля снимка возвращена по fence копии (`release_after_gpu_fence`), а не потеряна.
    let counters = rig.stream.stream.counters();
    assert_eq!(counters.released_after_fence, 1, "{counters:?}");
    assert_eq!(counters.leaked_leases, 0, "{counters:?}");
    assert_eq!(rig.gpu.lock().unwrap().released_unused, 0);
}

#[test]
fn time_limits_follow_the_injected_clock_not_the_wall_clock() {
    // Часы теста идут только вручную: без `advance` ни один срок не наступает.
    let (mut rig, _) = proven_rig();
    let sink = rig.begin(plan(1, 55));
    for _ in 0..20 {
        rig.engine.advance();
    }
    assert_eq!(sink.len(), 0, "часы стоят — срок ожидания записей не вышел");
    rig.clock.advance(Duration::from_millis(500));
    rig.run(3);
    assert!(matches!(sink.take().first(), Some(DevelopEvent::Secured { .. })));
}

//! Тесты потери GPU снимков и протокола при сбоях: исход GPU неизвестен — объекты не уничтожаются (утечка с учётом),
//! итог каждому снимку ровно один, следующий снимок получает новый GPU и завершается, `Failed` не бывает без
//! `Secured`, `Begin` без surface получает итог сразу.

use std::sync::Arc;
use std::time::Duration;

use still_contract::develop::{DevelopEvent, PrimaryTarget, PrimaryTargetOffer};
use still_contract::frame_choice::NoFrameReason;
use still_contract::plan::StillId;

use super::test_support::{EngineRig, MAIN, Sink, StreamRig, engine_rig, fake_encoder_surface, ms, plan};
use crate::backend::PastPresentation;
use crate::config::StillDevelopConfig;
use crate::still::StillEventSink;
use crate::still::{StillBegin, StillCommand};
use camera_contract::capture_result::SensorTimestamp;

fn proven_rig(config: StillDevelopConfig) -> EngineRig {
    let mut stream = StreamRig::with_ring(MAIN);
    let frames: Vec<SensorTimestamp> = stream.offer_many(8);
    let mut rig = engine_rig(config, stream);
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

fn offer_encoder_surface(rig: &mut EngineRig, id: u64) {
    rig.engine
        .apply_command(StillCommand::Target(PrimaryTargetOffer::Ready {
            id: StillId(id),
            target: PrimaryTarget::EncoderSurface(fake_encoder_surface()),
        }));
}

fn offer_readback(rig: &mut EngineRig, id: u64) {
    rig.engine
        .apply_command(StillCommand::Target(PrimaryTargetOffer::Ready {
            id: StillId(id),
            target: PrimaryTarget::Readback,
        }));
}

fn last_event(sink: &Sink) -> String {
    sink.0
        .lock()
        .unwrap()
        .last()
        .map(|event| format!("{event:?}"))
        .unwrap_or_default()
}

/// Показ дошёл до `Presenting`: снимок захвачен, проявлен, цель принята.
fn drive_to_presenting(rig: &mut EngineRig, id: u64) {
    rig.run(8);
    offer_encoder_surface(rig, id);
    rig.run(8);
    assert_eq!(rig.gpu.lock().unwrap().presents_started, 1, "показ в кодер запущен");
}

/// Следующий снимок после потери GPU получает новый GPU и доходит до результата (доказывает, что «занято» не
/// залипло и затвор снова работает).
fn assert_next_still_completes_on_a_fresh_gpu(rig: &mut EngineRig) {
    {
        let mut state = rig.gpu.lock().unwrap();
        state.lose_capture = false;
        state.lose_develop = false;
        state.lose_present = false;
        state.capture_forever = false;
        state.develop_forever = false;
        state.present_forever = false;
        state.flight = super::test_support::Flight::default();
    }
    let gpus_before = *rig.made_gpus.lock().unwrap();
    let next = rig.begin(plan(99, 45));
    rig.run(8);
    offer_readback(rig, 99);
    rig.run(8);
    assert_eq!(
        *rig.made_gpus.lock().unwrap(),
        gpus_before + 1,
        "потерянный GPU заменён новым для следующего снимка"
    );
    assert!(
        matches!(next.take().last(), Some(DevelopEvent::PrimaryReadback { .. })),
        "следующий снимок завершился картинкой"
    );
}

#[test]
fn a_stuck_present_does_not_destroy_the_image_under_the_gpu() {
    let config = StillDevelopConfig {
        encoder_present_timeout_ms: 200,
        ..StillDevelopConfig::default()
    };
    let mut rig = proven_rig(config);
    rig.gpu.lock().unwrap().present_forever = true;
    let sink = rig.begin(plan(1, 25));
    drive_to_presenting(&mut rig, 1);
    rig.clock.advance(Duration::from_millis(400));
    rig.run(2);
    assert_eq!(sink.finals_for(StillId(1)), 1);
    assert!(last_event(&sink).contains("Gpu"), "{}", last_event(&sink));
    let state = rig.gpu.lock().unwrap();
    assert_eq!(state.lost_calls, 1, "GPU объявлен потерянным");
    assert_eq!(
        state.events,
        ["gpu_lost"],
        "картинка показа не уничтожена ни до, ни после потери GPU (учтённая утечка)"
    );
    drop(state);
    let diagnostics = rig.engine.diagnostics();
    assert_eq!((diagnostics.gpu_lost, diagnostics.gpu_lost_by_deadline), (1, 1));
}

#[test]
fn a_present_whose_fence_cannot_be_polled_keeps_the_image_and_loses_the_gpu() {
    let mut rig = proven_rig(StillDevelopConfig::default());
    rig.gpu.lock().unwrap().lose_present = true;
    let sink = rig.begin(plan(1, 25));
    rig.run(8);
    offer_encoder_surface(&mut rig, 1);
    rig.run(8);
    assert_eq!(sink.finals_for(StillId(1)), 1);
    assert!(last_event(&sink).contains("Gpu"), "{}", last_event(&sink));
    assert_eq!(rig.gpu.lock().unwrap().events, ["gpu_lost"]);
    assert!(!rig.engine.has_gpu());
    assert_next_still_completes_on_a_fresh_gpu(&mut rig);
}

#[test]
fn unreadable_raw_samples_give_not_secured_and_never_a_failed_without_secured() {
    let mut rig = proven_rig(StillDevelopConfig::default());
    rig.gpu.lock().unwrap().fail_raw_samples = true;
    // План с DNG: сырые отсчёты нужны, чтение провалится.
    let sink = rig.begin(super::test_support::plan_with_dng(1, 25));
    rig.run(8);
    let events = sink.take();
    assert!(
        matches!(
            events.as_slice(),
            [DevelopEvent::NotSecured {
                reason: NoFrameReason::CaptureFailed,
                ..
            }]
        ),
        "единственное событие — NotSecured(CaptureFailed), без Secured и без Failed: {events:?}"
    );
    assert_eq!(rig.engine.diagnostics().refusals.raw_samples_unreadable, 1);
    assert_eq!(rig.engine.queue_len(), 0);
}

#[test]
fn a_capture_whose_fence_cannot_be_polled_loses_the_gpu_leaks_the_lease_and_does_not_stick_busy() {
    let mut rig = proven_rig(StillDevelopConfig::default());
    rig.gpu.lock().unwrap().lose_capture = true;
    let sink = rig.begin(plan(1, 25));
    rig.run(6);
    let events = sink.take();
    assert!(
        matches!(
            events.as_slice(),
            [DevelopEvent::NotSecured {
                reason: NoFrameReason::CaptureFailed,
                ..
            }]
        ),
        "{events:?}"
    );
    assert!(!rig.engine.has_gpu(), "GPU потерян и выброшен");
    {
        let state = rig.gpu.lock().unwrap();
        assert_eq!(
            state.leases_leaked, 1,
            "кадр камеры не возвращён: учтённая утечка лизинга"
        );
        assert_eq!(state.lost_calls, 1);
    }
    assert_eq!(
        rig.stream.stream.counters().leaked_leases,
        1,
        "утечка лизинга видна в счётчике хаба кадров"
    );
    assert_next_still_completes_on_a_fresh_gpu(&mut rig);
}

#[test]
fn a_capture_that_never_completes_is_failed_by_its_deadline_and_the_lease_is_leaked() {
    let config = StillDevelopConfig {
        capture_timeout_ms: 300,
        ..StillDevelopConfig::default()
    };
    let mut rig = proven_rig(config);
    rig.gpu.lock().unwrap().capture_forever = true;
    let sink = rig.begin(plan(1, 25));
    rig.run(4);
    assert_eq!(sink.finals_for(StillId(1)), 0, "срок ещё не вышел: снимок ждёт копию");
    rig.clock.advance(Duration::from_millis(500));
    rig.run(1);
    let events = sink.take();
    assert!(
        matches!(
            events.as_slice(),
            [DevelopEvent::NotSecured {
                reason: NoFrameReason::CaptureFailed,
                ..
            }]
        ),
        "{events:?}"
    );
    assert_eq!(rig.gpu.lock().unwrap().leases_leaked, 1);
    assert_eq!(rig.engine.diagnostics().gpu_lost_by_deadline, 1);
    assert_next_still_completes_on_a_fresh_gpu(&mut rig);
}

#[test]
fn a_development_whose_fence_cannot_be_polled_fails_with_gpu_and_the_next_still_works() {
    let mut rig = proven_rig(StillDevelopConfig::default());
    rig.gpu.lock().unwrap().lose_develop = true;
    let sink = rig.begin(plan(1, 25));
    rig.run(8);
    offer_readback(&mut rig, 1);
    rig.run(4);
    assert_eq!(sink.finals_for(StillId(1)), 1);
    assert!(last_event(&sink).contains("Gpu"), "{}", last_event(&sink));
    assert!(
        sink.0
            .lock()
            .unwrap()
            .first()
            .is_some_and(|event| matches!(event, DevelopEvent::Secured { .. }))
    );
    assert!(!rig.engine.has_gpu());
    assert_next_still_completes_on_a_fresh_gpu(&mut rig);
}

#[test]
fn a_development_that_never_completes_is_failed_by_its_deadline() {
    let config = StillDevelopConfig {
        develop_timeout_ms: 400,
        ..StillDevelopConfig::default()
    };
    let mut rig = proven_rig(config);
    rig.gpu.lock().unwrap().develop_forever = true;
    let sink = rig.begin(plan(1, 25));
    rig.run(8);
    offer_readback(&mut rig, 1);
    rig.run(3);
    assert_eq!(sink.finals_for(StillId(1)), 0, "срок ещё не вышел");
    rig.clock.advance(Duration::from_millis(600));
    rig.run(1);
    assert_eq!(sink.finals_for(StillId(1)), 1);
    assert!(last_event(&sink).contains("Gpu"), "{}", last_event(&sink));
    assert_eq!(rig.engine.diagnostics().gpu_lost_by_deadline, 1);
    assert_next_still_completes_on_a_fresh_gpu(&mut rig);
}

#[test]
fn other_stills_with_gpu_objects_get_one_final_each_when_the_gpu_is_lost_and_waiting_ones_survive() {
    let mut rig = proven_rig(StillDevelopConfig::default());
    rig.gpu.lock().unwrap().lose_develop = true;
    let developing = rig.begin(plan(1, 25));
    let awaiting_target = rig.begin(plan(2, 35));
    rig.run(8);
    offer_readback(&mut rig, 1);
    rig.run(4);
    assert_eq!(developing.finals_for(StillId(1)), 1);
    assert_eq!(
        awaiting_target.finals_for(StillId(2)),
        1,
        "захваченный на потерянном GPU получил итог"
    );
    assert!(
        last_event(&awaiting_target).contains("Gpu"),
        "{}",
        last_event(&awaiting_target)
    );
    assert_eq!(rig.engine.queue_len(), 0);
}

#[test]
fn begin_without_a_surface_gets_renderer_stopped_at_once_and_exactly_one_final() {
    let mut rig = proven_rig(StillDevelopConfig::default());
    let sink = Arc::new(Sink::default());
    let events: Arc<dyn StillEventSink> = Arc::clone(&sink) as Arc<dyn StillEventSink>;
    rig.engine.refuse_begin_without_surface(StillBegin {
        plan: plan(5, 25),
        events,
    });
    let delivered = sink.take();
    assert!(
        matches!(
            delivered.as_slice(),
            [DevelopEvent::NotSecured {
                reason: NoFrameReason::RendererStopped,
                ..
            }]
        ),
        "{delivered:?}"
    );
    assert_eq!(
        rig.engine.queue_len(),
        0,
        "снимок не встал в очередь до следующего attach"
    );
    let diagnostics = rig.engine.diagnostics();
    assert_eq!((diagnostics.begins, diagnostics.refusals.begin_without_surface), (1, 1));
    assert_eq!(diagnostics.outcomes.total(), 1);
}

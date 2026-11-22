//! Тесты очереди снимков: ёмкость, «один проявляется за раз», полосы на кадр, сроки цели, ровно один итог при
//! любом пути, брошенные снимки и утечки.

use std::time::Duration;

use camera_contract::capture_result::SensorTimestamp;
use still_contract::develop::{DevelopEvent, DevelopFailure, PrimaryTarget, PrimaryTargetOffer};
use still_contract::frame_choice::NoFrameReason;
use still_contract::plan::StillId;

use super::test_support::{EngineRig, MAIN, Sink, StreamRig, engine_rig, fake_encoder_surface, ms, plan};
use crate::backend::PastPresentation;
use crate::config::StillDevelopConfig;
use crate::still::StillCommand;
use std::sync::Arc;

fn proven_rig(config: StillDevelopConfig) -> (EngineRig, Vec<SensorTimestamp>) {
    let mut stream = StreamRig::with_ring(MAIN);
    let frames = stream.offer_many(8);
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
    (rig, frames)
}

fn offer_readback(rig: &mut EngineRig, id: u64) {
    rig.engine
        .apply_command(StillCommand::Target(PrimaryTargetOffer::Ready {
            id: StillId(id),
            target: PrimaryTarget::Readback,
        }));
}

#[test]
fn ten_presses_in_a_row_give_ten_results_exactly_once_each() {
    let (mut rig, _) = proven_rig(StillDevelopConfig::default());
    let sinks: Vec<Arc<Sink>> = (1..=10)
        .map(|id| rig.begin(plan(id, 15 + i64::try_from(id).unwrap_or(0))))
        .collect();
    // Каждому снимку записи тут же дают цель, как только он «захвачен».
    let mut offered = [false; 10];
    for _ in 0..200 {
        rig.run(1);
        for (index, sink) in sinks.iter().enumerate() {
            let secured = sink
                .0
                .lock()
                .unwrap()
                .iter()
                .any(|event| matches!(event, DevelopEvent::Secured { .. }));
            if secured && !offered[index] {
                offered[index] = true;
                offer_readback(&mut rig, index as u64 + 1);
            }
        }
        if sinks
            .iter()
            .all(|sink| sink.0.lock().unwrap().iter().any(DevelopEvent::is_final))
        {
            break;
        }
    }
    for (index, sink) in sinks.iter().enumerate() {
        let id = index as u64 + 1;
        assert_eq!(sink.finals_for(StillId(id)), 1, "снимок {id}: ровно один итог");
        assert!(
            matches!(
                sink.0.lock().unwrap().last(),
                Some(DevelopEvent::PrimaryReadback { .. })
            ),
            "снимок {id}: итог — картинка"
        );
    }
    let diagnostics = rig.engine.diagnostics();
    assert_eq!(diagnostics.begins, 10);
    assert_eq!(diagnostics.outcomes.total(), 10, "итогов столько же, сколько принято");
    assert_eq!(diagnostics.outcomes.readback, 10);
    assert_eq!(rig.engine.queue_len(), 0);
    // Копии шли по одной (поддельный GPU падает, если движок запустил вторую при занятой первой).
    assert_eq!(rig.gpu.lock().unwrap().captured.len(), 10);
    assert_eq!(rig.stream.stream.counters().leaked_leases, 0);
}

#[test]
fn the_queue_is_bounded_and_the_refused_press_gets_its_own_final_without_hurting_the_others() {
    let config = StillDevelopConfig {
        queue_capacity: 2,
        ..StillDevelopConfig::default()
    };
    let (mut rig, _) = proven_rig(config);
    let first = rig.begin(plan(1, 25));
    let second = rig.begin(plan(2, 35));
    let third = rig.begin(plan(3, 45));
    assert!(matches!(
        third.take().as_slice(),
        [DevelopEvent::NotSecured {
            reason: NoFrameReason::RendererBusy,
            ..
        }]
    ));
    assert_eq!(rig.engine.diagnostics().refusals.queue_full, 1);
    assert_eq!(rig.engine.queue_len(), 2, "принятые снимки не тронуты");
    rig.run(6);
    for sink in [&first, &second] {
        assert!(matches!(sink.take().first(), Some(DevelopEvent::Secured { .. })));
    }
}

#[test]
fn only_one_still_develops_at_a_time_and_slices_per_frame_are_limited() {
    let config = StillDevelopConfig {
        slices_per_frame: 2,
        ..StillDevelopConfig::default()
    };
    let (mut rig, _) = proven_rig(config);
    rig.gpu.lock().unwrap().slices_total = 5;
    let sinks = [rig.begin(plan(1, 25)), rig.begin(plan(2, 35))];
    rig.run(8);
    offer_readback(&mut rig, 1);
    offer_readback(&mut rig, 2);
    rig.run(1);
    for _ in 0..40 {
        rig.run(1);
        // «Один проявляется за раз»: поддельный GPU падает на втором `develop_start` при идущем первом.
        let state = rig.gpu.lock().unwrap();
        assert!(
            state.slices_per_call.iter().all(|slices| *slices <= 2),
            "не больше полос на кадр, чем в конфиге"
        );
    }
    for (index, sink) in sinks.iter().enumerate() {
        assert_eq!(sink.finals_for(StillId(index as u64 + 1)), 1);
    }
    let state = rig.gpu.lock().unwrap();
    assert_eq!(state.developments_started, 2);
    // 5 полос по 2 за вызов: 2 + 2 + 1.
    assert!(
        state.slices_per_call.starts_with(&[2, 2, 1]),
        "{:?}",
        state.slices_per_call
    );
    drop(state);
    assert_eq!(rig.engine.diagnostics().slices_submitted, 10);
}

#[test]
fn a_target_that_never_comes_times_out_with_one_failure() {
    let config = StillDevelopConfig {
        target_timeout_ms: 100,
        ..StillDevelopConfig::default()
    };
    let (mut rig, _) = proven_rig(config);
    let sink = rig.begin(plan(1, 25));
    rig.run(3);
    assert!(matches!(sink.take().as_slice(), [DevelopEvent::Secured { .. }]));
    rig.clock.advance(Duration::from_millis(70));
    rig.engine.advance();
    assert_eq!(sink.len(), 0, "срок ещё не вышел");
    rig.clock.advance(Duration::from_millis(40));
    rig.engine.advance();
    assert!(matches!(
        sink.take().as_slice(),
        [DevelopEvent::Failed {
            failure: DevelopFailure::TargetTimedOut,
            ..
        }]
    ));
    // Запоздавшая цель уже никому не нужна: возвращена потоку, второго итога нет.
    let late = rig
        .engine
        .apply_command(StillCommand::Target(PrimaryTargetOffer::Ready {
            id: StillId(1),
            target: PrimaryTarget::Readback,
        }));
    assert!(matches!(late, crate::still::StillCommandOutcome::Unsupported(_)));
    assert_eq!(sink.len(), 0);
}

#[test]
fn gpu_failures_end_with_exactly_one_event_on_every_stage() {
    // Копия не запустилась / сорвалась: до `Secured` → NotSecured.
    for (fail_start, fail_capture) in [(true, false), (false, true)] {
        let (mut rig, _) = proven_rig(StillDevelopConfig::default());
        {
            let mut state = rig.gpu.lock().unwrap();
            state.fail_capture_start = fail_start;
            state.fail_capture = fail_capture;
        }
        let sink = rig.begin(plan(1, 25));
        rig.run(4);
        assert!(
            matches!(sink.take().as_slice(), [DevelopEvent::NotSecured { .. }]),
            "start={fail_start} capture={fail_capture}"
        );
        assert_eq!(rig.engine.queue_len(), 0);
        assert_eq!(rig.stream.stream.counters().leaked_leases, 0);
    }

    // Проявление сорвалось: после `Secured` → Failed(Gpu).
    let (mut rig, _) = proven_rig(StillDevelopConfig::default());
    rig.gpu.lock().unwrap().fail_develop = true;
    let sink = rig.begin(plan(1, 25));
    rig.run(3);
    offer_readback(&mut rig, 1);
    rig.run(4);
    let events = sink.take();
    assert!(matches!(events.first(), Some(DevelopEvent::Secured { .. })));
    assert!(matches!(
        events.last(),
        Some(DevelopEvent::Failed {
            failure: DevelopFailure::Gpu(_),
            ..
        })
    ));
    assert_eq!(events.iter().filter(|event| event.is_final()).count(), 1);

    // Поверхность кодера отвергнута показом.
    let (mut rig, _) = proven_rig(StillDevelopConfig::default());
    rig.gpu.lock().unwrap().fail_present_start = true;
    let sink = rig.begin(plan(1, 25));
    rig.run(3);
    rig.engine
        .apply_command(StillCommand::Target(PrimaryTargetOffer::Ready {
            id: StillId(1),
            target: PrimaryTarget::EncoderSurface(fake_encoder_surface()),
        }));
    rig.run(10);
    assert!(matches!(
        sink.take().last(),
        Some(DevelopEvent::Failed {
            failure: DevelopFailure::Gpu(_),
            ..
        })
    ));
}

#[test]
fn an_encoder_attach_failing_later_on_poll_ends_like_a_refused_start_and_spares_the_next_still() {
    // Правило 6: с мастерской (решение 144) отказ подключения цели приходит не из `present_start`, а из опроса.
    // Итог снимка обязан быть тем же (`Failed(Gpu)`, ровно один), GPU не объявляется потерянным, следующий
    // снимок проходит.
    let (mut rig, _) = proven_rig(StillDevelopConfig::default());
    rig.gpu.lock().unwrap().fail_present_on_poll = true;
    let failing = rig.begin(plan(1, 25));
    rig.run(3);
    rig.engine
        .apply_command(StillCommand::Target(PrimaryTargetOffer::Ready {
            id: StillId(1),
            target: PrimaryTarget::EncoderSurface(fake_encoder_surface()),
        }));
    rig.run(10);
    let events = failing.take();
    assert!(matches!(
        events.last(),
        Some(DevelopEvent::Failed {
            failure: DevelopFailure::Gpu(_),
            ..
        })
    ));
    assert_eq!(events.iter().filter(|event| event.is_final()).count(), 1);
    let diagnostics = rig.engine.diagnostics();
    assert_eq!(diagnostics.gpu_lost, 0, "сбой подключения — не потеря GPU");
    assert_eq!(diagnostics.encoder_presents_completed, 0);
    assert_eq!(*rig.made_gpus.lock().unwrap(), 1, "GPU снимков не пересоздавался");

    rig.gpu.lock().unwrap().fail_present_on_poll = false;
    let next = rig.begin(plan(2, 35));
    rig.run(3);
    rig.engine
        .apply_command(StillCommand::Target(PrimaryTargetOffer::Ready {
            id: StillId(2),
            target: PrimaryTarget::EncoderSurface(fake_encoder_surface()),
        }));
    rig.run(10);
    assert!(matches!(
        next.take().last(),
        Some(DevelopEvent::PrimaryRendered { id: StillId(2) })
    ));
}

#[test]
fn a_stuck_encoder_present_fails_that_still_and_resets_the_gpu_for_the_rest() {
    let config = StillDevelopConfig {
        encoder_present_timeout_ms: 200,
        ..StillDevelopConfig::default()
    };
    let (mut rig, _) = proven_rig(config);
    rig.gpu.lock().unwrap().present_forever = true;
    let stuck = rig.begin(plan(1, 25));
    let waiting = rig.begin(plan(2, 35));
    rig.run(8);
    rig.engine
        .apply_command(StillCommand::Target(PrimaryTargetOffer::Ready {
            id: StillId(1),
            target: PrimaryTarget::EncoderSurface(fake_encoder_surface()),
        }));
    rig.run(6);
    rig.clock.advance(Duration::from_millis(400));
    rig.run(2);
    assert_eq!(stuck.finals_for(StillId(1)), 1);
    assert!(matches!(
        stuck.take().last(),
        Some(DevelopEvent::Failed {
            failure: DevelopFailure::Gpu(_),
            ..
        })
    ));
    // Второй снимок был захвачен, цель не пришла: его копия RAW жила на потерянном GPU, поэтому итог — сбой GPU
    // (объекты оставлены в утечку), а не «брошен».
    assert_eq!(waiting.finals_for(StillId(2)), 1);
    assert!(matches!(
        waiting.take().last(),
        Some(DevelopEvent::Failed {
            failure: DevelopFailure::Gpu(_),
            ..
        })
    ));
    assert!(!rig.engine.has_gpu(), "потерянный GPU выброшен целиком");
    assert_eq!(rig.engine.queue_len(), 0);
}

#[test]
fn dropping_the_engine_delivers_finals_and_a_busy_gpu_is_leaked_not_destroyed() {
    let (mut rig, _) = proven_rig(StillDevelopConfig::default());
    rig.gpu.lock().unwrap().abandon_idle = false;
    let sink = rig.begin(plan(1, 25));
    rig.run(3);
    let state = Arc::clone(&rig.gpu);
    let leaked_before = rig.engine.diagnostics().gpu_leaked_on_abandon;
    assert_eq!(leaked_before, 0);
    drop(rig);
    assert_eq!(sink.finals_for(StillId(1)), 1, "итог доставлен при уничтожении движка");
    assert_eq!(state.lock().unwrap().abandon_calls, 1);
}

#[test]
fn the_gpu_is_prepared_before_the_press_not_at_the_shutter() {
    let (mut rig, _) = proven_rig(StillDevelopConfig::default());
    assert_eq!(
        *rig.made_gpus.lock().unwrap(),
        1,
        "GPU готовится заранее по первому обработанному кадру"
    );
    let sink = rig.begin(plan(1, 25));
    rig.run(3);
    assert_eq!(
        *rig.made_gpus.lock().unwrap(),
        1,
        "нажатие не создаёт GPU заново: он уже готов"
    );
    assert!(matches!(sink.take().first(), Some(DevelopEvent::Secured { .. })));
}

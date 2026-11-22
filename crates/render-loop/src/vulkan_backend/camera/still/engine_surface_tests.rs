//! Тесты жизни движка снимков без surface (сессия 14e, решения 146 и 150): захваченное доводится до итога,
//! не захваченное получает честный отказ, отсоединение не ждёт GPU, опустевшая очередь без экрана отпускает GPU.

use std::sync::Arc;

use camera_contract::capture_result::SensorTimestamp;
use still_contract::develop::{DevelopEvent, DevelopFailure, PrimaryTarget, PrimaryTargetOffer};
use still_contract::frame_choice::NoFrameReason;
use still_contract::plan::StillId;

use super::feed::{FrameStillParams, ShownNow, StillFeedRecorder};
use super::test_support::{EngineRig, MAIN, Sink, StreamRig, engine_rig, ms, plan};
use crate::backend::PastPresentation;
use crate::config::StillDevelopConfig;
use crate::still::{StillCommand, StillWorkload};

/// Движок с лентой видоискателя (8 кадров) и записями драйвера каждые 10 мс: нажатие в 25 мс доказуемо.
fn proven_rig() -> (EngineRig, Vec<SensorTimestamp>) {
    let mut stream = StreamRig::with_ring(MAIN);
    let frames = stream.offer_many(8);
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
    (rig, frames)
}

fn offer_readback(rig: &mut EngineRig, id: u64) {
    rig.engine
        .apply_command(StillCommand::Target(PrimaryTargetOffer::Ready {
            id: StillId(id),
            target: PrimaryTarget::Readback,
        }));
}

/// Последнее событие снимка в виде текста (для сообщений и проверки вида итога).
fn last_event(sink: &Sink) -> Option<String> {
    sink.0.lock().unwrap().last().map(|event| format!("{event:?}"))
}

fn ended_with_readback(sink: &Sink, id: u64) -> bool {
    let events = sink.0.lock().unwrap();
    matches!(events.first(), Some(DevelopEvent::Secured { .. }))
        && matches!(events.last(), Some(DevelopEvent::PrimaryReadback { id: got, .. }) if *got == StillId(id))
}

#[test]
fn detach_keeps_captured_stills_and_refuses_only_those_that_still_need_the_screen() {
    let (mut rig, _) = proven_rig();
    // 1: захвачен, цели ещё нет; 3: проявляется; 2: нажат прямо перед отсоединением — нажатие позже всех записей
    // драйвера, снимок ещё ждёт доказательства «что было на экране» (срок `presentation_wait_ms` не истёк).
    let waiting_target = rig.begin(plan(1, 25));
    rig.run(3);
    let developing = rig.begin(plan(3, 35));
    rig.run(3);
    offer_readback(&mut rig, 3);
    rig.run(2);
    assert_eq!(
        rig.gpu.lock().unwrap().developments_started,
        1,
        "третий снимок проявляется"
    );
    let choosing = rig.begin(plan(2, 75));
    let abandons_before = rig.gpu.lock().unwrap().abandon_calls;

    rig.engine.on_detach();

    // Не захваченный — один честный отказ сразу; захваченные — ни одного итога, путь продолжается.
    assert_eq!(choosing.finals_for(StillId(2)), 1);
    assert!(matches!(
        choosing.take().last(),
        Some(DevelopEvent::NotSecured {
            reason: NoFrameReason::RendererStopped,
            ..
        })
    ));
    assert_eq!(
        waiting_target.finals_for(StillId(1)),
        0,
        "{:?}",
        last_event(&waiting_target)
    );
    assert_eq!(developing.finals_for(StillId(3)), 0, "{:?}", last_event(&developing));
    assert_eq!(rig.engine.workload(), StillWorkload::Unfinished);
    assert!(
        rig.engine.has_gpu(),
        "GPU захваченных снимков не разбирается вместе с экраном"
    );
    assert_eq!(
        rig.gpu.lock().unwrap().abandon_calls,
        abandons_before,
        "отсоединение не ждёт GPU снимков (бюджет квитанции)"
    );

    // Без экрана: цель приходит, шаги идут по таймеру, лент видоискателя больше нет.
    offer_readback(&mut rig, 1);
    rig.run(20);

    assert_eq!(waiting_target.finals_for(StillId(1)), 1);
    assert_eq!(developing.finals_for(StillId(3)), 1);
    assert!(
        ended_with_readback(&waiting_target, 1),
        "{:?}",
        last_event(&waiting_target)
    );
    assert!(ended_with_readback(&developing, 3), "{:?}", last_event(&developing));
    assert_eq!(rig.engine.workload(), StillWorkload::Idle);
    assert!(
        !rig.engine.has_gpu(),
        "без экрана опустевшая очередь отпускает GPU снимков"
    );
    let diagnostics = rig.engine.diagnostics();
    assert_eq!(diagnostics.continued_without_surface, 2);
    assert_eq!(diagnostics.gpu_released_without_surface, 1);
    assert_eq!(diagnostics.abandoned_after_capture, 0);
    assert_eq!(diagnostics.abandoned_before_capture, 1);
    assert_eq!(diagnostics.outcomes.readback, 2);
    assert_eq!(diagnostics.outcomes.failed_abandoned, 0);
    assert_eq!(rig.stream.stream.counters().leaked_leases, 0);
}

#[test]
fn a_raw_copy_in_flight_at_detach_finishes_and_is_saved_without_the_screen() {
    let (mut rig, frames) = proven_rig();
    rig.gpu.lock().unwrap().capture_polls_needed = 5;
    let sink = rig.begin(plan(1, 25));
    rig.run(1);
    assert!(rig.gpu.lock().unwrap().flight.capture, "копия RAW в полёте");
    assert_eq!(sink.len(), 0);

    rig.engine.on_detach();
    assert_eq!(sink.finals_for(StillId(1)), 0, "копию в полёте не бросаем");
    rig.run(8);
    assert!(
        matches!(sink.0.lock().unwrap().first(), Some(DevelopEvent::Secured { .. })),
        "{:?}",
        last_event(&sink)
    );
    offer_readback(&mut rig, 1);
    rig.run(10);

    assert_eq!(sink.finals_for(StillId(1)), 1);
    assert!(ended_with_readback(&sink, 1), "{:?}", last_event(&sink));
    // Кадр камеры вернулся после fence копии, а не утёк вместе с экраном.
    assert_eq!(rig.stream.stream.counters().leaked_leases, 0);
    assert_eq!(rig.gpu.lock().unwrap().captured.len(), 1);
    assert!(frames.contains(&rig.gpu.lock().unwrap().captured[0]));
}

#[test]
fn reattach_with_a_fresh_stream_passport_mid_development_keeps_the_gpu_and_finishes() {
    let (mut rig, _) = proven_rig();
    let sink = rig.begin(plan(1, 25));
    rig.run(3);
    offer_readback(&mut rig, 1);
    rig.run(1);
    assert_eq!(rig.gpu.lock().unwrap().developments_started, 1);
    rig.engine.on_detach();
    rig.run(1);

    // Новая surface — новая сессия видоискателя и новый `Arc` паспорта с тем же содержимым (после возврата из фона).
    let fresh_passport = Arc::new((*rig.stream.passport).clone());
    let mut recorder = StillFeedRecorder::default();
    let next_frame = rig.stream.offer();
    recorder.note_frame_params(
        next_frame,
        FrameStillParams {
            stream: Arc::clone(&fresh_passport),
            ..rig.stream.params_for()
        },
    );
    recorder.note_present(100, ShownNow::NewLiveFrame(next_frame));
    rig.engine.ingest_feed(recorder.take());
    rig.run(10);

    assert_eq!(sink.finals_for(StillId(1)), 1);
    assert!(ended_with_readback(&sink, 1), "{:?}", last_event(&sink));
    assert_eq!(
        *rig.made_gpus.lock().unwrap(),
        1,
        "GPU под живым проявлением не пересоздаётся новой сессией видоискателя"
    );
    assert!(
        rig.engine.has_gpu(),
        "с экраном GPU держится готовым к следующему нажатию"
    );
    assert_eq!(rig.engine.diagnostics().gpu_released_without_surface, 0);
}

#[test]
fn detach_with_an_empty_queue_releases_the_gpu_at_once() {
    let (mut rig, _) = proven_rig();
    assert!(rig.engine.has_gpu(), "GPU подготовлен заранее по ленте");
    rig.engine.on_detach();
    assert!(!rig.engine.has_gpu());
    assert_eq!(
        rig.gpu.lock().unwrap().abandon_calls,
        1,
        "простаивающий GPU разобран остановкой"
    );
    assert_eq!(rig.engine.diagnostics().gpu_released_without_surface, 1);
    assert_eq!(rig.engine.workload(), StillWorkload::Idle);
}

#[test]
fn stopping_after_detach_still_gives_every_captured_still_exactly_one_final() {
    let (mut rig, _) = proven_rig();
    let sink = rig.begin(plan(1, 25));
    rig.run(3);
    rig.engine.on_detach();
    assert_eq!(sink.finals_for(StillId(1)), 0);
    // Поток рендера неисправен или остановлен: `abandon_stills` → `abandon_all`.
    rig.engine.abandon_all();
    assert_eq!(sink.finals_for(StillId(1)), 1);
    assert!(matches!(
        sink.take().last(),
        Some(DevelopEvent::Failed {
            failure: DevelopFailure::Abandoned,
            ..
        })
    ));
    assert_eq!(rig.engine.workload(), StillWorkload::Idle);
    drop(rig);
    assert_eq!(sink.finals_for(StillId(1)), 0, "уничтожение движка не дублирует итог");
}

#[test]
fn workload_is_unfinished_from_the_press_until_the_final() {
    let (mut rig, _) = proven_rig();
    assert_eq!(rig.engine.workload(), StillWorkload::Idle);
    let sink = rig.begin(plan(1, 25));
    assert_eq!(rig.engine.workload(), StillWorkload::Unfinished);
    rig.run(3);
    offer_readback(&mut rig, 1);
    rig.run(10);
    assert_eq!(sink.finals_for(StillId(1)), 1);
    assert_eq!(rig.engine.workload(), StillWorkload::Idle);
}

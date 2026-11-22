//! Жизненный цикл снимков на настоящем бэкенде: отсоединение посреди проявления (снимок доводится без экрана,
//! сессия 14e), уничтожение бэкенда, вытесненный из кольца кадр и HEIF-путь (картинка в поверхность кодера,
//! `PresentationSurface::Headless` на ПК).

#![cfg(not(target_os = "android"))]
#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть сигнал о провале"
)]

mod continuity_rig;
mod still_rig;
mod support;
mod vulkan_support;

use std::sync::Arc;

use camera_contract::geometry::PixelSize;
use isp_vulkan::photo::PhotoConfig;
use render_loop::backend::DisplayBackend;
use render_loop::config::StillDevelopConfig;
use render_loop::still::{StillCommand, StillCommandOutcome, StillWorkload};
use still_contract::develop::{DevelopEvent, DevelopFailure, PrimaryTarget, PrimaryTargetOffer};
use still_contract::frame_choice::{FrameChoice, NoFrameReason, SubstitutionReason};
use still_contract::plan::StillId;
use still_rig::{HEIF, JPEG, StillBench, plan, press_during, setup};
use support::{Journal, fake_surface};

fn offer(bench: &mut StillBench, id: u64, target: PrimaryTarget) {
    let outcome = bench
        .rig
        .backend
        .apply_still_command(StillCommand::Target(PrimaryTargetOffer::Ready {
            id: StillId(id),
            target,
        }));
    assert!(matches!(outcome, StillCommandOutcome::Accepted), "{outcome:?}");
}

fn sliced() -> StillDevelopConfig {
    StillDevelopConfig {
        photo: PhotoConfig {
            slice_rows: 16,
            ..PhotoConfig::default()
        },
        ..StillDevelopConfig::default()
    }
}

/// Сколько шагов снимков без экрана ждать итога (поток рендера шагает по таймеру; здесь — вручную с паузой).
const STEPS_WITHOUT_SURFACE: usize = 2_000;

#[test]
fn detach_in_the_middle_of_development_keeps_the_heif_going_to_the_encoder_and_leaks_nothing() {
    let Some(mut bench) = setup(sliced(), 12) else {
        return;
    };
    bench.frames(10);
    bench.inject_timings();
    let developing = bench.begin(plan(1, press_during(3), HEIF));
    assert!(bench.frames_until(30, || developing.has_secured()));
    let journal = Arc::new(Journal::default());
    offer(&mut bench, 1, PrimaryTarget::EncoderSurface(fake_surface(&journal, 77)));
    // Проявление началось, но не закончилось.
    for _ in 0..30 {
        if bench
            .rig
            .backend
            .still_diagnostics()
            .is_some_and(|d| d.slices_submitted > 0)
        {
            break;
        }
        bench.frame();
    }
    assert!(
        bench
            .rig
            .backend
            .still_diagnostics()
            .is_some_and(|d| d.slices_submitted > 0)
    );
    assert_eq!(developing.finals(), 0, "проявление ещё идёт");
    // Нажат прямо перед пропажей экрана: ещё доказывает, какой кадр был на экране, — ему экран нужен.
    let choosing = bench.begin(plan(2, press_during(9), JPEG));

    assert!(bench.rig.backend.detach().is_some(), "окно возвращено");
    assert_eq!(developing.finals(), 0, "захваченный снимок не брошен вместе с экраном");
    assert_eq!(choosing.finals(), 1, "не захваченный — ровно один итог сразу");
    assert!(
        matches!(
            choosing.events().last(),
            Some(DevelopEvent::NotSecured {
                reason: NoFrameReason::RendererStopped,
                ..
            })
        ),
        "{:?}",
        choosing.events()
    );
    assert_eq!(bench.rig.backend.still_workload(), StillWorkload::Unfinished);

    // Экрана нет, кадров видоискателя нет: шаги — как у потока рендера по таймеру.
    for _ in 0..STEPS_WITHOUT_SURFACE {
        if developing.finals() == 1 {
            break;
        }
        bench.rig.backend.advance_stills();
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert_eq!(developing.finals(), 1, "{:?}", developing.events());
    assert!(
        matches!(
            developing.events().last(),
            Some(DevelopEvent::PrimaryRendered { id: StillId(1) })
        ),
        "снимок нарисован в поверхность кодера без экрана: {:?}",
        developing.events().last()
    );
    assert_eq!(bench.rig.backend.still_workload(), StillWorkload::Idle);
    let still = bench.rig.backend.still_diagnostics().expect("снимки");
    assert_eq!(
        (still.encoder_presents_started, still.encoder_presents_completed),
        (1, 1)
    );
    assert_eq!(still.continued_without_surface, 1);
    assert_eq!(
        still.gpu_released_without_surface, 1,
        "опустевшая очередь без экрана отпустила GPU"
    );
    assert_eq!(still.outcomes.failed_abandoned, 0);
    assert_eq!(still.gpu_leaked_on_abandon, 0);
    developing.events().clear();
    choosing.events().clear();
    bench.rig.stream.producer().end_stream(bench.epoch);
    let counters = bench.rig.stream.counters();
    assert_eq!(counters.leaked_leases, 0, "{counters:?}");
    assert_eq!(counters.frames_outstanding, 0, "{counters:?}");
    assert_eq!(
        bench.rig.backend.context().live_objects().total(),
        bench.rig.baseline.total(),
        "без экрана и без снимков объектов GPU снимков нет"
    );
}

#[test]
fn dropping_the_backend_delivers_a_final_to_every_unfinished_still() {
    let Some(mut bench) = setup(sliced(), 12) else {
        return;
    };
    bench.frames(10);
    bench.inject_timings();
    let waiting_target = bench.begin(plan(1, press_during(3), JPEG));
    assert!(bench.frames_until(30, || waiting_target.has_secured()));
    let choosing = bench.begin(plan(2, press_during(9), JPEG));
    let counters_source = bench.rig.stream.counters();
    assert_eq!(counters_source.leaked_leases, 0);
    drop(bench.rig);
    assert_eq!(waiting_target.finals(), 1);
    assert_eq!(choosing.finals(), 1);
    assert!(matches!(
        waiting_target.events().last(),
        Some(DevelopEvent::Failed {
            failure: DevelopFailure::Abandoned,
            ..
        })
    ));
}

#[test]
fn a_frame_pushed_out_of_the_ring_is_replaced_with_the_evicted_mark() {
    let Some(mut bench) = setup(StillDevelopConfig::default(), 3) else {
        return;
    };
    bench.frames(12);
    bench.inject_timings();
    // Показ №2 давно вытеснен из кольца в три кадра: снимок берёт ближайший оставшийся и говорит об этом.
    let sink = bench.begin(plan(1, press_during(2), JPEG));
    assert!(bench.frames_until(30, || sink.has_secured()));
    match sink.events().first() {
        Some(DevelopEvent::Secured { facts, .. }) => match facts.choice {
            FrameChoice::Substituted {
                reason,
                wanted,
                sensor_timestamp,
                ..
            } => {
                assert_eq!(reason, SubstitutionReason::ShownFrameEvicted);
                assert!(
                    wanted.is_some_and(|wanted| wanted < sensor_timestamp),
                    "хотели более ранний кадр"
                );
            }
            other => panic!("ожидали замену: {other:?}"),
        },
        other => panic!("{other:?}"),
    }
    let still = bench.rig.backend.still_diagnostics().expect("снимки");
    assert_eq!((still.picks.exact, still.picks.substituted_evicted), (0, 1));
    offer(&mut bench, 1, PrimaryTarget::Readback);
    assert!(bench.frames_until(60, || sink.finals() == 1));
    sink.events().clear();
    continuity_rig::finish(&mut bench.rig, &[bench.epoch]);
}

#[test]
fn heif_path_draws_into_the_encoder_surface_and_returns_every_gpu_object() {
    let Some(mut bench) = setup(sliced(), 12) else {
        return;
    };
    bench.frames(10);
    bench.inject_timings();
    let sink = bench.begin(plan(1, press_during(3), HEIF));
    assert!(bench.frames_until(30, || sink.has_secured()));
    let journal = Arc::new(Journal::default());
    offer(&mut bench, 1, PrimaryTarget::EncoderSurface(fake_surface(&journal, 77)));
    assert!(bench.frames_until(80, || sink.finals() == 1), "{:?}", sink.events());
    assert!(
        matches!(
            sink.events().last(),
            Some(DevelopEvent::PrimaryRendered { id: StillId(1) })
        ),
        "{:?}",
        sink.events().last()
    );
    let still = bench.rig.backend.still_diagnostics().expect("снимки");
    assert_eq!(
        (still.encoder_presents_started, still.encoder_presents_completed),
        (1, 1)
    );
    sink.events().clear();
    continuity_rig::finish(&mut bench.rig, &[bench.epoch]);
    assert_eq!(
        bench.rig.backend.context().live_objects().total(),
        bench.rig.baseline.total(),
        "цель кодера и объекты проявления освобождены"
    );
    let _ = (PixelSize::new(1, 1), NoFrameReason::RingEmpty);
}

#[test]
fn begin_without_an_attached_surface_gets_renderer_stopped_at_once() {
    let Some(mut bench) = setup(sliced(), 12) else {
        return;
    };
    bench.frames(10);
    bench.inject_timings();
    assert!(bench.rig.backend.detach().is_some(), "окно возвращено");
    // Surface нет, кадров показа больше не будет: итог обязан прийти сразу, а не «когда-нибудь после attach».
    let sink = bench.begin(plan(1, press_during(3), JPEG));
    assert_eq!(sink.finals(), 1, "ровно один итог сразу");
    assert!(
        matches!(
            sink.events().as_slice(),
            [DevelopEvent::NotSecured {
                reason: NoFrameReason::RendererStopped,
                ..
            }]
        ),
        "{:?}",
        sink.events()
    );
    let still = bench.rig.backend.still_diagnostics().expect("снимки");
    assert_eq!(still.refusals.begin_without_surface, 1);
    sink.events().clear();
}

#[test]
fn a_second_shutter_during_development_still_gets_a_submit_slot_for_its_capture() {
    // Много полос на кадр, но в полёте их не больше одной: остальные слоты очереди работ зарезервированы под
    // копию RAW следующего затвора и показ в кодер.
    let config = StillDevelopConfig {
        slices_per_frame: 64,
        max_slices_in_flight: 1,
        ..sliced()
    };
    let Some(mut bench) = setup(config, 12) else {
        return;
    };
    bench.frames(10);
    bench.inject_timings();
    let first = bench.begin(plan(1, press_during(3), JPEG));
    assert!(bench.frames_until(30, || first.has_secured()));
    offer(&mut bench, 1, PrimaryTarget::Readback);
    // Кадр, на котором проявление отправляет первые полосы.
    for _ in 0..30 {
        if bench
            .rig
            .backend
            .still_diagnostics()
            .is_some_and(|d| d.slices_submitted > 0)
        {
            break;
        }
        bench.frame();
    }
    let after_first_step = bench.rig.backend.still_diagnostics().expect("снимки");
    assert_eq!(
        after_first_step.slices_submitted, 1,
        "в полёте не больше `max_slices_in_flight`, хотя на кадр разрешено 64"
    );
    assert_eq!(first.finals(), 0, "проявление первого ещё идёт");
    // Второй затвор прямо во время проявления первого.
    let second = bench.begin(plan(2, press_during(8), JPEG));
    assert!(bench.frames_until(40, || second.has_secured()), "{:?}", second.events());
    let still = bench.rig.backend.still_diagnostics().expect("снимки");
    assert_eq!(
        still.refusals.capture_start_failed, 0,
        "слот под копию не был занят полосами"
    );
    offer(&mut bench, 2, PrimaryTarget::Readback);
    assert!(bench.frames_until(120, || first.finals() == 1 && second.finals() == 1));
    for sink in [&first, &second] {
        assert!(matches!(
            sink.events().last(),
            Some(DevelopEvent::PrimaryReadback { .. })
        ));
    }
    first.events().clear();
    second.events().clear();
    continuity_rig::finish(&mut bench.rig, &[bench.epoch]);
}

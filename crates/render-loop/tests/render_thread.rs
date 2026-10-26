//! Функциональные тесты потока рендера: настоящий поток `fasticamera-render`, fake-бэкенд,
//! fake-часы и fake-команды. Проверяется поведение целиком: от события в ящике до кадра
//! на «экране» и до подтверждённой квитанции.

#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть сигнал о провале"
)]

mod support;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use render_loop::backend::PresentOutcome;
use render_loop::config::RenderLoopConfig;
use render_loop::mailbox::InboxDelivery;
use render_loop::pacing::PacingVerdict;
use render_loop::thread::{FaultKind, RenderLoop, RenderLoopHandle, RenderLoopParts, RenderState, SpawnError};
use shell_contract::commands::FrameRateOutcome;
use shell_contract::surface::{SurfaceGeneration, SurfaceReleaseOutcome, SurfaceSize, surface_release_handoff};
use support::{
    FakeBackend, FakeClock, FakeCommands, Journal, OBJECTS_PER_ATTACH, Probe, SIZE, fake_surface, wait_until,
};

/// Предел ожидания в тестах: большой, чтобы не мигать на медленном CI, но конечный.
const DEADLINE: Duration = Duration::from_secs(5);

struct Rig {
    handle: RenderLoopHandle,
    probe: Arc<Probe>,
    journal: Arc<Journal>,
    commands: Arc<FakeCommands>,
    clock_stop: Arc<std::sync::atomic::AtomicBool>,
}

fn start_rig(configure: impl FnOnce(&Probe)) -> Rig {
    let journal = Arc::new(Journal::default());
    let probe = Probe::new(Arc::clone(&journal));
    configure(&probe);
    let (clock, clock_stop) = FakeClock::new();
    let commands = Arc::new(FakeCommands::default());
    let handle = RenderLoop::spawn(RenderLoopParts {
        config: RenderLoopConfig {
            // Fake показывает кадры по 120 Гц; по умолчанию просим 60.
            preferred_frame_rate_hz: 120.0,
            ..RenderLoopConfig::default()
        },
        backend: FakeBackend::new(Arc::clone(&probe)),
        clock,
        commands: commands.clone(),
    })
    .expect("поток запускается");
    Rig {
        handle,
        probe,
        journal,
        commands,
        clock_stop,
    }
}

impl Rig {
    fn show_surface(&self, generation: u64) {
        let inbox = self.handle.inbox();
        assert_eq!(
            inbox.surface_created(fake_surface(&self.journal, generation)),
            InboxDelivery::Delivered
        );
        inbox.surface_changed(SurfaceGeneration::new(generation), SIZE);
    }

    fn wait_for_frames(&self, at_least: u64) {
        assert!(
            wait_until(DEADLINE, || self.handle.status().frames_presented >= at_least),
            "кадры не пошли: {:?}",
            self.handle.status()
        );
    }

    /// Уничтожает surface и возвращает исход ожидания квитанции и время ожидания.
    fn destroy_surface(&self, generation: u64) -> (SurfaceReleaseOutcome, Duration) {
        let (request, waiter) = surface_release_handoff(SurfaceGeneration::new(generation));
        let started = Instant::now();
        self.handle.inbox().surface_destroyed(request);
        let outcome = waiter.wait(DEADLINE);
        (outcome, started.elapsed())
    }
}

#[test]
fn created_and_changed_surface_gets_frames_with_increasing_indices() {
    let rig = start_rig(|_| {});
    rig.show_surface(1);
    rig.wait_for_frames(10);

    let frames = rig.probe.rendered_frames();
    assert!(frames.len() >= 10);
    assert!(
        frames.windows(2).all(|pair| pair[1] == pair[0] + 1),
        "индексы кадров идут подряд: {frames:?}"
    );
    assert_eq!(frames[0], 0);
    let status = rig.handle.status();
    assert_eq!(status.state, RenderState::Presenting);
    assert_eq!(status.generation, Some(SurfaceGeneration::new(1)));
    assert_eq!(status.surfaces_attached, 1);
    assert_eq!(rig.probe.live_objects(), OBJECTS_PER_ATTACH);
    assert_eq!(
        status.backend.live_objects_by_kind,
        BTreeMap::from([("Fake".to_owned(), OBJECTS_PER_ATTACH)]),
        "перепись по видам доходит до статуса"
    );
}

#[test]
fn backend_is_driven_from_named_render_thread_and_frame_rate_is_requested_once() {
    let rig = start_rig(|_| {});
    rig.show_surface(1);
    rig.wait_for_frames(1);

    assert_eq!(
        rig.probe.attach_thread_names.lock().expect("имена").as_slice(),
        ["fasticamera-render"]
    );
    assert_eq!(
        rig.commands.requested_rates.lock().expect("частоты").as_slice(),
        [120.0]
    );
    assert_eq!(
        rig.handle.status().frame_rate_request,
        Some(FrameRateOutcome::DeferredUntilSurface)
    );
}

#[test]
fn pacing_is_fed_from_backend_presentations() {
    let rig = start_rig(|_| {});
    rig.show_surface(1);
    assert!(
        wait_until(DEADLINE, || rig.handle.status().pacing.intervals >= 10),
        "пейсинг не набрал интервалов: {:?}",
        rig.handle.status().pacing
    );
    let pacing = rig.handle.status().pacing;
    assert_eq!(pacing.misses, 0, "fake показывает ровно по 120 Гц");
    // Окно 10 с не набралось за тест — честный вердикт «рано судить».
    assert_eq!(pacing.verdict, PacingVerdict::NotEnoughData);
}

#[test]
fn known_validation_deviations_reach_status_without_being_errors() {
    let rig = start_rig(|probe| {
        probe
            .known_deviations
            .lock()
            .expect("отходы")
            .insert("VUID-fake-00001".to_owned(), 7);
    });
    rig.show_surface(1);
    rig.wait_for_frames(3);
    assert!(
        wait_until(DEADLINE, || rig
            .handle
            .status()
            .backend
            .known_deviations
            .get("VUID-fake-00001")
            == Some(&7)),
        "счётчик отхода не дошёл до статуса: {:?}",
        rig.handle.status().backend
    );
    assert_eq!(rig.handle.status().backend.validation_errors, 0, "отход не ошибка");
}

#[test]
fn destroy_confirms_only_after_detach_and_window_release() {
    let rig = start_rig(|probe| probe.detach_delay_ms.store(60, Ordering::SeqCst));
    rig.show_surface(1);
    rig.wait_for_frames(3);

    let (outcome, _) = rig.destroy_surface(1);
    assert_eq!(outcome, SurfaceReleaseOutcome::Confirmed);
    // В момент, когда оболочка получила подтверждение, всё уже должно быть отпущено.
    let journal = &rig.journal;
    let detach_done = journal
        .position("detach_done(1)")
        .expect("detach завершён до подтверждения");
    let window_released = journal
        .position("window_released(1)")
        .expect("окно отпущено до подтверждения");
    assert!(
        detach_done < window_released,
        "сначала swapchain, потом окно: {:?}",
        journal.entries()
    );
    assert_eq!(rig.probe.live_objects(), 0);
    let status = rig.handle.status();
    assert_eq!(status.state, RenderState::WaitingForSurface);
    assert_eq!(status.releases_confirmed, 1);
    assert!(
        status.backend.live_objects_by_kind.is_empty(),
        "к моменту подтверждения в статусе перепись в покое: {:?}",
        status.backend.live_objects_by_kind
    );
    assert!(status.last_release_latency.expect("задержка измерена") >= Duration::from_millis(60));
    let frames_after = rig.probe.rendered_frames().len();
    std::thread::sleep(Duration::from_millis(30));
    assert_eq!(
        rig.probe.rendered_frames().len(),
        frames_after,
        "после destroy кадров нет"
    );
}

#[test]
fn release_while_idle_is_confirmed_fast() {
    let rig = start_rig(|_| {});
    // Поток спит без surface; квитанция чужого/неизвестного поколения.
    std::thread::sleep(Duration::from_millis(50));
    let (outcome, waited) = rig.destroy_surface(42);
    assert_eq!(outcome, SurfaceReleaseOutcome::Confirmed);
    assert!(waited < Duration::from_millis(400), "ожидание заняло {waited:?}");

    // То же после уже отработавшей surface: поток снова спит.
    rig.show_surface(1);
    rig.wait_for_frames(1);
    let (first, _) = rig.destroy_surface(1);
    assert_eq!(first, SurfaceReleaseOutcome::Confirmed);
    std::thread::sleep(Duration::from_millis(50));
    let (idle, waited) = rig.destroy_surface(1);
    assert_eq!(idle, SurfaceReleaseOutcome::Confirmed);
    assert!(waited < Duration::from_millis(400), "ожидание заняло {waited:?}");
}

#[test]
fn second_created_detaches_old_surface_before_attaching_new_one() {
    let rig = start_rig(|_| {});
    rig.show_surface(1);
    rig.wait_for_frames(2);
    rig.show_surface(2);
    assert!(wait_until(DEADLINE, || rig
        .journal
        .position("attach(2,1440x3216)")
        .is_some()));

    let entries = rig.journal.entries();
    let detach_old = rig.journal.position("detach_done(1)").expect("старая отсоединена");
    let attach_new = rig.journal.position("attach(2,1440x3216)").expect("новая присоединена");
    assert!(detach_old < attach_new, "порядок событий: {entries:?}");
    // Запоздавшая квитанция старой surface подтверждается сразу и не трогает новую.
    let (outcome, _) = rig.destroy_surface(1);
    assert_eq!(outcome, SurfaceReleaseOutcome::Confirmed);
    assert_eq!(rig.handle.status().generation, Some(SurfaceGeneration::new(2)));
    assert_eq!(rig.probe.live_objects(), OBJECTS_PER_ATTACH);
}

#[test]
fn size_change_recreates_swapchain_and_frames_continue() {
    let rig = start_rig(|_| {});
    rig.show_surface(1);
    rig.wait_for_frames(2);
    let landscape = SurfaceSize {
        width_px: 3216,
        height_px: 1440,
    };
    rig.handle.inbox().surface_changed(SurfaceGeneration::new(1), landscape);
    assert!(wait_until(DEADLINE, || rig.handle.status().recreations == 1));
    assert_eq!(rig.probe.resizes.lock().expect("размеры").as_slice(), [landscape]);
    let presented = rig.handle.status().frames_presented;
    rig.wait_for_frames(presented + 3);
}

#[test]
fn stale_size_change_is_ignored_and_counted() {
    let rig = start_rig(|_| {});
    rig.show_surface(2);
    rig.wait_for_frames(1);
    rig.handle.inbox().surface_changed(SurfaceGeneration::new(1), SIZE);
    assert!(wait_until(DEADLINE, || rig.handle.status().stale_changes == 1));
    assert!(
        rig.probe.resizes.lock().expect("размеры").is_empty(),
        "чужой размер не применён"
    );
}

#[test]
fn needs_recreate_outcome_triggers_resize_with_current_size() {
    let rig = start_rig(|probe| {
        probe
            .scripted_outcomes
            .lock()
            .expect("исходы")
            .push_back(PresentOutcome::NeedsRecreate);
    });
    rig.show_surface(1);
    assert!(wait_until(DEADLINE, || rig.handle.status().recreations == 1));
    assert_eq!(rig.probe.resizes.lock().expect("размеры").as_slice(), [SIZE]);
    rig.wait_for_frames(3);
}

#[test]
fn surface_lost_detaches_and_destroy_still_confirms() {
    let rig = start_rig(|probe| {
        probe
            .scripted_outcomes
            .lock()
            .expect("исходы")
            .push_back(PresentOutcome::SurfaceLost);
    });
    rig.show_surface(1);
    assert!(wait_until(DEADLINE, || rig.handle.status().state == RenderState::SurfaceUnusable));
    assert!(rig.journal.position("detach_done(1)").is_some(), "бэкенд освобождён");
    assert_eq!(rig.probe.live_objects(), 0);

    let (outcome, _) = rig.destroy_surface(1);
    assert_eq!(outcome, SurfaceReleaseOutcome::Confirmed);
    assert_eq!(rig.handle.status().state, RenderState::WaitingForSurface);
}

#[test]
fn acquire_timeout_is_counted_without_error() {
    let rig = start_rig(|probe| {
        probe
            .scripted_outcomes
            .lock()
            .expect("исходы")
            .extend([PresentOutcome::TimedOut, PresentOutcome::TimedOut]);
    });
    rig.show_surface(1);
    rig.wait_for_frames(3);
    let status = rig.handle.status();
    assert_eq!(status.acquire_timeouts, 2);
    assert_eq!(status.backend_errors, 0);
    assert_eq!(status.state, RenderState::Presenting);
}

#[test]
fn attach_failure_is_reported_and_destroy_confirms_without_detach() {
    let rig = start_rig(|probe| probe.fail_attach.store(true, Ordering::SeqCst));
    rig.show_surface(1);
    assert!(wait_until(DEADLINE, || rig.handle.status().state == RenderState::SurfaceUnusable));
    let status = rig.handle.status();
    assert_eq!(status.backend_errors, 1);
    assert!(status.last_backend_error.expect("текст ошибки").contains("attach"));
    assert!(rig.probe.rendered_frames().is_empty());

    let (outcome, _) = rig.destroy_surface(1);
    assert_eq!(outcome, SurfaceReleaseOutcome::Confirmed);
    assert_eq!(rig.journal.count("detach_started(1)"), 0);
}

#[test]
fn pause_stops_frames_and_resume_restarts_them() {
    let rig = start_rig(|_| {});
    rig.show_surface(1);
    rig.wait_for_frames(3);
    rig.handle.inbox().paused();
    assert!(wait_until(DEADLINE, || rig.handle.status().state == RenderState::Paused));
    let frozen = rig.probe.rendered_frames().len();
    std::thread::sleep(Duration::from_millis(60));
    assert_eq!(rig.probe.rendered_frames().len(), frozen, "на паузе кадров нет");
    assert_eq!(
        rig.probe.live_objects(),
        OBJECTS_PER_ATTACH,
        "surface на паузе остаётся присоединённой"
    );

    rig.handle.inbox().resumed();
    assert!(wait_until(DEADLINE, || rig.probe.rendered_frames().len() > frozen + 2));
}

#[test]
fn backend_panic_on_frame_faults_thread_but_release_is_still_confirmed() {
    let rig = start_rig(|probe| probe.panic_on_frame.store(5, Ordering::SeqCst));
    rig.show_surface(1);
    assert!(wait_until(DEADLINE, || rig.handle.status().state == RenderState::Faulted));
    let status = rig.handle.status();
    let fault = status.fault.expect("неисправность описана");
    assert_eq!(fault.kind, FaultKind::BackendPanic);
    assert!(
        fault.message.contains("boom"),
        "сообщение паники сохранено: {}",
        fault.message
    );

    let (outcome, waited) = rig.destroy_surface(1);
    assert_eq!(outcome, SurfaceReleaseOutcome::Confirmed);
    assert!(waited < Duration::from_millis(400));
    assert_eq!(rig.handle.status().state, RenderState::Faulted);

    // Новая surface в неисправном потоке не удерживается: окно отпущено сразу.
    rig.show_surface(2);
    assert!(wait_until(DEADLINE, || rig
        .journal
        .position("window_released(2)")
        .is_some()));
    assert!(rig.journal.position("attach(2,1440x3216)").is_none());
}

#[test]
fn backend_panic_in_detach_still_confirms_waiter() {
    let rig = start_rig(|_| {});
    rig.show_surface(1);
    rig.wait_for_frames(2);
    rig.probe.panic_on_detach.store(true, Ordering::SeqCst);

    let (outcome, _) = rig.destroy_surface(1);
    assert!(
        matches!(
            outcome,
            SurfaceReleaseOutcome::Confirmed | SurfaceReleaseOutcome::DroppedWithoutConfirmation
        ),
        "ожидание не должно зависнуть: {outcome:?}"
    );
    assert_eq!(rig.handle.status().state, RenderState::Faulted);
}

#[test]
fn stopped_clock_faults_thread_and_release_is_confirmed() {
    let rig = start_rig(|_| {});
    rig.show_surface(1);
    rig.wait_for_frames(2);
    rig.clock_stop.store(true, Ordering::SeqCst);
    assert!(wait_until(DEADLINE, || rig.handle.status().state == RenderState::Faulted));
    assert_eq!(
        rig.handle.status().fault.expect("описана").kind,
        FaultKind::ClockStopped
    );
    assert_eq!(rig.destroy_surface(1).0, SurfaceReleaseOutcome::Confirmed);
}

#[test]
fn twenty_create_destroy_cycles_leave_no_live_objects_and_confirm_every_waiter() {
    let rig = start_rig(|_| {});
    let baseline = rig.probe.live_objects();
    for generation in 1..=20 {
        rig.show_surface(generation);
        assert!(
            wait_until(DEADLINE, || rig.handle.status().generation
                == Some(SurfaceGeneration::new(generation))
                && rig.handle.status().state == RenderState::Presenting),
            "цикл {generation}: показ не начался"
        );
        let (outcome, _) = rig.destroy_surface(generation);
        assert_eq!(outcome, SurfaceReleaseOutcome::Confirmed, "цикл {generation}");
    }
    assert_eq!(rig.probe.live_objects(), baseline);
    let status = rig.handle.status();
    assert_eq!(status.releases_confirmed, 20);
    assert_eq!(status.surfaces_attached, 20);
    assert_eq!(
        status.backend.live_objects, baseline,
        "диагностика бэкенда тоже видит возврат"
    );
    assert_eq!(rig.journal.count("window_released(1)"), 1);
    assert_eq!(rig.journal.count("window_released(20)"), 1);
}

#[test]
fn stop_detaches_held_surface_and_later_release_is_dropped_immediately() {
    let rig = start_rig(|_| {});
    rig.show_surface(1);
    rig.wait_for_frames(2);
    let inbox = rig.handle.inbox();
    let journal = Arc::clone(&rig.journal);
    let probe = Arc::clone(&rig.probe);

    let final_status = rig.handle.stop();

    assert_eq!(final_status.state, RenderState::Stopped);
    assert!(journal.position("detach_done(1)").is_some());
    assert!(journal.position("window_released(1)").is_some());
    assert_eq!(probe.live_objects(), 0);

    // Потока нет: квитанция не должна ждать таймаут.
    let (request, waiter) = surface_release_handoff(SurfaceGeneration::new(1));
    assert_eq!(inbox.surface_destroyed(request), InboxDelivery::ReceiverGone);
    assert_eq!(
        waiter.wait(Duration::from_secs(3600)),
        SurfaceReleaseOutcome::DroppedWithoutConfirmation
    );
}

#[test]
fn dropping_handle_stops_thread_and_releases_surface() {
    let rig = start_rig(|_| {});
    rig.show_surface(1);
    rig.wait_for_frames(1);
    let Rig {
        handle, journal, probe, ..
    } = rig;
    drop(handle);
    assert!(journal.position("detach_done(1)").is_some());
    assert_eq!(probe.live_objects(), 0);
}

#[test]
fn invalid_config_is_rejected_before_thread_starts() {
    let journal = Arc::new(Journal::default());
    let (clock, _stop) = FakeClock::new();
    let config = RenderLoopConfig {
        frames_in_flight: 0,
        ..RenderLoopConfig::default()
    };
    let result = RenderLoop::spawn(RenderLoopParts {
        config,
        backend: FakeBackend::new(Probe::new(journal)),
        clock,
        commands: Arc::new(FakeCommands::default()),
    });
    assert!(matches!(result, Err(SpawnError::InvalidConfig(_))));
}

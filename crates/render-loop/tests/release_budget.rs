//! Бюджет квитанции освобождения surface и согласованность паузы с ящиком.
//!
//! Оболочка в `surfaceDestroyed` ждёт квитанцию ограниченное время (500 мс). Поток рендера
//! не должен складывать несколько ожиданий GPU: уничтожение, уже лежащее в ящике, обязано
//! пропускать пересоздание цепочки и новый кадр (acquire), а detach — единственное оставшееся
//! ожидание. Тут же — потеря паузы при схлопывании `Resumed` + `Paused` в ящике.

#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть сигнал о провале"
)]

mod support;

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use render_loop::backend::PresentOutcome;
use render_loop::config::RenderLoopConfig;
use render_loop::thread::{FrameClock, RenderLoop, RenderLoopHandle, RenderLoopParts, RenderState};
use shell_contract::surface::{SurfaceGeneration, SurfaceReleaseOutcome, SurfaceSize, surface_release_handoff};
use support::{
    FakeBackend, FakeClock, FakeCommands, GateState, GatedClock, Journal, Probe, SIZE, fake_surface, wait_until,
};

const DEADLINE: Duration = Duration::from_secs(5);

/// Медленное пересоздание цепочки (ожидание GPU), заведомо дороже любого допустимого ответа.
const SLOW_RESIZE_MS: u64 = 400;

/// Предел ответа на уничтожение: с запасом ниже `SLOW_RESIZE_MS`, но выше шума планировщика.
const FAST_RELEASE: Duration = Duration::from_millis(250);

const OTHER_SIZE: SurfaceSize = SurfaceSize {
    width_px: 1080,
    height_px: 2400,
};

struct Rig {
    handle: RenderLoopHandle,
    probe: Arc<Probe>,
    journal: Arc<Journal>,
}

fn start_rig<C: FrameClock>(clock: C, configure: impl FnOnce(&Probe)) -> Rig {
    let journal = Arc::new(Journal::default());
    let probe = Probe::new(Arc::clone(&journal));
    configure(&probe);
    let handle = RenderLoop::spawn(RenderLoopParts {
        config: RenderLoopConfig::default(),
        backend: FakeBackend::new(Arc::clone(&probe)),
        clock,
        commands: Arc::new(FakeCommands::default()),
    })
    .expect("поток запускается");
    Rig { handle, probe, journal }
}

impl Rig {
    fn show_surface(&self, generation: u64) {
        let inbox = self.handle.inbox();
        inbox.surface_created(fake_surface(&self.journal, generation));
        inbox.surface_changed(SurfaceGeneration::new(generation), SIZE);
    }

    fn wait_for_frames(&self, at_least: u64) {
        assert!(
            wait_until(DEADLINE, || self.handle.status().frames_presented >= at_least),
            "кадры не пошли: {:?}",
            self.handle.status()
        );
    }

    /// Удерживает поток внутри кадра: события, положенные теперь, окажутся в одном пакете.
    fn hold_worker_inside_a_frame(&self) {
        self.probe.hold_frames.store(true, Ordering::SeqCst);
        assert!(
            wait_until(DEADLINE, || self.probe.frame_in_progress.load(Ordering::SeqCst)),
            "поток не вошёл в кадр"
        );
    }

    fn release_worker(&self) {
        self.probe.hold_frames.store(false, Ordering::SeqCst);
    }
}

#[test]
fn pending_destroy_skips_the_slow_resize_and_is_confirmed_with_a_single_wait() {
    let (clock, _stop) = FakeClock::new();
    let rig = start_rig(clock, |probe| {
        probe.resize_delay_ms.store(SLOW_RESIZE_MS, Ordering::SeqCst)
    });
    rig.show_surface(1);
    rig.wait_for_frames(3);

    rig.hold_worker_inside_a_frame();
    let inbox = rig.handle.inbox();
    inbox.surface_changed(SurfaceGeneration::new(1), OTHER_SIZE);
    let (request, waiter) = surface_release_handoff(SurfaceGeneration::new(1));
    let sent_at = Instant::now();
    inbox.surface_destroyed(request);
    rig.release_worker();

    assert_eq!(waiter.wait(DEADLINE), SurfaceReleaseOutcome::Confirmed);
    let waited = sent_at.elapsed();
    assert!(
        waited < FAST_RELEASE,
        "квитанция пришла за {waited:?}: медленное пересоздание ({SLOW_RESIZE_MS} мс) не пропущено"
    );
    assert!(
        rig.probe.resizes.lock().expect("размеры").is_empty(),
        "пересоздание для уходящей surface не выполняется"
    );
    assert_eq!(rig.journal.count("detach_done(1)"), 1, "единственное ожидание — detach");
    assert_eq!(rig.probe.live_objects(), 0, "GPU-объекты освобождены");
}

#[test]
fn resize_without_pending_destroy_is_still_executed() {
    // Контрольный случай: без уничтожения в пакете пересоздание ВЫПОЛНЯЕТСЯ (пропуск не общий).
    let (clock, _stop) = FakeClock::new();
    let rig = start_rig(clock, |_| {});
    rig.show_surface(1);
    rig.wait_for_frames(3);
    rig.handle
        .inbox()
        .surface_changed(SurfaceGeneration::new(1), OTHER_SIZE);
    assert!(wait_until(DEADLINE, || !rig
        .probe
        .resizes
        .lock()
        .expect("размеры")
        .is_empty()));
    assert_eq!(*rig.probe.resizes.lock().expect("размеры"), [OTHER_SIZE]);
}

#[test]
fn needs_recreate_with_destroy_in_mailbox_does_not_recreate() {
    let (clock, _stop) = FakeClock::new();
    let rig = start_rig(clock, |probe| {
        probe.resize_delay_ms.store(SLOW_RESIZE_MS, Ordering::SeqCst)
    });
    rig.show_surface(1);
    rig.wait_for_frames(3);

    // Следующий кадр вернёт NeedsRecreate, пока уничтожение уже лежит в ящике.
    rig.probe
        .scripted_outcomes
        .lock()
        .expect("исходы")
        .push_back(PresentOutcome::NeedsRecreate);
    rig.hold_worker_inside_a_frame();
    let (request, waiter) = surface_release_handoff(SurfaceGeneration::new(1));
    let sent_at = Instant::now();
    rig.handle.inbox().surface_destroyed(request);
    rig.release_worker();

    assert_eq!(waiter.wait(DEADLINE), SurfaceReleaseOutcome::Confirmed);
    assert!(
        sent_at.elapsed() < FAST_RELEASE,
        "ждали пересоздание: {:?}",
        sent_at.elapsed()
    );
    assert!(
        rig.probe.resizes.lock().expect("размеры").is_empty(),
        "самозапрошенное пересоздание пропущено"
    );
}

#[test]
fn destroy_arriving_while_waiting_for_a_tick_prevents_starting_a_new_frame() {
    let (clock, gate): (GatedClock, Arc<GateState>) = GatedClock::new();
    let rig = start_rig(clock, |_| {});
    rig.show_surface(1);
    // Первый тик выдаём, чтобы поток присоединил surface и дошёл до ожидания второго.
    gate.closed.store(false, Ordering::SeqCst);
    rig.wait_for_frames(1);
    gate.closed.store(true, Ordering::SeqCst);
    assert!(
        wait_until(DEADLINE, || gate.waiting.load(Ordering::SeqCst)),
        "поток ждёт тик"
    );
    let frames_before = rig.probe.rendered_frames().len();

    let (request, waiter) = surface_release_handoff(SurfaceGeneration::new(1));
    rig.handle.inbox().surface_destroyed(request);
    // Тик приходит ПОСЛЕ уничтожения. Если кадр всё же начнётся, он «зависнет» в бэкенде
    // (удержание) и квитанция не придёт — как долгий acquire, съедающий бюджет.
    rig.probe.hold_frames.store(true, Ordering::SeqCst);
    gate.closed.store(false, Ordering::SeqCst);

    let outcome = waiter.wait(Duration::from_millis(300));
    rig.probe.hold_frames.store(false, Ordering::SeqCst);
    assert_eq!(
        outcome,
        SurfaceReleaseOutcome::Confirmed,
        "поток увяз в кадре вместо подтверждения квитанции"
    );
    assert_eq!(rig.probe.rendered_frames().len(), frames_before, "новый кадр не начат");
}

#[test]
fn resumed_then_paused_delivered_while_worker_is_busy_still_stops_presentation() {
    let (clock, _stop) = FakeClock::new();
    let rig = start_rig(clock, |_| {});
    rig.show_surface(1);
    rig.wait_for_frames(3);

    // Поток не на паузе; пока он занят кадром, приходят Resumed и сразу Paused.
    rig.hold_worker_inside_a_frame();
    let inbox = rig.handle.inbox();
    inbox.resumed();
    inbox.paused();
    rig.release_worker();

    assert!(
        wait_until(DEADLINE, || rig.handle.status().state == RenderState::Paused),
        "итоговая пауза потеряна: {:?}",
        rig.handle.status().state
    );
    let frozen = rig.probe.rendered_frames().len();
    std::thread::sleep(Duration::from_millis(60));
    assert_eq!(rig.probe.rendered_frames().len(), frozen, "на паузе кадров нет");
}

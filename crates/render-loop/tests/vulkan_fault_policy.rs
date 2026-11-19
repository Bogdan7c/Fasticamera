//! Политика потока рендера при отказах бэкенда (fake-бэкенд, настоящий поток): учёт неудачных
//! `detach` на пути неисправности, видимость итога detach в статусе к моменту квитанции
//! и пауза между повторными пересозданиями цепочки.
//!
//! Файл относится к семейству `vulkan_*`: именно Vulkan-бэкенд даёт `detach_failures` (утечка
//! swapchain/surface при зависшем GPU) и откладывает пересоздание при нулевом размере окна.

#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть сигнал о провале"
)]

mod support;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread;
use std::time::Duration;

use render_loop::backend::{
    AttachError, BackendDiagnostics, BackendError, DisplayBackend, FrameTiming, PastPresentation, PresentOutcome,
};
use render_loop::config::RenderLoopConfig;
use render_loop::thread::{RenderLoop, RenderLoopHandle, RenderLoopParts, RenderState};
use shell_contract::surface::{
    NativeSurface, SurfaceGeneration, SurfaceReleaseOutcome, SurfaceSize, surface_release_handoff,
};
use support::{FakeBackend, FakeClock, FakeCommands, Journal, Probe, SIZE, fake_surface, wait_until};

const DEADLINE: Duration = Duration::from_secs(5);

/// Бэкенд, чей `detach` считается неудачным (как у Vulkan-бэкенда при зависшем GPU): окно
/// возвращает, но Vulkan-объекты «утекли», и это отражено в `detach_failures`.
struct LeakyDetachBackend {
    inner: FakeBackend,
    detach_failures: Arc<AtomicU64>,
    count_failure_on_detach: Arc<AtomicBool>,
    /// Сколько спать внутри `attach`, мс (0 — не спать). Тест растягивает работу потока
    /// ПОСЛЕ подтверждения квитанции, чтобы порядок «статус, затем квитанция» стал проверяемым.
    attach_delay_ms: Arc<AtomicU64>,
}

impl DisplayBackend for LeakyDetachBackend {
    fn attach(&mut self, surface: NativeSurface, size: SurfaceSize) -> Result<(), AttachError> {
        let delay_ms = self.attach_delay_ms.load(Ordering::SeqCst);
        if delay_ms > 0 {
            thread::sleep(Duration::from_millis(delay_ms));
        }
        self.inner.attach(surface, size)
    }

    fn resize(&mut self, size: SurfaceSize) -> Result<(), BackendError> {
        self.inner.resize(size)
    }

    fn render_and_present(&mut self, timing: FrameTiming) -> PresentOutcome {
        self.inner.render_and_present(timing)
    }

    fn detach(&mut self) -> Option<NativeSurface> {
        let surface = self.inner.detach();
        if surface.is_some() && self.count_failure_on_detach.load(Ordering::SeqCst) {
            self.detach_failures.fetch_add(1, Ordering::SeqCst);
        }
        surface
    }

    fn drain_past_presentations(&mut self) -> Vec<PastPresentation> {
        self.inner.drain_past_presentations()
    }

    fn diagnostics(&mut self) -> BackendDiagnostics {
        let mut diagnostics = self.inner.diagnostics();
        diagnostics.detach_failures = self.detach_failures.load(Ordering::SeqCst);
        diagnostics
    }
}

struct Rig {
    handle: RenderLoopHandle,
    probe: Arc<Probe>,
    journal: Arc<Journal>,
}

fn start(config: RenderLoopConfig, probe_setup: impl FnOnce(&Probe)) -> Rig {
    start_with_backend(config, probe_setup, |inner| LeakyDetachBackend {
        inner,
        detach_failures: Arc::new(AtomicU64::new(0)),
        count_failure_on_detach: Arc::new(AtomicBool::new(false)),
        attach_delay_ms: Arc::new(AtomicU64::new(0)),
    })
}

fn start_with_backend(
    config: RenderLoopConfig,
    probe_setup: impl FnOnce(&Probe),
    wrap: impl FnOnce(FakeBackend) -> LeakyDetachBackend,
) -> Rig {
    let journal = Arc::new(Journal::default());
    let probe = Probe::new(Arc::clone(&journal));
    probe_setup(&probe);
    let (clock, _stop) = FakeClock::new();
    let handle = RenderLoop::spawn(RenderLoopParts {
        config,
        backend: wrap(FakeBackend::new(Arc::clone(&probe))),
        clock,
        commands: Arc::new(FakeCommands::default()),
        performance_hints: std::sync::Arc::new(shell_contract::performance_hint::NoPerformanceHints::new(
            "тест без ADPF",
        )),
    })
    .expect("поток запускается");
    Rig { handle, probe, journal }
}

impl Rig {
    fn show_surface(&self, generation: u64) {
        let inbox = self.handle.inbox();
        let _ = inbox.surface_created(fake_surface(&self.journal, generation));
        inbox.surface_changed(SurfaceGeneration::new(generation), SIZE);
    }

    fn destroy_surface(&self, generation: u64) -> SurfaceReleaseOutcome {
        let (request, waiter) = surface_release_handoff(SurfaceGeneration::new(generation));
        self.handle.inbox().surface_destroyed(request);
        waiter.wait(DEADLINE)
    }
}

#[test]
fn detach_failure_on_the_fault_path_is_counted_and_the_receipt_is_still_confirmed() {
    let leaky_flag = Arc::new(AtomicBool::new(true));
    let failures = Arc::new(AtomicU64::new(0));
    let (flag, counter) = (Arc::clone(&leaky_flag), Arc::clone(&failures));
    let rig = start_with_backend(
        RenderLoopConfig::default(),
        |probe| probe.panic_on_frame.store(3, Ordering::SeqCst),
        move |inner| LeakyDetachBackend {
            inner,
            detach_failures: counter,
            count_failure_on_detach: flag,
            attach_delay_ms: Arc::new(AtomicU64::new(0)),
        },
    );
    rig.show_surface(1);
    assert!(
        wait_until(DEADLINE, || rig.handle.status().state == RenderState::Faulted),
        "поток не перешёл в Faulted: {:?}",
        rig.handle.status()
    );

    // Паника на кадре -> enter_fault -> detach бэкенда с утечкой: статус обязан это показать.
    assert_eq!(rig.handle.status().backend.detach_failures, 1);
    // Политика подтверждения не меняется: оболочка не должна зависнуть.
    assert_eq!(rig.destroy_surface(1), SurfaceReleaseOutcome::Confirmed);
}

#[test]
fn panicking_detach_on_the_fault_path_counts_as_a_detach_failure() {
    let rig = start(RenderLoopConfig::default(), |probe| {
        probe.panic_on_frame.store(2, Ordering::SeqCst);
        probe.panic_on_detach.store(true, Ordering::SeqCst);
    });
    rig.show_surface(1);
    assert!(wait_until(DEADLINE, || rig.handle.status().state == RenderState::Faulted));

    assert_eq!(
        rig.handle.status().backend.detach_failures,
        1,
        "упавший detach — тоже неудачный: объекты GPU могли остаться"
    );
    assert_eq!(rig.destroy_surface(1), SurfaceReleaseOutcome::Confirmed);
}

/// Инвариант: тот, кого разбудила квитанция освобождения, уже видит в статусе итог detach
/// (`detach_failures`) и само подтверждение (`releases_confirmed`). Раньше статус
/// публиковался только в конце итерации, ПОСЛЕ квитанции, и тест плавал под нагрузкой CI.
///
/// Порядок делаем детерминированным: поток держим внутри кадра, кладём в ящик одним пакетом
/// «уничтожить surface 1» и «создать surface 2», а `attach` второй surface растягиваем на
/// 300 мс. Без публикации до квитанции статус обновился бы только после медленного attach.
#[test]
fn detach_failure_on_the_normal_path_is_visible_in_the_status_when_the_receipt_is_confirmed() {
    let flag = Arc::new(AtomicBool::new(true));
    let counter = Arc::new(AtomicU64::new(0));
    let attach_delay_ms = Arc::new(AtomicU64::new(0));
    let (flag_in, counter_in, delay_in) = (Arc::clone(&flag), Arc::clone(&counter), Arc::clone(&attach_delay_ms));
    let rig = start_with_backend(
        RenderLoopConfig::default(),
        |_| {},
        move |inner| LeakyDetachBackend {
            inner,
            detach_failures: counter_in,
            count_failure_on_detach: flag_in,
            attach_delay_ms: delay_in,
        },
    );
    rig.show_surface(1);
    assert!(wait_until(DEADLINE, || rig.handle.status().frames_presented >= 3));

    // Поток стоит внутри кадра: всё, что положим ниже, он заберёт одним пакетом.
    rig.probe.hold_frames.store(true, Ordering::SeqCst);
    assert!(wait_until(DEADLINE, || rig
        .probe
        .frame_in_progress
        .load(Ordering::SeqCst)));
    let (request, waiter) = surface_release_handoff(SurfaceGeneration::new(1));
    rig.handle.inbox().surface_destroyed(request);
    attach_delay_ms.store(300, Ordering::SeqCst);
    rig.show_surface(2);
    rig.probe.hold_frames.store(false, Ordering::SeqCst);

    assert_eq!(waiter.wait(DEADLINE), SurfaceReleaseOutcome::Confirmed);
    // Читаем сразу после квитанции, пока поток ещё спит в attach второй surface.
    let status_at_receipt = rig.handle.status();
    assert_eq!(
        status_at_receipt.backend.detach_failures, 1,
        "итог detach не виден по квитанции"
    );
    assert_eq!(
        status_at_receipt.releases_confirmed, 1,
        "подтверждение не видно по квитанции"
    );
}

#[test]
fn deferred_recreate_is_not_retried_on_every_tick_but_a_size_change_applies_at_once() {
    // Интервал заведомо больше длительности теста: допустима ровно одна самостоятельная попытка.
    let config = RenderLoopConfig {
        recreate_retry_interval_ms: 600_000,
        ..RenderLoopConfig::default()
    };
    let rig = start(config, |probe| {
        // Бэкенд 40 кадров подряд просит пересоздание (как при нулевом размере окна).
        let mut scripted = probe.scripted_outcomes.lock().expect("исходы");
        for _ in 0..40 {
            scripted.push_back(PresentOutcome::NeedsRecreate);
        }
    });
    rig.show_surface(1);
    assert!(
        wait_until(DEADLINE, || rig.probe.rendered_frames().len() >= 60),
        "кадры не дошли до конца сценария"
    );

    let resizes_after_deferral = rig.probe.resizes.lock().expect("размеры").len();
    assert_eq!(
        resizes_after_deferral, 1,
        "40 запросов пересоздания подряд должны дать одну попытку, а не по одной на тик"
    );

    // Смена размера от оболочки не подпадает под паузу.
    let new_size = SurfaceSize {
        width_px: 1080,
        height_px: 2400,
    };
    rig.handle.inbox().surface_changed(SurfaceGeneration::new(1), new_size);
    assert!(wait_until(DEADLINE, || rig
        .probe
        .resizes
        .lock()
        .expect("размеры")
        .len()
        == 2));
    assert_eq!(
        rig.probe.resizes.lock().expect("размеры").last().copied(),
        Some(new_size)
    );
}

//! Путь «событие оболочки → маршрут → настоящий поток рендера → квитанция» на fake-бэкенде и fake-часах.

use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use render_loop::backend::{
    AttachError, BackendDiagnostics, BackendError, DisplayBackend, FrameTiming, PastPresentation, PresentOutcome,
};
use render_loop::config::RenderLoopConfig;
use render_loop::thread::{FrameClock, FrameTick, FrameWait, RenderLoopParts, RenderState};
use shell_contract::clock::MonotonicNanos;
use shell_contract::commands::{CommandOutcome, FrameRate, FrameRateOutcome, ScreenWake, ShellCommands};
use shell_contract::surface::{
    NativeSurface, PlatformWindow, SurfaceGeneration, SurfaceReleaseOutcome, SurfaceSize, surface_release_handoff,
};
use shell_contract::vsync::{FrameTimelines, VsyncTimeline};

use super::*;

/// Окно, которое сообщает, что его отпустили.
struct TrackedWindow(Arc<AtomicBool>);

impl PlatformWindow for TrackedWindow {
    fn raw_window(&self) -> NonNull<c_void> {
        NonNull::dangling()
    }
}

impl Drop for TrackedWindow {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

/// Бэкенд без GPU: держит surface между attach и detach, каждый кадр «показывает».
struct HoldingBackend {
    surface: Option<NativeSurface>,
    fail_attach: bool,
    frames: Arc<Mutex<u64>>,
}

impl DisplayBackend for HoldingBackend {
    fn attach(&mut self, surface: NativeSurface, _size: SurfaceSize) -> Result<(), AttachError> {
        if self.fail_attach {
            return Err(AttachError::SurfaceRejected(BackendError {
                operation: "attach".to_owned(),
                reason: "окно мертво".to_owned(),
            }));
        }
        self.surface = Some(surface);
        Ok(())
    }

    fn resize(&mut self, _size: SurfaceSize) -> Result<(), BackendError> {
        Ok(())
    }

    fn render_and_present(&mut self, _timing: FrameTiming) -> PresentOutcome {
        *self.frames.lock().expect("замок") += 1;
        PresentOutcome::Presented
    }

    fn detach(&mut self) -> Option<NativeSurface> {
        self.surface.take()
    }

    fn diagnostics(&mut self) -> BackendDiagnostics {
        BackendDiagnostics::default()
    }

    fn drain_past_presentations(&mut self) -> Vec<PastPresentation> {
        Vec::new()
    }
}

/// Часы, тикающие каждые пару миллисекунд.
struct SteadyClock {
    sequence: u64,
}

impl FrameClock for SteadyClock {
    fn wait_next_frame(&mut self, timeout: Duration) -> FrameWait {
        thread::sleep(Duration::from_millis(2).min(timeout));
        self.sequence += 1;
        FrameWait::Tick(FrameTick {
            sequence: self.sequence,
            expected_present: MonotonicNanos::from_nanos(0),
            deadline: MonotonicNanos::from_nanos(0),
            skipped_ticks: 0,
            timelines: FrameTimelines::only(VsyncTimeline {
                vsync_id: 0,
                expected_present: MonotonicNanos::from_nanos(0),
                deadline: MonotonicNanos::from_nanos(0),
            }),
        })
    }
}

struct SilentCommands;

impl ShellCommands for SilentCommands {
    fn request_camera_permission(&self) -> CommandOutcome {
        CommandOutcome::Queued
    }

    fn set_preferred_frame_rate(&self, _rate: FrameRate) -> FrameRateOutcome {
        FrameRateOutcome::Applied
    }

    fn open_system_gallery(&self) -> CommandOutcome {
        CommandOutcome::Queued
    }

    fn keep_screen_on(&self, _wake: ScreenWake) -> CommandOutcome {
        CommandOutcome::Queued
    }
}

fn spawn_runtime(fail_attach: bool) -> (RenderRuntime, Arc<Mutex<u64>>) {
    let frames = Arc::new(Mutex::new(0));
    let parts = RenderLoopParts {
        config: RenderLoopConfig::default(),
        backend: HoldingBackend {
            surface: None,
            fail_attach,
            frames: Arc::clone(&frames),
        },
        clock: SteadyClock { sequence: 0 },
        commands: Arc::new(SilentCommands),
        performance_hints: std::sync::Arc::new(shell_contract::performance_hint::NoPerformanceHints::new(
            "тест без ADPF",
        )),
    };
    (
        RenderRuntime::spawn(parts, ValidationState::Enabled).expect("поток рендера"),
        frames,
    )
}

fn wait_until(what: &str, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !condition() {
        assert!(Instant::now() < deadline, "не дождались: {what}");
        thread::sleep(Duration::from_millis(2));
    }
}

fn live_status(runtime: &RenderRuntime) -> render_loop::thread::RenderStatus {
    match runtime.status.observe() {
        RenderObservation::Live { status, .. } => *status,
        RenderObservation::Unavailable { reason } => panic!("рендер недоступен: {reason}"),
    }
}

const SIZE: SurfaceSize = SurfaceSize {
    width_px: 1440,
    height_px: 3216,
};

#[test]
fn route_feeds_real_render_thread_until_frames_are_presented_and_receipt_is_confirmed_after_release() {
    let (mut runtime, frames) = spawn_runtime(false);
    let window_released = Arc::new(AtomicBool::new(false));
    let surface = NativeSurface::new(
        SurfaceGeneration::new(1),
        Box::new(TrackedWindow(Arc::clone(&window_released))),
    );
    runtime.route.surface_created(surface);
    runtime.route.surface_changed(SurfaceGeneration::new(1), SIZE);
    runtime.route.resumed();

    wait_until("первые кадры", || {
        let status = live_status(&runtime);
        status.state == RenderState::Presenting && status.frames_presented > 0
    });
    assert!(*frames.lock().expect("замок") > 0, "бэкенд действительно получил кадры");
    assert_eq!(live_status(&runtime).generation, Some(SurfaceGeneration::new(1)));
    assert!(
        !window_released.load(Ordering::SeqCst),
        "пока идёт показ, окно держит поток рендера"
    );

    let (request, waiter) = surface_release_handoff(SurfaceGeneration::new(1));
    runtime.route.surface_destroyed(request);
    assert_eq!(waiter.wait(Duration::from_secs(2)), SurfaceReleaseOutcome::Confirmed);
    assert!(
        window_released.load(Ordering::SeqCst),
        "окно отпущено до подтверждения квитанции"
    );
    wait_until("статус после освобождения", || {
        live_status(&runtime).releases_confirmed == 1
    });
    assert_ne!(live_status(&runtime).state, RenderState::Presenting);
}

#[test]
fn surface_that_backend_rejects_is_still_confirmed_on_destroy() {
    let (mut runtime, _frames) = spawn_runtime(true);
    let surface = NativeSurface::new(
        SurfaceGeneration::new(4),
        Box::new(TrackedWindow(Arc::new(AtomicBool::new(false)))),
    );
    runtime.route.surface_created(surface);
    runtime.route.surface_changed(SurfaceGeneration::new(4), SIZE);
    runtime.route.resumed();
    wait_until("отказ бэкенда посчитан", || {
        live_status(&runtime).backend_errors > 0
    });

    let (request, waiter) = surface_release_handoff(SurfaceGeneration::new(4));
    runtime.route.surface_destroyed(request);
    assert_eq!(
        waiter.wait(Duration::from_secs(2)),
        SurfaceReleaseOutcome::Confirmed,
        "даже при отказе бэкенда surfaceDestroyed не зависает"
    );
    assert_eq!(live_status(&runtime).frames_presented, 0);
}

#[test]
fn unavailable_runtime_confirms_immediately_and_reports_reason() {
    let mut runtime = RenderRuntime::unavailable("vsync не запущен: у потока нет looper");
    let surface = NativeSurface::new(
        SurfaceGeneration::new(2),
        Box::new(TrackedWindow(Arc::new(AtomicBool::new(false)))),
    );
    runtime.route.surface_created(surface);
    let (request, waiter) = surface_release_handoff(SurfaceGeneration::new(2));
    runtime.route.surface_destroyed(request);
    assert_eq!(
        waiter.wait(Duration::from_millis(100)),
        SurfaceReleaseOutcome::Confirmed
    );
    match runtime.status.observe() {
        RenderObservation::Unavailable { reason } => assert!(reason.contains("looper"), "{reason}"),
        other => panic!("ожидалось Unavailable, получено {other:?}"),
    }
}

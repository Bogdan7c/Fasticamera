//! Паника в цикле потока вне вызова бэкенда (часы кадров): поток не умирает молча, а честно
//! переходит в `Faulted`, отпускает экран и продолжает подтверждать квитанции.

#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть сигнал о провале"
)]

mod support;

use std::sync::Arc;
use std::time::Duration;

use render_loop::config::RenderLoopConfig;
use render_loop::thread::{FaultKind, RenderLoop, RenderLoopParts, RenderState};
use shell_contract::commands::ScreenWake;
use shell_contract::surface::{SurfaceGeneration, SurfaceReleaseOutcome, surface_release_handoff};
use support::{FakeBackend, FakeCommands, Journal, PanickingClock, Probe, SIZE, fake_surface, wait_until};

const DEADLINE: Duration = Duration::from_secs(5);

#[test]
fn clock_panic_faults_the_thread_promptly_releases_the_screen_and_keeps_confirming_receipts() {
    let journal = Arc::new(Journal::default());
    let probe = Probe::new(Arc::clone(&journal));
    let commands = Arc::new(FakeCommands::with_journal(Arc::clone(&journal)));
    let handle = RenderLoop::spawn(RenderLoopParts {
        config: RenderLoopConfig::default(),
        backend: FakeBackend::new(Arc::clone(&probe)),
        clock: PanickingClock::new(5),
        commands: commands.clone(),
        performance_hints: std::sync::Arc::new(shell_contract::performance_hint::NoPerformanceHints::new(
            "тест без ADPF",
        )),
    })
    .expect("поток запускается");

    let inbox = handle.inbox();
    inbox.surface_created(fake_surface(&journal, 1));
    inbox.surface_changed(SurfaceGeneration::new(1), SIZE);

    // Статус обязан стать Faulted сам, без stop_and_join.
    assert!(
        wait_until(DEADLINE, || handle.status().state == RenderState::Faulted),
        "поток умер молча: {:?}",
        handle.status()
    );
    let status = handle.status();
    let fault = status.fault.expect("причина неисправности записана");
    assert_eq!(fault.kind, FaultKind::WorkerPanic);
    assert!(fault.message.contains("fake clock: boom"), "{}", fault.message);

    // Экран отпущен: после KeepOn показа ушёл AllowSleep.
    assert!(
        wait_until(DEADLINE, || commands.screen_wake_calls().last()
            == Some(&ScreenWake::AllowSleep)),
        "блокировка гашения экрана не снята: {:?}",
        commands.screen_wake_calls()
    );
    assert_eq!(
        commands.screen_wake_calls(),
        [ScreenWake::KeepOn, ScreenWake::AllowSleep]
    );

    // GPU-объекты отпущены (detach на пути неисправности), окно возвращено.
    assert_eq!(probe.live_objects(), 0);
    assert_eq!(journal.count("window_released(1)"), 1);

    // Оболочка не зависает: квитанция подтверждается живым потоком (не таймаут и не Dropped).
    let (request, waiter) = surface_release_handoff(SurfaceGeneration::new(1));
    inbox.surface_destroyed(request);
    assert_eq!(waiter.wait(DEADLINE), SurfaceReleaseOutcome::Confirmed);
    assert_eq!(handle.stop().state, RenderState::Stopped);
}

//! Тесты блокировки гашения экрана: поток рендера просит платформу «не гасить экран»
//! при входе в показ и «можно гасить» при выходе, и только на переходах, а не на кадрах.

#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть сигнал о провале"
)]

mod support;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use render_loop::config::RenderLoopConfig;
use render_loop::thread::{RenderLoop, RenderLoopHandle, RenderLoopParts, RenderState};
use shell_contract::commands::{CommandOutcome, ScreenWake};
use shell_contract::surface::{SurfaceGeneration, SurfaceReleaseOutcome, surface_release_handoff};
use support::{FakeBackend, FakeClock, FakeCommands, Journal, Probe, SIZE, fake_surface, wait_until};

const DEADLINE: Duration = Duration::from_secs(5);

struct Rig {
    handle: RenderLoopHandle,
    journal: Arc<Journal>,
    commands: Arc<FakeCommands>,
    clock_stop: Arc<AtomicBool>,
}

fn start_rig(config: RenderLoopConfig) -> Rig {
    let journal = Arc::new(Journal::default());
    let probe = Probe::new(Arc::clone(&journal));
    let (clock, clock_stop) = FakeClock::new();
    let commands = Arc::new(FakeCommands::with_journal(Arc::clone(&journal)));
    let handle = RenderLoop::spawn(RenderLoopParts {
        config,
        backend: FakeBackend::new(probe),
        clock,
        commands: commands.clone(),
    })
    .expect("поток запускается");
    Rig {
        handle,
        journal,
        commands,
        clock_stop,
    }
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
}

#[test]
fn keep_on_is_sent_once_after_attach_and_not_per_frame() {
    let rig = start_rig(RenderLoopConfig::default());
    assert!(rig.commands.screen_wake_calls().is_empty(), "до показа команд нет");

    rig.show_surface(1);
    rig.wait_for_frames(60);

    assert_eq!(rig.commands.screen_wake_calls(), [ScreenWake::KeepOn]);
    let attach = rig.journal.position("attach(1,1440x3216)").expect("attach");
    let keep_on = rig.journal.position("keep_screen_on(KeepOn)").expect("KeepOn");
    assert!(attach < keep_on, "KeepOn после attach: {:?}", rig.journal.entries());
    let status = rig.handle.status().screen_wake;
    assert_eq!(status.requested, ScreenWake::KeepOn);
    assert_eq!(status.last_outcome, Some(CommandOutcome::Queued));
    assert_eq!(status.commands_sent, 1);
}

#[test]
fn allow_sleep_is_sent_after_destroy_and_keep_on_again_for_next_surface() {
    let rig = start_rig(RenderLoopConfig::default());
    rig.show_surface(1);
    rig.wait_for_frames(3);

    let (request, waiter) = surface_release_handoff(SurfaceGeneration::new(1));
    rig.handle.inbox().surface_destroyed(request);
    assert_eq!(waiter.wait(DEADLINE), SurfaceReleaseOutcome::Confirmed);
    assert!(wait_until(DEADLINE, || rig.commands.screen_wake_calls().len() == 2));
    assert_eq!(
        rig.commands.screen_wake_calls(),
        [ScreenWake::KeepOn, ScreenWake::AllowSleep]
    );
    let detach = rig.journal.position("detach_done(1)").expect("detach");
    let allow = rig.journal.position("keep_screen_on(AllowSleep)").expect("AllowSleep");
    assert!(detach < allow, "AllowSleep после detach: {:?}", rig.journal.entries());

    rig.show_surface(2);
    assert!(wait_until(DEADLINE, || rig.commands.screen_wake_calls().len() == 3));
    assert_eq!(rig.commands.screen_wake_calls()[2], ScreenWake::KeepOn);
}

#[test]
fn fault_releases_the_screen_wake() {
    let rig = start_rig(RenderLoopConfig::default());
    rig.show_surface(1);
    rig.wait_for_frames(2);
    rig.clock_stop.store(true, Ordering::SeqCst);

    assert!(wait_until(DEADLINE, || rig.handle.status().state == RenderState::Faulted));
    assert!(wait_until(DEADLINE, || {
        rig.commands.screen_wake_calls() == [ScreenWake::KeepOn, ScreenWake::AllowSleep]
    }));
}

#[test]
fn stop_releases_the_screen_wake() {
    let rig = start_rig(RenderLoopConfig::default());
    rig.show_surface(1);
    rig.wait_for_frames(2);
    let commands = Arc::clone(&rig.commands);

    let final_status = rig.handle.stop();

    assert_eq!(final_status.state, RenderState::Stopped);
    assert_eq!(
        commands.screen_wake_calls(),
        [ScreenWake::KeepOn, ScreenWake::AllowSleep]
    );
    assert_eq!(final_status.screen_wake.requested, ScreenWake::AllowSleep);
    assert_eq!(final_status.screen_wake.commands_sent, 2);
}

#[test]
fn disabled_flag_sends_no_commands_at_all() {
    let rig = start_rig(RenderLoopConfig {
        keep_screen_on_while_presenting: false,
        ..RenderLoopConfig::default()
    });
    rig.show_surface(1);
    rig.wait_for_frames(10);
    let commands = Arc::clone(&rig.commands);

    let final_status = rig.handle.stop();

    assert!(commands.screen_wake_calls().is_empty());
    assert_eq!(final_status.screen_wake.commands_sent, 0);
    assert_eq!(final_status.screen_wake.last_outcome, None);
}

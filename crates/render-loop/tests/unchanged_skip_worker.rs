//! Как поток рендера учитывает тики, пропущенные как «ничего не изменилось» (решение 214, сессия 16a), через
//! НАСТОЯЩИЙ поток на fake-бэкенде, который сам решает, какие тики пропустить, и честно сообщает времена показов:
//! * пропуск — не дефект: пейсинг идеально ровного показа через тик (120 Гц, картинка стоит) остаётся `Passed` без
//!   единого «пропуска кадра», в том числе после серий из нескольких пропусков и при возврате к показу каждого тика;
//! * учёт отдельный: `ticks_skipped_unchanged` считает пропуски, `frames_presented` — только показы, ни ошибок
//!   бэкенда, ни таймаутов acquire, ни схода на ближнюю линию;
//! * ADPF получает отчёт только о показанных кадрах: пропущенный тик не работа и не повод ужиматься;
//! * проявление снимков не встаёт: бэкенд получает `advance_stills` на КАЖДОМ тике, пропущенные включены.

#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть сигнал о провале"
)]

mod support;

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use render_loop::backend::{
    AttachError, BackendDiagnostics, BackendError, DisplayBackend, DisplayInfo, FrameTiming, PastPresentation,
    PresentOutcome,
};
use render_loop::config::{AdpfConfig, AdpfMode, RenderLoopConfig};
use render_loop::pacing::PacingVerdict;
use render_loop::thread::{RenderLoop, RenderLoopParts, RenderStatus};
use shell_contract::clock::MonotonicNanos;
use shell_contract::performance_hint::fake::FakePerformanceHints;
use shell_contract::surface::{NativeSurface, SurfaceGeneration, SurfaceSize};
use support::{FakeClock, FakeCommands, Journal, SIZE, fake_surface, wait_until};

/// Идеальные 120 Гц: экран показывает ровно по времени кадра.
const PERIOD_NANOS: i64 = 8_333_333;
/// Сколько записей помнит «драйвер»: больше, чем длинная серия пропусков, чтобы не терять записи.
const DRIVER_RING_CAPACITY: usize = 64;

/// Сценарий пропусков: какие тики бэкенд пропускает как «ничего не изменилось».
#[derive(Debug, Clone, Copy)]
enum Scenario {
    /// Покой при камере 60 к/с на протяжении всего прогона: пропуск через тик.
    IdleEveryOtherTick,
    /// Покой, затем длинные серии (четыре пропуска подряд), затем движение (каждый тик показывается).
    IdleThenLongRunsThenMotion,
}

impl Scenario {
    fn skips_tick(self, tick: u64) -> bool {
        match self {
            Self::IdleEveryOtherTick => tick % 2 == 1,
            Self::IdleThenLongRunsThenMotion => match tick {
                0..120 => tick % 2 == 1,
                120..220 => !tick.is_multiple_of(5),
                _ => false,
            },
        }
    }
}

struct SkippingBackend {
    scenario: Scenario,
    held_surface: Option<NativeSurface>,
    tick: u64,
    next_present_id: u32,
    ring: VecDeque<PastPresentation>,
    /// Сколько раз бэкенд получил `advance_stills` (должно идти на каждом тике).
    advance_stills_calls: Arc<AtomicU64>,
    /// Сколько тиков бэкенд получил в `render_and_present`.
    ticks_seen: Arc<AtomicU64>,
    /// Сколько раз бэкенд узнал, что показ возобновлён после паузы.
    resumed_calls: Arc<AtomicU64>,
}

impl DisplayBackend for SkippingBackend {
    fn attach(&mut self, surface: NativeSurface, _size: SurfaceSize) -> Result<(), AttachError> {
        self.held_surface = Some(surface);
        Ok(())
    }

    fn resize(&mut self, _size: SurfaceSize) -> Result<(), BackendError> {
        Ok(())
    }

    fn render_and_present(&mut self, timing: FrameTiming) -> PresentOutcome {
        let tick = self.tick;
        self.tick += 1;
        self.ticks_seen.fetch_add(1, Ordering::SeqCst);
        if self.scenario.skips_tick(tick) {
            return PresentOutcome::SkippedUnchanged;
        }
        if self.ring.len() == DRIVER_RING_CAPACITY {
            self.ring.pop_front();
        }
        let present_id = self.next_present_id;
        self.next_present_id += 1;
        self.ring.push_back(PastPresentation {
            present_id,
            frame_index: timing.frame_index,
            // Экран показывает кадр ровно на vsync своего тика: пропущенные тики остаются дырами во времени.
            actual_present: MonotonicNanos::from_nanos(
                1_000_000_000 + i64::try_from(timing.frame_index).expect("кадр") * PERIOD_NANOS,
            ),
        });
        PresentOutcome::Presented
    }

    fn detach(&mut self) -> Option<NativeSurface> {
        self.held_surface.take()
    }

    fn diagnostics(&mut self) -> BackendDiagnostics {
        BackendDiagnostics {
            display: Some(DisplayInfo {
                format: "B8G8R8A8_UNORM".to_owned(),
                color_space: "SRGB_NONLINEAR_KHR".to_owned(),
                extent: SIZE,
                transform: "Identity".to_owned(),
                image_count: 3,
                recreation_count: 0,
                refresh_cycle: Some(Duration::from_nanos(PERIOD_NANOS as u64)),
            }),
            ..BackendDiagnostics::default()
        }
    }

    fn drain_past_presentations(&mut self) -> Vec<PastPresentation> {
        self.ring.drain(..).collect()
    }

    fn advance_stills(&mut self) {
        self.advance_stills_calls.fetch_add(1, Ordering::SeqCst);
    }

    fn presentation_resumed(&mut self) {
        self.resumed_calls.fetch_add(1, Ordering::SeqCst);
    }
}

struct Run {
    status: RenderStatus,
    ticks_seen: u64,
    advance_stills_calls: u64,
    adpf_reports: usize,
}

/// Гоняет поток, пока не пройдут все три фазы сценария (покой, серии, движение) и не наберётся окно пейсинга.
fn run_scenario(scenario: Scenario, pacing_window_s: f64) -> Run {
    let journal = Arc::new(Journal::default());
    let advance_stills_calls = Arc::new(AtomicU64::new(0));
    let ticks_seen = Arc::new(AtomicU64::new(0));
    let hints = FakePerformanceHints::new(777);
    let (clock, _clock_stop) = FakeClock::new();
    let handle = RenderLoop::spawn(RenderLoopParts {
        config: RenderLoopConfig {
            preferred_frame_rate_hz: 120.0,
            pacing_window_s,
            diagnostics_poll_interval_ms: 20,
            adpf: AdpfConfig {
                mode: AdpfMode::Enabled,
                ..AdpfConfig::default()
            },
            ..RenderLoopConfig::default()
        },
        backend: SkippingBackend {
            scenario,
            held_surface: None,
            tick: 0,
            next_present_id: 0,
            ring: VecDeque::new(),
            advance_stills_calls: Arc::clone(&advance_stills_calls),
            ticks_seen: Arc::clone(&ticks_seen),
            resumed_calls: Arc::new(AtomicU64::new(0)),
        },
        clock,
        commands: Arc::new(FakeCommands::default()),
        performance_hints: Arc::new(hints.clone()),
    })
    .expect("поток запускается");
    let inbox = handle.inbox();
    inbox.surface_created(fake_surface(&journal, 1));
    inbox.surface_changed(SurfaceGeneration::new(1), SIZE);
    assert!(
        wait_until(Duration::from_secs(20), || {
            // Три фазы пройдены (>= 220 тиков) и ещё запас показа каждого тика, чтобы окно пейсинга закрыло серии.
            ticks_seen.load(Ordering::SeqCst) >= 420
        }),
        "сценарий не дошёл до конца: {:?}",
        handle.status()
    );
    let status = handle.stop();
    Run {
        status,
        ticks_seen: ticks_seen.load(Ordering::SeqCst),
        advance_stills_calls: advance_stills_calls.load(Ordering::SeqCst),
        adpf_reports: hints.journal().reports.len(),
    }
}

/// Окно пейсинга шире всего прогона: в него попадают все фазы сценария, а не только последняя.
const WIDE_WINDOW_S: f64 = 30.0;

#[test]
fn deliberately_skipped_ticks_are_counted_apart_and_are_neither_errors_nor_presents() {
    let scenario = Scenario::IdleThenLongRunsThenMotion;
    let run = run_scenario(scenario, WIDE_WINDOW_S);
    let skipped_in_script: u64 = (0..run.ticks_seen).filter(|tick| scenario.skips_tick(*tick)).count() as u64;
    let status = &run.status;
    assert_eq!(status.ticks_skipped_unchanged, skipped_in_script, "{status:?}");
    assert_eq!(
        status.frames_presented,
        run.ticks_seen - skipped_in_script,
        "показом считается только настоящий показ"
    );
    assert_eq!(status.backend_errors, 0);
    assert_eq!(status.acquire_timeouts, 0);
    assert_eq!(
        status.present_target.skipped_ticks_for_depth, 0,
        "сход на ближнюю линию — другая причина пропуска"
    );
}

#[test]
fn pacing_stays_clean_through_idle_skips_long_skip_runs_and_the_return_to_every_tick() {
    let run = run_scenario(Scenario::IdleThenLongRunsThenMotion, WIDE_WINDOW_S);
    let pacing = run.status.pacing;
    // Окно держит все три фазы: показы через тик, через пять тиков и каждый тик.
    assert!(pacing.intervals > 200, "в окне весь прогон: {pacing:?}");
    assert_eq!(pacing.misses, 0, "пауза задумана, потерь нет: {pacing:?}");
    assert_eq!(pacing.timing_gaps, 0, "{pacing:?}");
    assert_eq!(
        pacing.histogram[1], pacing.intervals,
        "по остатку после вычета нарочных пропусков каждый интервал — один период: {pacing:?}"
    );
}

#[test]
fn a_full_pacing_window_of_idle_skips_passes_the_verdict() {
    let run = run_scenario(Scenario::IdleEveryOtherTick, 0.5);
    let pacing = run.status.pacing;
    assert_eq!(pacing.misses, 0, "{pacing:?}");
    assert_eq!(
        pacing.verdict,
        PacingVerdict::Passed,
        "показ через тик при неизменной картинке — не дефект: {pacing:?}"
    );
}

#[test]
fn adpf_reports_only_presented_frames() {
    let run = run_scenario(Scenario::IdleThenLongRunsThenMotion, WIDE_WINDOW_S);
    let presented = usize::try_from(run.status.frames_presented).expect("число");
    assert!(run.adpf_reports > 20, "отчёты идут: {}", run.adpf_reports);
    assert!(
        run.adpf_reports <= presented,
        "пропущенный тик не работа кадра: отчётов {} при показах {presented}",
        run.adpf_reports
    );
    assert_eq!(run.status.adpf.report_failures, 0);
}

#[test]
fn stills_are_advanced_on_every_tick_including_skipped_ones() {
    let run = run_scenario(Scenario::IdleThenLongRunsThenMotion, WIDE_WINDOW_S);
    assert_eq!(
        run.advance_stills_calls, run.ticks_seen,
        "проявление снимков идёт на каждом тике: пропуски его не тормозят"
    );
}

#[test]
fn resuming_after_a_pause_tells_the_backend_exactly_once_and_a_pause_alone_does_not() {
    let journal = Arc::new(Journal::default());
    let resumed_calls = Arc::new(AtomicU64::new(0));
    let (clock, _clock_stop) = FakeClock::new();
    let handle = RenderLoop::spawn(RenderLoopParts {
        config: RenderLoopConfig::default(),
        backend: SkippingBackend {
            scenario: Scenario::IdleEveryOtherTick,
            held_surface: None,
            tick: 0,
            next_present_id: 0,
            ring: VecDeque::new(),
            advance_stills_calls: Arc::new(AtomicU64::new(0)),
            ticks_seen: Arc::new(AtomicU64::new(0)),
            resumed_calls: Arc::clone(&resumed_calls),
        },
        clock,
        commands: Arc::new(FakeCommands::default()),
        performance_hints: Arc::new(FakePerformanceHints::new(777)),
    })
    .expect("поток запускается");
    let inbox = handle.inbox();
    inbox.surface_created(fake_surface(&journal, 1));
    inbox.surface_changed(SurfaceGeneration::new(1), SIZE);
    assert!(wait_until(Duration::from_secs(10), || handle
        .status()
        .ticks_skipped_unchanged
        > 3));
    inbox.paused();
    assert!(wait_until(Duration::from_secs(10), || handle.status().state
        == render_loop::thread::RenderState::Paused));
    assert_eq!(resumed_calls.load(Ordering::SeqCst), 0, "пауза сама эталон не трогает");
    inbox.resumed();
    assert!(wait_until(Duration::from_secs(10), || resumed_calls
        .load(Ordering::SeqCst)
        == 1));
    drop(handle.stop());
    assert_eq!(resumed_calls.load(Ordering::SeqCst), 1);
}

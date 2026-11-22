//! Поток рендера двигает снимки без показа (сессия 14e, решение 150).
//!
//! Проверяется настоящий поток `fasticamera-render` целиком: после уничтожения surface (или паузы) в ящик больше
//! НИЧЕГО не приходит, а захваченный снимок всё равно получает итог — шаги ему даёт таймер потока. Без снимков
//! поток спит без пробуждений, неисправность отдаёт снимкам итог сразу, квитанция освобождения не ждёт снимков.
//!
//! Бэкенд — обёртка над общим fake-показом (`support::FakeBackend`) с поддельными снимками: снимку нужно
//! `STEPS_TO_FINISH` шагов `advance_stills`, и шаги засчитываются только после разрешения теста (`may_progress`),
//! чтобы итог наверняка случился ПОСЛЕ отсоединения, а не на кадрах с показом.

#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть сигнал о провале"
)]

mod support;

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use camera_contract::ids::LensFacing;
use camera_contract::viewfinder_crop::CropAspect;
use camera_contract::zoom::{ZoomCatalog, ZoomFraming, ZoomRatio};
use render_loop::backend::{
    AttachError, BackendDiagnostics, BackendError, DisplayBackend, FrameTiming, PastPresentation, PresentOutcome,
};
use render_loop::config::RenderLoopConfig;
use render_loop::mailbox::InboxDelivery;
use render_loop::still::{StillBegin, StillCommand, StillCommandOutcome, StillEventSink, StillWorkload};
use render_loop::thread::{RenderLoop, RenderLoopHandle, RenderLoopParts, RenderState};
use shell_contract::clock::MonotonicNanos;
use shell_contract::orientation::DeviceOrientation;
use shell_contract::surface::{
    NativeSurface, SurfaceGeneration, SurfaceReleaseOutcome, SurfaceSize, surface_release_handoff,
};
use still_contract::develop::{DevelopEvent, DevelopFailure};
use still_contract::plan::{FrameSelection, PrimaryFormat, RawOutput, ShutterPress, StillId, StillOutputs, StillPlan};
use support::{FakeBackend, FakeClock, FakeCommands, Journal, Probe, fake_surface, wait_until};

const DEADLINE: Duration = Duration::from_secs(5);
const SIZE: SurfaceSize = SurfaceSize {
    width_px: 1080,
    height_px: 2412,
};
/// Сколько шагов `advance_stills` нужно поддельному снимку до итога.
const STEPS_TO_FINISH: u32 = 5;

// ---- поддельные снимки ----

struct PendingStill {
    id: StillId,
    events: Arc<dyn StillEventSink>,
    steps_left: u32,
}

/// Общее состояние поддельных снимков: тест управляет им и читает учёт.
#[derive(Default)]
struct StillProbe {
    pending: Mutex<Vec<PendingStill>>,
    /// Пока `false`, шаги не продвигают снимки (итог не может случиться раньше, чем тест разрешит).
    may_progress: AtomicBool,
    attached: AtomicBool,
    advance_calls: AtomicU64,
    advance_calls_without_surface: AtomicU64,
    abandon_calls: AtomicU64,
    /// Следующий показ кадра паникует (неисправность бэкенда).
    panic_on_next_frame: AtomicBool,
}

impl StillProbe {
    fn pending_len(&self) -> usize {
        self.pending.lock().unwrap_or_else(PoisonError::into_inner).len()
    }
}

/// Показ — общий fake, снимки — поддельные с учётом шагов.
struct StillCapableBackend {
    display: FakeBackend,
    stills: Arc<StillProbe>,
}

impl DisplayBackend for StillCapableBackend {
    fn attach(&mut self, surface: NativeSurface, size: SurfaceSize) -> Result<(), AttachError> {
        let attached = self.display.attach(surface, size);
        self.stills.attached.store(attached.is_ok(), Ordering::SeqCst);
        attached
    }

    fn resize(&mut self, size: SurfaceSize) -> Result<(), BackendError> {
        self.display.resize(size)
    }

    fn render_and_present(&mut self, timing: FrameTiming) -> PresentOutcome {
        if self.stills.panic_on_next_frame.swap(false, Ordering::SeqCst) {
            panic!("fake backend: показ упал при недоделанном снимке");
        }
        self.display.render_and_present(timing)
    }

    fn detach(&mut self) -> Option<NativeSurface> {
        self.stills.attached.store(false, Ordering::SeqCst);
        self.display.detach()
    }

    fn diagnostics(&mut self) -> BackendDiagnostics {
        self.display.diagnostics()
    }

    fn drain_past_presentations(&mut self) -> Vec<PastPresentation> {
        self.display.drain_past_presentations()
    }

    fn apply_still_command(&mut self, command: StillCommand) -> StillCommandOutcome {
        match command {
            StillCommand::Begin(StillBegin { plan, events }) => {
                self.stills
                    .pending
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .push(PendingStill {
                        id: plan.id,
                        events,
                        steps_left: STEPS_TO_FINISH,
                    });
                StillCommandOutcome::Accepted
            }
            other @ (StillCommand::Target(_) | StillCommand::FlashFailed { .. }) => {
                StillCommandOutcome::Unsupported(other)
            }
        }
    }

    fn advance_stills(&mut self) {
        self.stills.advance_calls.fetch_add(1, Ordering::SeqCst);
        if !self.stills.attached.load(Ordering::SeqCst) {
            self.stills.advance_calls_without_surface.fetch_add(1, Ordering::SeqCst);
        }
        if !self.stills.may_progress.load(Ordering::SeqCst) {
            return;
        }
        let mut pending = self.stills.pending.lock().unwrap_or_else(PoisonError::into_inner);
        for still in pending.iter_mut() {
            still.steps_left = still.steps_left.saturating_sub(1);
        }
        pending.retain(|still| {
            let finished = still.steps_left == 0;
            if finished {
                still.events.deliver(DevelopEvent::PrimaryRendered { id: still.id });
            }
            !finished
        });
    }

    fn still_workload(&self) -> StillWorkload {
        if self.stills.pending_len() == 0 {
            StillWorkload::Idle
        } else {
            StillWorkload::Unfinished
        }
    }

    fn abandon_stills(&mut self) {
        self.stills.abandon_calls.fetch_add(1, Ordering::SeqCst);
        for still in self
            .stills
            .pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .drain(..)
        {
            still.events.deliver(DevelopEvent::Failed {
                id: still.id,
                failure: DevelopFailure::Abandoned,
            });
        }
    }
}

// ---- стенд ----

#[derive(Default)]
struct Sink(Mutex<Vec<DevelopEvent>>);

impl StillEventSink for Sink {
    fn deliver(&self, event: DevelopEvent) {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).push(event);
    }
}

impl Sink {
    fn events(&self) -> Vec<String> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .map(|event| format!("{event:?}"))
            .collect()
    }
}

struct Rig {
    handle: RenderLoopHandle,
    journal: Arc<Journal>,
    stills: Arc<StillProbe>,
    clock_stop: Arc<AtomicBool>,
}

impl Drop for Rig {
    fn drop(&mut self) {
        self.clock_stop.store(true, Ordering::SeqCst);
    }
}

fn start() -> Rig {
    let journal = Arc::new(Journal::default());
    let probe = Probe::new(Arc::clone(&journal));
    let stills = Arc::new(StillProbe::default());
    let (clock, clock_stop) = FakeClock::new();
    let handle = RenderLoop::spawn(RenderLoopParts {
        config: RenderLoopConfig::default(),
        backend: StillCapableBackend {
            display: FakeBackend::new(probe),
            stills: Arc::clone(&stills),
        },
        clock,
        commands: Arc::new(FakeCommands::default()),
        performance_hints: Arc::new(shell_contract::performance_hint::NoPerformanceHints::new(
            "тест без ADPF",
        )),
    })
    .expect("поток запускается");
    Rig {
        handle,
        journal,
        stills,
        clock_stop,
    }
}

fn plan(id: u64) -> StillPlan {
    StillPlan {
        id: StillId(id),
        press: ShutterPress {
            at: MonotonicNanos::from_nanos(1_000),
        },
        selection: FrameSelection::ShownAtPress,
        framing: ZoomFraming {
            aspect: CropAspect::SensorNative,
            facing: LensFacing::Back,
            ratio: ZoomRatio::new(1.0).expect("коэффициент"),
            catalog: Arc::new(ZoomCatalog::default()),
        },
        orientation: DeviceOrientation::Portrait,
        outputs: StillOutputs {
            primary: PrimaryFormat::Heif,
            raw: RawOutput::Skip,
        },
    }
}

impl Rig {
    fn show_surface_and_wait_for_frames(&self) {
        let inbox = self.handle.inbox();
        assert_eq!(
            inbox.surface_created(fake_surface(&self.journal, 1)),
            InboxDelivery::Delivered
        );
        inbox.surface_changed(SurfaceGeneration::new(1), SIZE);
        assert!(
            wait_until(DEADLINE, || self.handle.status().frames_presented >= 3),
            "кадры не пошли: {:?}",
            self.handle.status()
        );
    }

    /// Нажатие затвора: снимок принят бэкендом и ждёт итога.
    fn press(&self, id: u64) -> Arc<Sink> {
        let sink = Arc::new(Sink::default());
        let events: Arc<dyn StillEventSink> = Arc::clone(&sink) as Arc<dyn StillEventSink>;
        assert_eq!(
            self.handle
                .inbox()
                .still_command(StillCommand::Begin(StillBegin { plan: plan(id), events })),
            InboxDelivery::Delivered
        );
        assert!(
            wait_until(DEADLINE, || self.stills.pending_len() == 1),
            "бэкенд не принял снимок"
        );
        sink
    }

    fn destroy_surface(&self) -> SurfaceReleaseOutcome {
        let (request, waiter) = surface_release_handoff(SurfaceGeneration::new(1));
        self.handle.inbox().surface_destroyed(request);
        waiter.wait(DEADLINE)
    }
}

// ---- тесты ----

#[test]
fn a_captured_still_gets_its_final_after_the_surface_is_gone_with_no_further_events() {
    let rig = start();
    rig.show_surface_and_wait_for_frames();
    let sink = rig.press(1);

    // Квитанция освобождения не ждёт снимков: подтверждена, пока снимок ещё в работе.
    assert_eq!(rig.destroy_surface(), SurfaceReleaseOutcome::Confirmed);
    assert!(
        sink.events().is_empty(),
        "снимок не брошен отсоединением: {:?}",
        sink.events()
    );
    assert_eq!(rig.handle.status().state, RenderState::WaitingForSurface);

    // В ящик больше ничего не идёт. Единственный источник шагов — таймер потока без показа.
    rig.stills.may_progress.store(true, Ordering::SeqCst);
    assert!(
        wait_until(DEADLINE, || !sink.events().is_empty()),
        "без экрана снимок так и не получил итог (шагов без surface: {})",
        rig.stills.advance_calls_without_surface.load(Ordering::SeqCst)
    );
    assert_eq!(sink.events(), vec!["PrimaryRendered { id: StillId(1) }".to_owned()]);
    assert!(
        rig.stills.advance_calls_without_surface.load(Ordering::SeqCst) >= u64::from(STEPS_TO_FINISH),
        "итог дали шаги без surface"
    );
    assert_eq!(
        rig.stills.abandon_calls.load(Ordering::SeqCst),
        0,
        "снимок доведён, а не брошен"
    );

    // Снимков больше нет — поток спит без срока: ни одного лишнего пробуждения.
    let calls_after_final = rig.stills.advance_calls.load(Ordering::SeqCst);
    std::thread::sleep(Duration::from_millis(150));
    assert_eq!(rig.stills.advance_calls.load(Ordering::SeqCst), calls_after_final);
}

#[test]
fn a_paused_activity_with_the_surface_still_attached_keeps_moving_stills() {
    let rig = start();
    rig.show_surface_and_wait_for_frames();
    let sink = rig.press(1);
    assert_eq!(rig.handle.inbox().paused(), InboxDelivery::Delivered);
    assert!(
        wait_until(DEADLINE, || rig.handle.status().state != RenderState::Presenting),
        "пауза не остановила показ"
    );
    let frames_when_paused = rig.handle.status().frames_presented;

    rig.stills.may_progress.store(true, Ordering::SeqCst);
    assert!(
        wait_until(DEADLINE, || !sink.events().is_empty()),
        "на паузе снимок не получил итог"
    );
    assert_eq!(sink.events(), vec!["PrimaryRendered { id: StillId(1) }".to_owned()]);
    assert!(rig.stills.attached.load(Ordering::SeqCst), "surface не отсоединялась");
    assert_eq!(
        rig.handle.status().frames_presented,
        frames_when_paused,
        "итог дали шаги без показа, а не новые кадры"
    );
}

#[test]
fn a_backend_fault_gives_an_unfinished_still_its_final_at_once() {
    let rig = start();
    rig.show_surface_and_wait_for_frames();
    let sink = rig.press(1);
    rig.stills.panic_on_next_frame.store(true, Ordering::SeqCst);
    assert!(
        wait_until(DEADLINE, || rig.handle.status().state == RenderState::Faulted),
        "поток не перешёл в неисправность"
    );
    assert!(
        wait_until(DEADLINE, || !sink.events().is_empty()),
        "снимок неисправного потока остался без итога"
    );
    assert_eq!(rig.stills.abandon_calls.load(Ordering::SeqCst), 1);
    match sink.0.lock().unwrap_or_else(PoisonError::into_inner).as_slice() {
        [
            DevelopEvent::Failed {
                failure: DevelopFailure::Abandoned,
                ..
            },
        ] => {}
        other => panic!("ожидали один Failed(Abandoned), получили {other:?}"),
    }
    // Неисправный поток бэкенд не зовёт и ради снимков себя не будит.
    let calls = rig.stills.advance_calls.load(Ordering::SeqCst);
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(rig.stills.advance_calls.load(Ordering::SeqCst), calls);
}

#[test]
fn without_stills_the_thread_without_a_surface_never_steps_them() {
    let rig = start();
    rig.show_surface_and_wait_for_frames();
    assert_eq!(rig.destroy_surface(), SurfaceReleaseOutcome::Confirmed);
    std::thread::sleep(Duration::from_millis(150));
    assert_eq!(
        rig.stills.advance_calls_without_surface.load(Ordering::SeqCst),
        0,
        "поток без снимков и без surface будил себя"
    );
}

//! Поток рендера и снимки: бэкенд без поддержки снимков (fake) не оставляет `Begin` без итога.
//!
//! Проверяется поведение целиком: команда в ящик настоящего потока → ровно одно итоговое событие в приёмник снимка,
//! учёт в `RenderStatus::still_commands`, цель кодера без снимка отпускается (поверхность) и считается.

#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть сигнал о провале"
)]

mod support;

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use camera_contract::fixtures::camera_id;
use camera_contract::ids::LensFacing;
use camera_contract::viewfinder_crop::CropAspect;
use camera_contract::zoom::{ZoomCatalog, ZoomFraming, ZoomRatio};
use render_loop::config::RenderLoopConfig;
use render_loop::mailbox::InboxDelivery;
use render_loop::still::{StillBegin, StillCommand, StillEventSink};
use render_loop::thread::{RenderLoop, RenderLoopHandle, RenderLoopParts};
use shell_contract::clock::MonotonicNanos;
use shell_contract::orientation::DeviceOrientation;
use still_contract::develop::{DevelopEvent, PrimaryTarget, PrimaryTargetOffer};
use still_contract::frame_choice::NoFrameReason;
use still_contract::plan::{FrameSelection, PrimaryFormat, RawOutput, ShutterPress, StillId, StillOutputs, StillPlan};
use support::{FakeBackend, FakeClock, FakeCommands, Journal, Probe, fake_surface, wait_until};

const DEADLINE: Duration = Duration::from_secs(5);

#[derive(Default)]
struct Sink(Mutex<Vec<DevelopEvent>>);

impl StillEventSink for Sink {
    fn deliver(&self, event: DevelopEvent) {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).push(event);
    }
}

impl Sink {
    fn len(&self) -> usize {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).len()
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
            primary: PrimaryFormat::Jpeg,
            raw: RawOutput::Skip,
        },
    }
}

fn start() -> (RenderLoopHandle, Arc<Journal>, Arc<std::sync::atomic::AtomicBool>) {
    let journal = Arc::new(Journal::default());
    let probe = Probe::new(Arc::clone(&journal));
    let (clock, stop) = FakeClock::new();
    let handle = RenderLoop::spawn(RenderLoopParts {
        config: RenderLoopConfig::default(),
        backend: FakeBackend::new(probe),
        clock,
        commands: Arc::new(FakeCommands::default()),
        performance_hints: Arc::new(shell_contract::performance_hint::NoPerformanceHints::new(
            "тест без ADPF",
        )),
    })
    .expect("поток запускается");
    let _ = camera_id("unused");
    (handle, journal, stop)
}

#[test]
fn a_backend_without_stills_answers_every_begin_with_exactly_one_not_secured() {
    let (handle, journal, stop) = start();
    let inbox = handle.inbox();
    assert_eq!(
        inbox.surface_created(fake_surface(&journal, 1)),
        InboxDelivery::Delivered
    );
    let sinks: Vec<Arc<Sink>> = (1..=3).map(|_| Arc::new(Sink::default())).collect();
    for (index, sink) in sinks.iter().enumerate() {
        let events: Arc<dyn StillEventSink> = Arc::clone(sink) as Arc<dyn StillEventSink>;
        let delivery = inbox.still_command(StillCommand::Begin(StillBegin {
            plan: plan(index as u64 + 1),
            events,
        }));
        assert_eq!(delivery, InboxDelivery::Delivered);
    }
    assert!(
        wait_until(DEADLINE, || handle.status().still_commands.begins_unsupported == 3),
        "{:?}",
        handle.status().still_commands
    );
    for (index, sink) in sinks.iter().enumerate() {
        assert_eq!(sink.len(), 1, "снимок {}: ровно одно событие", index + 1);
        match sink.0.lock().unwrap_or_else(PoisonError::into_inner).as_slice() {
            [DevelopEvent::NotSecured { id, reason }] => {
                assert_eq!(*id, StillId(index as u64 + 1));
                assert_eq!(*reason, NoFrameReason::NoLiveCameraFrame);
            }
            other => panic!("ожидали NotSecured, получили {other:?}"),
        }
    }
    let log = handle.status().still_commands;
    assert_eq!(log.begins_accepted, 0);
    assert_eq!(log.targets_accepted, 0);
    stop.store(true, std::sync::atomic::Ordering::SeqCst);
    drop(handle.stop());
}

#[test]
fn an_encoder_target_for_a_backend_without_stills_is_dropped_and_counted() {
    let (handle, journal, stop) = start();
    let inbox = handle.inbox();
    assert_eq!(
        inbox.surface_created(fake_surface(&journal, 1)),
        InboxDelivery::Delivered
    );
    let delivery = inbox.still_command(StillCommand::Target(PrimaryTargetOffer::Ready {
        id: StillId(9),
        target: PrimaryTarget::Readback,
    }));
    assert_eq!(delivery, InboxDelivery::Delivered);
    assert!(
        wait_until(DEADLINE, || handle.status().still_commands.targets_dropped == 1),
        "{:?}",
        handle.status().still_commands
    );
    stop.store(true, std::sync::atomic::Ordering::SeqCst);
    drop(handle.stop());
}

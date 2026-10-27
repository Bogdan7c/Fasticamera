//! Тесты опустошения очереди кадров: эпоха и метка у предложенных кадров, исчерпание пула,
//! сбои, проба CPU-доступа и гонка «колбэк внутри NDK» против «закрытие reader-а».

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::os::fd::OwnedFd;
use std::os::unix::net::UnixStream;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use raw_frame_stream::ReleaseFences;
use shell_contract::hardware_buffer::BorrowedHardwareBuffer;
use shell_contract::sync_fence::GpuWaitFence;

use super::*;

const BOUND: Duration = Duration::from_secs(5);
const SETTLE: Duration = Duration::from_millis(100);
const EPOCH: StreamEpoch = StreamEpoch(7);

/// Кадр-пустышка: буфер — указатель на байт в куче, камере «возвращается» без последствий.
struct FakeFrame {
    storage: Box<u8>,
}

impl FakeFrame {
    fn boxed() -> Box<dyn ProducerFrame> {
        Box::new(Self { storage: Box::new(0) })
    }
}

// SAFETY: указатель ведёт на байт в куче, который живёт, пока жив сам `FakeFrame`.
unsafe impl ProducerFrame for FakeFrame {
    fn hardware_buffer(&self) -> BorrowedHardwareBuffer<'_> {
        let pointer = NonNull::from(&*self.storage).cast::<std::ffi::c_void>();
        // SAFETY: см. выше — буфер живёт не меньше заимствования.
        unsafe { BorrowedHardwareBuffer::from_raw(pointer) }
    }

    fn return_to_producer(self: Box<Self>, _release: ReleaseFences) {}
}

fn frame_at(nanos: i64) -> AcquireOutcome {
    AcquireOutcome::Frame(OfferedFrame {
        sensor_timestamp: SensorTimestamp::from_nanos(nanos),
        acquire_fence: GpuWaitFence::already_signaled(),
        frame: FakeFrame::boxed(),
    })
}

fn frame_with_pending_fence(nanos: i64) -> (AcquireOutcome, UnixStream) {
    let (ours, peer) = UnixStream::pair().expect("пара сокетов");
    let fence = GpuWaitFence::from_sync_fd(OwnedFd::from(ours));
    let outcome = AcquireOutcome::Frame(OfferedFrame {
        sensor_timestamp: SensorTimestamp::from_nanos(nanos),
        acquire_fence: fence,
        frame: FakeFrame::boxed(),
    });
    (outcome, peer)
}

/// Поддельный reader: заранее заданная очередь итогов и журнал проб.
struct ScriptedOps {
    script: RefCell<VecDeque<AcquireOutcome>>,
    acquire_calls: Cell<u32>,
    probe_result: RefCell<Result<usize, i32>>,
    probe_calls: Cell<u32>,
}

impl ScriptedOps {
    fn new(script: Vec<AcquireOutcome>) -> Self {
        Self {
            script: RefCell::new(script.into()),
            acquire_calls: Cell::new(0),
            probe_result: RefCell::new(Ok(5120)),
            probe_calls: Cell::new(0),
        }
    }
}

impl ReaderOps for ScriptedOps {
    fn acquire_next_frame(&self) -> AcquireOutcome {
        self.acquire_calls.set(self.acquire_calls.get() + 1);
        self.script.borrow_mut().pop_front().unwrap_or(AcquireOutcome::NoBuffer)
    }

    fn copy_first_row(&self, _frame: &dyn ProducerFrame) -> Result<usize, i32> {
        self.probe_calls.set(self.probe_calls.get() + 1);
        *self.probe_result.borrow()
    }
}

/// Поддельный поток: записывает всё, что ему предложили.
#[derive(Default)]
struct RecordingOffers {
    offered: RefCell<Vec<(StreamEpoch, SensorTimestamp)>>,
    pool_exhausted: RefCell<Vec<StreamEpoch>>,
    cpu_copies: Cell<u32>,
}

impl FrameOffers for RecordingOffers {
    fn offer_frame(&self, epoch: StreamEpoch, frame: OfferedFrame, _now: Instant) -> OfferOutcome {
        self.offered.borrow_mut().push((epoch, frame.sensor_timestamp));
        OfferOutcome::AwaitingResult
    }

    fn note_pool_exhausted(&self, epoch: StreamEpoch) {
        self.pool_exhausted.borrow_mut().push(epoch);
    }

    fn note_cpu_copy(&self) {
        self.cpu_copies.set(self.cpu_copies.get() + 1);
    }
}

/// Поддельные часы: отдают заданное «сейчас» или сбой.
struct FakeClock(Option<i64>);

impl BoottimeClock for FakeClock {
    fn now(&self) -> Option<SensorTimestamp> {
        self.0.map(SensorTimestamp::from_nanos)
    }
}

/// Всё окружение прохода одним куском.
struct Harness {
    gate: CallbackGate,
    diagnostics: BackendDiagnostics,
    offers: RecordingOffers,
    probe: CpuAccessProbe,
    clock: FakeClock,
}

impl Harness {
    fn new() -> Self {
        Self::with_clock(FakeClock(Some(1_000_000_000)))
    }

    fn with_clock(clock: FakeClock) -> Self {
        Self {
            clock,
            gate: CallbackGate::new(),
            diagnostics: BackendDiagnostics::new(4),
            offers: RecordingOffers::default(),
            probe: CpuAccessProbe::default(),
        }
    }

    fn drain(&self, ops: &ScriptedOps) -> DrainOutcome {
        drain_available_frames(&DrainPorts {
            gate: &self.gate,
            diagnostics: &self.diagnostics,
            ops,
            offers: &self.offers,
            epoch: EPOCH,
            probe: &self.probe,
            clock: &self.clock,
        })
    }
}

fn nanos(value: i64) -> SensorTimestamp {
    SensorTimestamp::from_nanos(value)
}

#[test]
fn drain_offers_every_frame_with_the_reader_epoch_and_its_own_timestamp() {
    let harness = Harness::new();
    let ops = ScriptedOps::new(vec![frame_at(100), frame_at(200), AcquireOutcome::NoBuffer]);
    assert_eq!(harness.drain(&ops), DrainOutcome::Drained { frames: 2 });
    assert_eq!(
        *harness.offers.offered.borrow(),
        vec![(EPOCH, nanos(100)), (EPOCH, nanos(200))],
        "порядок и метки кадров сохранены, эпоха — этого reader-а"
    );
    let snapshot = harness.diagnostics.snapshot();
    assert_eq!(snapshot.reader.frames_acquired, 2);
    assert_eq!(snapshot.callbacks_after_close, 0);
    assert!(harness.offers.pool_exhausted.borrow().is_empty());
}

#[test]
fn exhausted_pool_is_reported_once_and_the_drain_stops_leaving_the_frame_queued() {
    let harness = Harness::new();
    let ops = ScriptedOps::new(vec![frame_at(100), AcquireOutcome::PoolExhausted, frame_at(300)]);
    assert_eq!(harness.drain(&ops), DrainOutcome::PoolExhausted { frames: 1 });
    assert_eq!(
        *harness.offers.pool_exhausted.borrow(),
        vec![EPOCH],
        "ровно одно уведомление"
    );
    assert_eq!(ops.acquire_calls.get(), 2, "после отказа reader больше не спрашивают");
    assert_eq!(harness.offers.offered.borrow().len(), 1, "кадр за отказом не тронут");
    let snapshot = harness.diagnostics.snapshot().reader;
    assert_eq!(
        (
            snapshot.pool_exhausted,
            snapshot.frames_acquired,
            snapshot.acquire_errors
        ),
        (1, 1, 0)
    );
}

#[test]
fn acquire_failure_is_counted_with_its_code_and_stops_without_a_pool_note() {
    let harness = Harness::new();
    let ops = ScriptedOps::new(vec![frame_at(1), AcquireOutcome::Failed { status: -10000 }]);
    assert_eq!(
        harness.drain(&ops),
        DrainOutcome::AcquireFailed {
            status: -10000,
            frames: 1
        }
    );
    let snapshot = harness.diagnostics.snapshot().reader;
    assert_eq!(snapshot.acquire_errors, 1);
    assert_eq!(snapshot.last_acquire_error, Some(-10000));
    assert_eq!(snapshot.pool_exhausted, 0, "сбой — не исчерпание пула");
    assert!(harness.offers.pool_exhausted.borrow().is_empty());
}

#[test]
fn closed_gate_never_touches_the_reader_and_offers_nothing() {
    let harness = Harness::new();
    harness.gate.close();
    let ops = ScriptedOps::new(vec![frame_at(1)]);
    assert_eq!(harness.drain(&ops), DrainOutcome::GateClosed);
    assert_eq!(ops.acquire_calls.get(), 0, "в NDK после закрытия не заходим");
    assert!(harness.offers.offered.borrow().is_empty());
    assert_eq!(harness.diagnostics.snapshot().callbacks_after_close, 1);
}

#[test]
fn disarmed_probe_never_reads_pixels_through_the_cpu() {
    let harness = Harness::new();
    let ops = ScriptedOps::new(vec![frame_at(1), frame_at(2)]);
    harness.drain(&ops);
    assert_eq!(ops.probe_calls.get(), 0);
    assert_eq!(harness.offers.cpu_copies.get(), 0);
    assert_eq!(harness.diagnostics.snapshot().reader.cpu_access_probe_copies, 0);
}

#[test]
fn armed_probe_copies_each_frame_reports_it_to_the_stream_and_still_offers_the_frame() {
    let harness = Harness::new();
    harness.probe.arm_copy_first_row();
    let ops = ScriptedOps::new(vec![frame_at(1), frame_at(2)]);
    harness.drain(&ops);
    assert_eq!(ops.probe_calls.get(), 2);
    assert_eq!(harness.offers.cpu_copies.get(), 2, "поток узнал о каждой CPU-копии");
    assert_eq!(harness.diagnostics.snapshot().reader.cpu_access_probe_copies, 2);
    assert_eq!(harness.offers.offered.borrow().len(), 2);
}

#[test]
fn probe_that_cannot_lock_the_buffer_is_counted_as_failure_and_is_not_a_copy() {
    let harness = Harness::new();
    harness.probe.arm_copy_first_row();
    let ops = ScriptedOps::new(vec![frame_at(1)]);
    *ops.probe_result.borrow_mut() = Err(-22);
    harness.drain(&ops);
    let snapshot = harness.diagnostics.snapshot().reader;
    assert_eq!(
        (snapshot.cpu_access_probe_copies, snapshot.cpu_access_probe_failures),
        (0, 1)
    );
    assert_eq!(
        harness.offers.cpu_copies.get(),
        0,
        "копии не было — поток о ней не уведомляем"
    );
    assert_eq!(
        harness.offers.offered.borrow().len(),
        1,
        "сбой пробы не отнимает кадр у потока"
    );
}

#[test]
fn probe_skips_a_frame_whose_acquire_fence_is_still_pending() {
    let harness = Harness::new();
    harness.probe.arm_copy_first_row();
    let (pending, _peer) = frame_with_pending_fence(1);
    let ops = ScriptedOps::new(vec![pending]);
    harness.drain(&ops);
    assert_eq!(ops.probe_calls.get(), 0, "читать буфер до fence нельзя");
    assert_eq!(harness.diagnostics.snapshot().reader.cpu_access_probe_failures, 1);
    assert_eq!(harness.offers.offered.borrow().len(), 1);
}

/// Reader, который «застревает» внутри acquire, пока тест не разрешит продолжить: так
/// воспроизводится колбэк, вошедший в NDK к моменту закрытия.
struct BlockingOps {
    entered: mpsc::Sender<()>,
    proceed: std::sync::Mutex<mpsc::Receiver<()>>,
    acquire_calls: AtomicU32,
}

impl ReaderOps for BlockingOps {
    fn acquire_next_frame(&self) -> AcquireOutcome {
        self.acquire_calls.fetch_add(1, Ordering::SeqCst);
        self.entered.send(()).expect("тест ждёт входа");
        let proceed = self.proceed.lock().expect("замок цел");
        proceed.recv_timeout(BOUND).expect("тест разрешит продолжить");
        AcquireOutcome::NoBuffer
    }

    fn copy_first_row(&self, _frame: &dyn ProducerFrame) -> Result<usize, i32> {
        Err(-1)
    }
}

/// Поток, безопасный для передачи между потоками теста (без `RefCell`).
struct NullOffers;

impl FrameOffers for NullOffers {
    fn offer_frame(&self, _epoch: StreamEpoch, _frame: OfferedFrame, _now: Instant) -> OfferOutcome {
        OfferOutcome::StaleEpoch
    }

    fn note_pool_exhausted(&self, _epoch: StreamEpoch) {}

    fn note_cpu_copy(&self) {}
}

#[test]
fn closing_waits_for_a_callback_inside_the_reader_and_later_callbacks_stay_out() {
    let gate = CallbackGate::new();
    let diagnostics = BackendDiagnostics::new(4);
    let probe = CpuAccessProbe::default();
    let (entered_tx, entered_rx) = mpsc::channel();
    let (proceed_tx, proceed_rx) = mpsc::channel();
    let ops = BlockingOps {
        entered: entered_tx,
        proceed: std::sync::Mutex::new(proceed_rx),
        acquire_calls: AtomicU32::new(0),
    };
    let ports = DrainPorts {
        gate: &gate,
        diagnostics: &diagnostics,
        ops: &ops,
        offers: &NullOffers,
        epoch: EPOCH,
        probe: &probe,
        clock: &FakeClock(Some(0)),
    };
    let close_returned = AtomicBool::new(false);

    thread::scope(|scope| {
        let callback = scope.spawn(|| drain_available_frames(&ports));
        entered_rx.recv_timeout(BOUND).expect("колбэк вошёл в acquire");

        // Закрытие reader-а (поток камеры): `AImageReader_delete` без ворот завис бы здесь навсегда.
        let closer = scope.spawn(|| {
            gate.close();
            close_returned.store(true, Ordering::SeqCst);
        });
        thread::sleep(SETTLE);
        assert!(
            !close_returned.load(Ordering::SeqCst),
            "закрытие вернулось, пока колбэк внутри NDK: delete зависнет на замке reader-а"
        );

        proceed_tx.send(()).expect("колбэк ждёт");
        assert_eq!(
            callback.join().expect("колбэк завершился"),
            DrainOutcome::Drained { frames: 0 }
        );
        closer.join().expect("закрытие завершилось");
    });

    // После закрытия новый колбэк в reader не заходит.
    let calls_before = ops.acquire_calls.load(Ordering::SeqCst);
    assert_eq!(drain_available_frames(&ports), DrainOutcome::GateClosed);
    assert_eq!(ops.acquire_calls.load(Ordering::SeqCst), calls_before);
}

#[test]
fn arrival_age_is_recorded_exactly_for_each_frame() {
    // «Сейчас» = 1 000 000 000 нс; кадры начались на 30 мс и на 50 мс раньше.
    let harness = Harness::with_clock(FakeClock(Some(1_000_000_000)));
    let ops = ScriptedOps::new(vec![
        frame_at(970_000_000),
        frame_at(950_000_000),
        AcquireOutcome::NoBuffer,
    ]);
    assert_eq!(harness.drain(&ops), DrainOutcome::Drained { frames: 2 });
    let age = harness.diagnostics.snapshot().reader.arrival_age;
    assert_eq!(
        (age.samples, age.total_micros, age.min_micros, age.max_micros),
        (2, 80_000, Some(30_000), Some(50_000))
    );
    assert_eq!((age.clock_failures, age.negative), (0, 0));
}

#[test]
fn clock_failure_is_counted_without_a_sample_and_the_frame_is_still_offered() {
    let harness = Harness::with_clock(FakeClock(None));
    let ops = ScriptedOps::new(vec![frame_at(100), AcquireOutcome::NoBuffer]);
    assert_eq!(harness.drain(&ops), DrainOutcome::Drained { frames: 1 });
    let age = harness.diagnostics.snapshot().reader.arrival_age;
    assert_eq!(
        (age.samples, age.clock_failures, age.negative, age.min_micros),
        (0, 1, 0, None)
    );
    assert_eq!(harness.offers.offered.borrow().len(), 1);
}

#[test]
fn future_sensor_timestamp_is_counted_as_negative_without_a_sample() {
    let harness = Harness::with_clock(FakeClock(Some(1_000)));
    let ops = ScriptedOps::new(vec![frame_at(2_000), AcquireOutcome::NoBuffer]);
    assert_eq!(harness.drain(&ops), DrainOutcome::Drained { frames: 1 });
    let age = harness.diagnostics.snapshot().reader.arrival_age;
    assert_eq!(
        (age.samples, age.clock_failures, age.negative, age.max_micros),
        (0, 0, 1, None)
    );
    assert_eq!(harness.offers.offered.borrow().len(), 1);
}

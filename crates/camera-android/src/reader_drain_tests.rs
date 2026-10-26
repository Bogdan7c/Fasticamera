//! Тесты опустошения очереди кадров и гонки «колбэк внутри NDK» против «закрытие reader-а».

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use super::*;

const BOUND: Duration = Duration::from_secs(5);
const SETTLE: Duration = Duration::from_millis(100);

/// Поддельный reader: заранее заданная очередь итогов и журнал отпущенных кадров.
struct ScriptedOps {
    script: RefCell<VecDeque<AcquireOutcome<u32>>>,
    released: RefCell<Vec<u32>>,
    acquire_calls: Cell<u32>,
}

impl ScriptedOps {
    fn new(script: Vec<AcquireOutcome<u32>>) -> Self {
        Self {
            script: RefCell::new(script.into()),
            released: RefCell::new(Vec::new()),
            acquire_calls: Cell::new(0),
        }
    }
}

impl ReaderOps for ScriptedOps {
    type Image = u32;

    fn acquire_next_image(&self) -> AcquireOutcome<u32> {
        self.acquire_calls.set(self.acquire_calls.get() + 1);
        self.script.borrow_mut().pop_front().unwrap_or(AcquireOutcome::NoBuffer)
    }

    fn release_image(&self, image: u32) {
        self.released.borrow_mut().push(image);
    }
}

#[test]
fn drain_acquires_and_releases_every_frame_until_the_queue_is_empty() {
    let gate = CallbackGate::new();
    let diagnostics = BackendDiagnostics::new(4);
    let ops = ScriptedOps::new(vec![
        AcquireOutcome::Image(10),
        AcquireOutcome::Image(11),
        AcquireOutcome::NoBuffer,
    ]);
    assert_eq!(
        drain_available_images(&gate, &diagnostics, &ops),
        DrainOutcome::Drained { frames: 2 }
    );
    assert_eq!(*ops.released.borrow(), vec![10, 11]);
    let snapshot = diagnostics.snapshot();
    assert_eq!(
        (snapshot.raw_sink.frames_acquired, snapshot.raw_sink.frames_released),
        (2, 2)
    );
    assert_eq!(snapshot.callbacks_after_close, 0);
}

#[test]
fn acquire_failure_is_counted_with_its_code_and_stops_the_drain() {
    let gate = CallbackGate::new();
    let diagnostics = BackendDiagnostics::new(4);
    let ops = ScriptedOps::new(vec![
        AcquireOutcome::Image(1),
        AcquireOutcome::Failed { status: -30002 },
    ]);
    assert_eq!(
        drain_available_images(&gate, &diagnostics, &ops),
        DrainOutcome::AcquireFailed {
            status: -30002,
            frames: 1
        }
    );
    let snapshot = diagnostics.snapshot();
    assert_eq!(snapshot.raw_sink.acquire_errors, 1);
    assert_eq!(snapshot.raw_sink.last_acquire_error, Some(-30002));
    assert_eq!(snapshot.raw_sink.frames_released, 1);
}

#[test]
fn closed_gate_never_touches_the_reader_and_is_counted() {
    let gate = CallbackGate::new();
    let diagnostics = BackendDiagnostics::new(4);
    gate.close();
    let ops = ScriptedOps::new(vec![AcquireOutcome::Image(1)]);
    assert_eq!(
        drain_available_images(&gate, &diagnostics, &ops),
        DrainOutcome::GateClosed
    );
    assert_eq!(ops.acquire_calls.get(), 0, "в NDK после закрытия не заходим");
    assert_eq!(diagnostics.snapshot().callbacks_after_close, 1);
}

/// Reader, который «застревает» внутри acquire, пока тест не разрешит продолжить: так
/// воспроизводится колбэк, вошедший в NDK к моменту закрытия.
struct BlockingOps {
    entered: mpsc::Sender<()>,
    proceed: std::sync::Mutex<mpsc::Receiver<()>>,
    acquire_calls: AtomicU32,
}

impl ReaderOps for BlockingOps {
    type Image = u32;

    fn acquire_next_image(&self) -> AcquireOutcome<u32> {
        self.acquire_calls.fetch_add(1, Ordering::SeqCst);
        self.entered.send(()).expect("тест ждёт входа");
        let proceed = self.proceed.lock().expect("замок цел");
        proceed.recv_timeout(BOUND).expect("тест разрешит продолжить");
        AcquireOutcome::NoBuffer
    }

    fn release_image(&self, _image: u32) {}
}

#[test]
fn closing_waits_for_a_callback_inside_the_reader_and_later_callbacks_stay_out() {
    let gate = CallbackGate::new();
    let diagnostics = BackendDiagnostics::new(4);
    let (entered_tx, entered_rx) = mpsc::channel();
    let (proceed_tx, proceed_rx) = mpsc::channel();
    let ops = BlockingOps {
        entered: entered_tx,
        proceed: std::sync::Mutex::new(proceed_rx),
        acquire_calls: AtomicU32::new(0),
    };
    let close_returned = AtomicBool::new(false);

    thread::scope(|scope| {
        let callback = scope.spawn(|| drain_available_images(&gate, &diagnostics, &ops));
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
    assert_eq!(
        drain_available_images(&gate, &diagnostics, &ops),
        DrainOutcome::GateClosed
    );
    assert_eq!(ops.acquire_calls.load(Ordering::SeqCst), calls_before);
}

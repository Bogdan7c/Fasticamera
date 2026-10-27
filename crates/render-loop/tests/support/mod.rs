//! Общие fake-объекты для функциональных тестов потока рендера: бэкенд, часы кадров,
//! команды оболочки и окно. Все они пишут события в один упорядоченный журнал.

#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "fake-объекты тестов: падение и есть сигнал о провале"
)]
#![allow(dead_code, reason = "каждый тестовый файл использует свою часть общих fake-объектов")]

use std::collections::{BTreeMap, VecDeque};
use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use render_loop::backend::{
    AttachError, BackendDiagnostics, BackendError, DisplayBackend, FrameTiming, PastPresentation, PresentOutcome,
};
use render_loop::thread::{FrameClock, FrameTick, FrameWait};
use shell_contract::clock::MonotonicNanos;
use shell_contract::commands::{CommandOutcome, FrameRate, FrameRateOutcome, ScreenWake, ShellCommands};
use shell_contract::surface::{NativeSurface, PlatformWindow, SurfaceGeneration, SurfaceSize};
use shell_contract::vsync::{FrameTimelines, VsyncTimeline};

pub const SIZE: SurfaceSize = SurfaceSize {
    width_px: 1440,
    height_px: 3216,
};

/// Сколько «объектов GPU» fake создаёт на один attach.
pub const OBJECTS_PER_ATTACH: u64 = 3;

/// 120 Гц в наносекундах.
const PERIOD_NANOS: i64 = 8_333_333;

/// Упорядоченный журнал событий всех fake-объектов.
#[derive(Default)]
pub struct Journal(Mutex<Vec<String>>);

impl Journal {
    pub fn record(&self, entry: impl Into<String>) {
        self.0.lock().expect("журнал").push(entry.into());
    }

    pub fn entries(&self) -> Vec<String> {
        self.0.lock().expect("журнал").clone()
    }

    pub fn position(&self, entry: &str) -> Option<usize> {
        self.entries().iter().position(|candidate| candidate == entry)
    }

    pub fn count(&self, entry: &str) -> usize {
        self.entries().iter().filter(|candidate| *candidate == entry).count()
    }
}

/// Общие данные fake-бэкенда: доступны и потоку рендера, и тесту.
pub struct Probe {
    pub journal: Arc<Journal>,
    pub live_objects: AtomicU64,
    /// Номера кадров, которые бэкенд получил на отрисовку.
    pub rendered_frames: Mutex<Vec<u64>>,
    /// Имена потоков, из которых вызывали `attach`.
    pub attach_thread_names: Mutex<Vec<String>>,
    /// Паниковать на кадре с этим номером (`u64::MAX` — никогда).
    pub panic_on_frame: AtomicU64,
    pub panic_on_detach: AtomicBool,
    pub fail_attach: AtomicBool,
    /// Сколько спать внутри detach, мс (делает порядок «detach, затем подтверждение» заметным).
    pub detach_delay_ms: AtomicU64,
    /// Заготовленные исходы кадров; когда пусто — `Presented`.
    pub scripted_outcomes: Mutex<VecDeque<PresentOutcome>>,
    pub resizes: Mutex<Vec<SurfaceSize>>,
    /// Сколько спать внутри resize, мс (имитация ожидания GPU при пересоздании цепочки).
    pub resize_delay_ms: AtomicU64,
    /// Пока true, `render_and_present` блокируется (поток рендера «занят кадром»): тест
    /// детерминированно кладёт события в ящик, пока поток внутри кадра.
    pub hold_frames: AtomicBool,
    /// true, пока бэкенд стоит внутри удерживаемого кадра.
    pub frame_in_progress: AtomicBool,
    /// Что fake-бэкенд сообщает как срабатывания известных отходов validation.
    pub known_deviations: Mutex<std::collections::BTreeMap<String, u64>>,
}

impl Probe {
    pub fn new(journal: Arc<Journal>) -> Arc<Self> {
        Arc::new(Self {
            journal,
            live_objects: AtomicU64::new(0),
            rendered_frames: Mutex::new(Vec::new()),
            attach_thread_names: Mutex::new(Vec::new()),
            panic_on_frame: AtomicU64::new(u64::MAX),
            panic_on_detach: AtomicBool::new(false),
            fail_attach: AtomicBool::new(false),
            detach_delay_ms: AtomicU64::new(0),
            scripted_outcomes: Mutex::new(VecDeque::new()),
            resizes: Mutex::new(Vec::new()),
            resize_delay_ms: AtomicU64::new(0),
            hold_frames: AtomicBool::new(false),
            frame_in_progress: AtomicBool::new(false),
            known_deviations: Mutex::new(std::collections::BTreeMap::new()),
        })
    }

    pub fn live_objects(&self) -> u64 {
        self.live_objects.load(Ordering::SeqCst)
    }

    pub fn rendered_frames(&self) -> Vec<u64> {
        self.rendered_frames.lock().expect("кадры").clone()
    }
}

/// Fake-бэкенд: считает кадры, «создаёт» объекты GPU и пишет журнал.
pub struct FakeBackend {
    probe: Arc<Probe>,
    held_surface: Option<NativeSurface>,
    unreported_frames: Vec<u64>,
}

impl FakeBackend {
    pub fn new(probe: Arc<Probe>) -> Self {
        Self {
            probe,
            held_surface: None,
            unreported_frames: Vec::new(),
        }
    }
}

impl DisplayBackend for FakeBackend {
    fn attach(&mut self, surface: NativeSurface, size: SurfaceSize) -> Result<(), AttachError> {
        let generation = surface.generation().value();
        self.probe
            .attach_thread_names
            .lock()
            .expect("имена")
            .push(thread::current().name().unwrap_or("<без имени>").to_owned());
        if self.probe.fail_attach.load(Ordering::SeqCst) {
            self.probe.journal.record(format!("attach_failed({generation})"));
            // Контракт: при ошибке бэкенд ничего не держит, surface уничтожается здесь.
            drop(surface);
            return Err(AttachError::SwapchainFailed(BackendError {
                operation: "create_swapchain".to_owned(),
                reason: "fake: отказ".to_owned(),
            }));
        }
        self.probe.live_objects.fetch_add(OBJECTS_PER_ATTACH, Ordering::SeqCst);
        self.probe
            .journal
            .record(format!("attach({generation},{}x{})", size.width_px, size.height_px));
        self.held_surface = Some(surface);
        Ok(())
    }

    fn resize(&mut self, size: SurfaceSize) -> Result<(), BackendError> {
        self.probe.resizes.lock().expect("размеры").push(size);
        thread::sleep(Duration::from_millis(self.probe.resize_delay_ms.load(Ordering::SeqCst)));
        self.probe
            .journal
            .record(format!("resize({}x{})", size.width_px, size.height_px));
        Ok(())
    }

    fn render_and_present(&mut self, timing: FrameTiming) -> PresentOutcome {
        if timing.frame_index == self.probe.panic_on_frame.load(Ordering::SeqCst) {
            panic!("fake backend: boom на кадре {}", timing.frame_index);
        }
        if self.probe.hold_frames.load(Ordering::SeqCst) {
            self.probe.frame_in_progress.store(true, Ordering::SeqCst);
            while self.probe.hold_frames.load(Ordering::SeqCst) {
                thread::sleep(Duration::from_millis(1));
            }
            self.probe.frame_in_progress.store(false, Ordering::SeqCst);
        }
        self.probe
            .rendered_frames
            .lock()
            .expect("кадры")
            .push(timing.frame_index);
        let scripted = self.probe.scripted_outcomes.lock().expect("исходы").pop_front();
        let outcome = scripted.unwrap_or(PresentOutcome::Presented);
        if outcome == PresentOutcome::Presented {
            self.unreported_frames.push(timing.frame_index);
        }
        outcome
    }

    fn detach(&mut self) -> Option<NativeSurface> {
        let surface = self.held_surface.take()?;
        let generation = surface.generation().value();
        self.probe.journal.record(format!("detach_started({generation})"));
        if self.probe.panic_on_detach.load(Ordering::SeqCst) {
            panic!("fake backend: detach упал");
        }
        thread::sleep(Duration::from_millis(self.probe.detach_delay_ms.load(Ordering::SeqCst)));
        self.probe.live_objects.fetch_sub(OBJECTS_PER_ATTACH, Ordering::SeqCst);
        self.probe.journal.record(format!("detach_done({generation})"));
        Some(surface)
    }

    fn diagnostics(&mut self) -> BackendDiagnostics {
        BackendDiagnostics {
            live_objects: self.probe.live_objects(),
            // Один условный вид: хватает, чтобы проверить, что перепись по видам доходит до статуса.
            live_objects_by_kind: match self.probe.live_objects() {
                0 => BTreeMap::new(),
                live => BTreeMap::from([("Fake".to_owned(), live)]),
            },
            validation_errors: 0,
            validation_warnings: 0,
            detach_failures: 0,
            display: None,
            known_deviations: self.probe.known_deviations.lock().expect("отходы").clone(),
            camera: None,
            viewfinder: None,
        }
    }

    fn drain_past_presentations(&mut self) -> Vec<PastPresentation> {
        self.unreported_frames
            .drain(..)
            .map(|frame_index| PastPresentation {
                present_id: u32::try_from(frame_index).expect("номер показа"),
                frame_index,
                actual_present: MonotonicNanos::from_nanos(
                    1_000_000_000 + i64::try_from(frame_index).expect("индекс") * PERIOD_NANOS,
                ),
            })
            .collect()
    }
}

/// Окно, записывающее в журнал момент своего освобождения.
pub struct FakeWindow {
    generation: u64,
    journal: Arc<Journal>,
}

impl PlatformWindow for FakeWindow {
    fn raw_window(&self) -> NonNull<c_void> {
        NonNull::dangling()
    }
}

impl Drop for FakeWindow {
    fn drop(&mut self) {
        self.journal.record(format!("window_released({})", self.generation));
    }
}

pub fn fake_surface(journal: &Arc<Journal>, generation: u64) -> NativeSurface {
    NativeSurface::new(
        SurfaceGeneration::new(generation),
        Box::new(FakeWindow {
            generation,
            journal: Arc::clone(journal),
        }),
    )
}

/// Часы, тикающие примерно раз в миллисекунду (быстрее реального vsync — тесты короче).
pub struct FakeClock {
    sequence: u64,
    stopped: Arc<AtomicBool>,
}

impl FakeClock {
    pub fn new() -> (Self, Arc<AtomicBool>) {
        let stopped = Arc::new(AtomicBool::new(false));
        (
            Self {
                sequence: 0,
                stopped: Arc::clone(&stopped),
            },
            stopped,
        )
    }
}

impl FrameClock for FakeClock {
    fn wait_next_frame(&mut self, timeout: Duration) -> FrameWait {
        if self.stopped.load(Ordering::SeqCst) {
            return FrameWait::Stopped;
        }
        thread::sleep(timeout.min(Duration::from_millis(1)));
        self.sequence += 1;
        FrameWait::Tick(FrameTick {
            sequence: self.sequence,
            expected_present: MonotonicNanos::from_nanos(
                i64::try_from(self.sequence).expect("номер тика") * PERIOD_NANOS,
            ),
            deadline: MonotonicNanos::from_nanos(
                i64::try_from(self.sequence).expect("номер тика") * PERIOD_NANOS - PERIOD_NANOS / 2,
            ),
            skipped_ticks: 0,
            timelines: FrameTimelines::only(VsyncTimeline {
                vsync_id: i64::try_from(self.sequence).expect("номер тика"),
                expected_present: MonotonicNanos::from_nanos(
                    i64::try_from(self.sequence).expect("номер тика") * PERIOD_NANOS,
                ),
                deadline: MonotonicNanos::from_nanos(
                    i64::try_from(self.sequence).expect("номер тика") * PERIOD_NANOS - PERIOD_NANOS / 2,
                ),
            }),
        })
    }
}

/// Команды оболочки, запоминающие запрошенную частоту.
#[derive(Default)]
pub struct FakeCommands {
    pub requested_rates: Mutex<Vec<f32>>,
    /// Все вызовы `keep_screen_on` по порядку.
    pub screen_wake_calls: Mutex<Vec<ScreenWake>>,
    /// Общий журнал: вызовы `keep_screen_on` пишутся в него рядом с событиями бэкенда и окна,
    /// чтобы проверять порядок «attach -> KeepOn», «detach -> AllowSleep».
    pub journal: Option<Arc<Journal>>,
}

impl FakeCommands {
    pub fn with_journal(journal: Arc<Journal>) -> Self {
        Self {
            journal: Some(journal),
            ..Self::default()
        }
    }

    pub fn screen_wake_calls(&self) -> Vec<ScreenWake> {
        self.screen_wake_calls.lock().expect("вызовы").clone()
    }
}

impl ShellCommands for FakeCommands {
    fn request_camera_permission(&self) -> CommandOutcome {
        CommandOutcome::Queued
    }

    fn set_preferred_frame_rate(&self, rate: FrameRate) -> FrameRateOutcome {
        self.requested_rates.lock().expect("частоты").push(rate.hertz());
        FrameRateOutcome::DeferredUntilSurface
    }

    fn open_system_gallery(&self) -> CommandOutcome {
        CommandOutcome::Queued
    }

    fn keep_screen_on(&self, wake: ScreenWake) -> CommandOutcome {
        self.screen_wake_calls.lock().expect("вызовы").push(wake);
        if let Some(journal) = &self.journal {
            journal.record(format!("keep_screen_on({wake:?})"));
        }
        CommandOutcome::Queued
    }
}

/// Ждёт, пока условие станет истинным, не дольше `deadline`. Возвращает, дождались ли.
pub fn wait_until(deadline: Duration, mut condition: impl FnMut() -> bool) -> bool {
    let started = Instant::now();
    while started.elapsed() < deadline {
        if condition() {
            return true;
        }
        thread::sleep(Duration::from_millis(2));
    }
    condition()
}

/// Часы, которые паникуют на тике с заданным номером (имитация сбоя вне бэкенда).
pub struct PanickingClock {
    inner: FakeClock,
    ticks_before_panic: u64,
    ticks_given: u64,
}

impl PanickingClock {
    pub fn new(ticks_before_panic: u64) -> Self {
        let (inner, _stopped) = FakeClock::new();
        Self {
            inner,
            ticks_before_panic,
            ticks_given: 0,
        }
    }
}

impl FrameClock for PanickingClock {
    fn wait_next_frame(&mut self, timeout: Duration) -> FrameWait {
        if self.ticks_given >= self.ticks_before_panic {
            panic!("fake clock: boom после {} тиков", self.ticks_given);
        }
        self.ticks_given += 1;
        self.inner.wait_next_frame(timeout)
    }
}

/// Часы, которые блокируются в ожидании тика, пока тест не откроет ворота.
pub struct GatedClock {
    inner: FakeClock,
    gate: Arc<GateState>,
}

/// Общее состояние ворот часов: тест видит, что поток ждёт тик, и разрешает его выдать.
#[derive(Default)]
pub struct GateState {
    /// Поток рендера сейчас стоит в ожидании тика.
    pub waiting: AtomicBool,
    /// Пока true, тик не выдаётся.
    pub closed: AtomicBool,
}

impl GatedClock {
    pub fn new() -> (Self, Arc<GateState>) {
        let gate = Arc::new(GateState::default());
        gate.closed.store(true, Ordering::SeqCst);
        let (inner, _stopped) = FakeClock::new();
        (
            Self {
                inner,
                gate: Arc::clone(&gate),
            },
            gate,
        )
    }
}

impl FrameClock for GatedClock {
    fn wait_next_frame(&mut self, timeout: Duration) -> FrameWait {
        self.gate.waiting.store(true, Ordering::SeqCst);
        while self.gate.closed.load(Ordering::SeqCst) {
            thread::sleep(Duration::from_millis(1));
        }
        self.gate.waiting.store(false, Ordering::SeqCst);
        self.inner.wait_next_frame(timeout)
    }
}

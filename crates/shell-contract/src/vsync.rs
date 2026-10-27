//! Тики vsync: когда экран показывает следующий кадр (сессия 03, задел для пейсинга рендера).
//!
//! Поток рендера не должен гадать, когда будить GPU: Choreographer платформы сообщает момент
//! vsync и «линию кадра» — когда кадр, поставленный сейчас, реально попадёт на экран
//! (`expected_present`) и к какому сроку его надо успеть сдать (`deadline`). Этот модуль —
//! нейтральный словарь: тик, ящик «самый свежий тик», RAII-подписка и трейт источника.
//! Реализация для телефона — `android-shell` (поток с `ALooper` и `AChoreographer`).
//!
//! Все времена тика — `CLOCK_MONOTONIC` ([`MonotonicNanos`]): это же часы, в которых
//! `VK_GOOGLE_display_timing` принимает `desiredPresentTime`.
//!
//! # Почему «самый свежий тик», а не очередь
//! Если потребитель задержался, накопленные устаревшие vsync ему не нужны: рисовать надо под
//! ближайший показ. Ящик [`VsyncSlot`] хранит один тик; новый тик заменяет непрочитанный,
//! а потребитель узнаёт, сколько тиков он пропустил (`skipped`), и видит это в счётчиках.

use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use crate::clock::MonotonicNanos;

#[path = "vsync_timelines.rs"]
mod timelines;

pub use timelines::{FrameTimelines, FrameTimelinesError, MAX_FRAME_TIMELINES, VsyncTimeline};

/// Один vsync: что сообщил Choreographer.
///
/// Плоские поля `expected_present`/`deadline`/`vsync_id` — предпочитаемая линия кадра (как до
/// сессии 07: старые потребители не меняются). Все линии — в `timelines`; инвариант «плоские
/// поля = `timelines.preferred()`» держит конструктор [`VsyncTick::from_timelines`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VsyncTick {
    /// Порядковый номер тика источника, начиная с [`VsyncSlot::FIRST_SEQUENCE`]; растёт строго.
    pub sequence: u64,
    /// Момент vsync (`AChoreographerFrameCallbackData_getFrameTimeNanos`).
    pub frame_time: MonotonicNanos,
    /// Когда кадр этой линии будет показан на экране.
    pub expected_present: MonotonicNanos,
    /// Срок, к которому кадр надо сдать композитору, чтобы он попал в `expected_present`.
    pub deadline: MonotonicNanos,
    /// Идентификатор линии кадра в SurfaceFlinger (нужен для отчётов о показе).
    pub vsync_id: i64,
    /// Все линии кадра этого vsync (предпочитаемая среди них).
    pub timelines: FrameTimelines,
}

impl VsyncTick {
    /// Тик из списка линий: плоские поля берутся из предпочитаемой линии.
    pub fn from_timelines(sequence: u64, frame_time: MonotonicNanos, timelines: FrameTimelines) -> Self {
        let preferred = timelines.preferred();
        Self {
            sequence,
            frame_time,
            expected_present: preferred.expected_present,
            deadline: preferred.deadline,
            vsync_id: preferred.vsync_id,
            timelines,
        }
    }
}

/// Итог ожидания следующего тика.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[expect(
    clippy::large_enum_variant,
    reason = "тик с линиями кадра (~260 байт) возвращается по значению раз на vsync; Box выделял бы память на каждом vsync"
)]
pub enum VsyncWait {
    /// Свежий тик. `skipped` — сколько тиков между последним виденным и этим потребитель не увидел.
    Tick { tick: VsyncTick, skipped: u64 },
    /// За отведённое время нового тика не было (источник жив, но молчит).
    TimedOut,
    /// Источник остановлен: ждать больше нечего.
    Stopped,
}

/// Счётчики ящика: монотонные с создания.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct VsyncSlotCounters {
    /// Принятые тики (не считая отброшенных устаревших).
    pub published: u64,
    /// Тики, заменённые новыми до того, как потребитель их забрал.
    pub overwritten: u64,
    /// Тики, отброшенные из-за номера не больше уже принятого (нарушение порядка источника).
    pub stale_ignored: u64,
    /// Тики, отданные потребителю.
    pub delivered: u64,
}

#[derive(Debug, Default)]
struct SlotState {
    latest: Option<VsyncTick>,
    /// Забрал ли потребитель `latest` (иначе следующая публикация считается перезаписью).
    latest_delivered: bool,
    /// Номер последнего принятого тика; новые обязаны быть строго больше.
    last_published_sequence: u64,
    stopped: bool,
    counters: VsyncSlotCounters,
}

/// Ящик «самый свежий тик» с блокирующим ожиданием. Один производитель, один потребитель.
#[derive(Debug, Default)]
pub struct VsyncSlot {
    state: Mutex<SlotState>,
    tick_or_stop: Condvar,
}

impl VsyncSlot {
    /// Номер первого тика. Ноль зарезервирован под «ничего не видел» ([`Self::NOTHING_SEEN`]).
    pub const FIRST_SEQUENCE: u64 = 1;
    /// Значение `after_sequence` для потребителя, который ещё не видел ни одного тика.
    pub const NOTHING_SEEN: u64 = 0;

    pub fn new() -> Self {
        Self::default()
    }

    /// Кладёт тик, заменяя непрочитанный. Тик с номером не больше принятого игнорируется
    /// и считается (`stale_ignored`): порядок источника нарушен, потребитель не должен «откатиться».
    /// После [`Self::stop`] публикация — пустая операция.
    pub fn publish(&self, tick: VsyncTick) {
        let mut state = self.lock_state();
        if state.stopped {
            return;
        }
        if tick.sequence <= state.last_published_sequence {
            state.counters.stale_ignored += 1;
            return;
        }
        if state.latest.is_some() && !state.latest_delivered {
            state.counters.overwritten += 1;
        }
        state.latest = Some(tick);
        state.latest_delivered = false;
        state.last_published_sequence = tick.sequence;
        state.counters.published += 1;
        drop(state);
        self.tick_or_stop.notify_all();
    }

    /// Ждёт тик с номером больше `after_sequence` не дольше `timeout`.
    /// Остановка приоритетнее готового тика: потребитель должен выйти, а не рисовать дальше.
    pub fn wait_next(&self, after_sequence: u64, timeout: Duration) -> VsyncWait {
        // Крайний срок считаем один раз: ложные пробуждения не удлиняют ожидание.
        let deadline = Instant::now().checked_add(timeout);
        let mut state = self.lock_state();
        loop {
            if state.stopped {
                return VsyncWait::Stopped;
            }
            if let Some(tick) = state.latest
                && tick.sequence > after_sequence
            {
                state.latest_delivered = true;
                state.counters.delivered += 1;
                let skipped = tick.sequence - after_sequence - 1;
                return VsyncWait::Tick { tick, skipped };
            }
            let remaining = match deadline {
                Some(deadline) => deadline.saturating_duration_since(Instant::now()),
                // Срок не представим (огромный timeout): ждём порциями, пока не придёт тик или стоп.
                None => Duration::from_secs(3600),
            };
            if remaining.is_zero() {
                return VsyncWait::TimedOut;
            }
            let (guard, _) = self
                .tick_or_stop
                .wait_timeout(state, remaining)
                .unwrap_or_else(PoisonError::into_inner);
            state = guard;
        }
    }

    /// Останавливает ящик: все ожидающие просыпаются с [`VsyncWait::Stopped`]. Повторный вызов безвреден.
    pub fn stop(&self) {
        self.lock_state().stopped = true;
        self.tick_or_stop.notify_all();
    }

    pub fn is_stopped(&self) -> bool {
        self.lock_state().stopped
    }

    pub fn counters(&self) -> VsyncSlotCounters {
        self.lock_state().counters
    }

    fn lock_state(&self) -> MutexGuard<'_, SlotState> {
        // Состояние — простые поля, паника другого потока не оставляет его в «полуобновлённом»
        // виде, поэтому отравление замка не повод ронять потребителя.
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Работающая подписка на vsync. Пока она жива, источник публикует тики; уничтожение
/// сначала будит ожидающих (`Stopped`), потом останавливает производителя.
pub struct VsyncSubscription {
    slot: Arc<VsyncSlot>,
    /// Объект источника, чей `Drop` останавливает производителя (поток, колбэк…).
    /// Поля освобождаются после тела `Drop::drop`, где ящик уже остановлен.
    _stop_on_drop: Box<dyn Send>,
}

impl VsyncSubscription {
    /// `stop_on_drop` — объект источника, чей `Drop` останавливает производителя тиков.
    pub fn new(slot: Arc<VsyncSlot>, stop_on_drop: Box<dyn Send>) -> Self {
        Self {
            slot,
            _stop_on_drop: stop_on_drop,
        }
    }

    /// См. [`VsyncSlot::wait_next`].
    pub fn wait_next(&self, after_sequence: u64, timeout: Duration) -> VsyncWait {
        self.slot.wait_next(after_sequence, timeout)
    }

    pub fn counters(&self) -> VsyncSlotCounters {
        self.slot.counters()
    }

    /// Ящик подписки — чтобы отдать ожидание другому потоку (например, остановить его снаружи).
    pub fn slot(&self) -> &Arc<VsyncSlot> {
        &self.slot
    }
}

impl Drop for VsyncSubscription {
    fn drop(&mut self) {
        // Сначала будим ожидающих: потом поле `_stop_on_drop` остановит производителя (join потока).
        self.slot.stop();
    }
}

/// Почему источник vsync не запустился. Варианты различимы: «версия слишком старая» и
/// «поток без looper» лечатся по-разному.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum VsyncSourceError {
    #[error("vsync с линиями кадра требует Android API {required}, на устройстве {actual}")]
    UnsupportedApiLevel { required: u32, actual: u32 },
    #[error("у потока нет looper: Choreographer недоступен")]
    NoLooper,
    #[error("сбой платформы при запуске vsync: {reason}")]
    PlatformFailure { reason: String },
}

/// Источник vsync. Тики идут, пока жива подписка; запуск по намерению (батарея, как у датчиков).
pub trait VsyncSource: Send + Sync {
    fn start_vsync(&self) -> Result<VsyncSubscription, VsyncSourceError>;
}

#[cfg(test)]
#[path = "vsync_tests.rs"]
mod tests;

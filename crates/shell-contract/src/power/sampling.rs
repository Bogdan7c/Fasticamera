//! Неблокирующий источник показаний батареи: кольцо, подписка и учёт сбоев.
//!
//! Модель как у гироскопа ([`crate::motion::MotionRing`]): один писатель (поток опроса
//! платформы) кладёт показания в ограниченное кольцо, потребитель (сценарий замера) забирает
//! их, когда ему удобно. Ни один метод потребителя не ждёт платформу: замок кольца держится
//! только на время копирования. Если потребитель не успевает забирать, самые старые
//! показания вытесняются — со счётчиком `evicted_unread`, не молча.

use std::collections::VecDeque;
use std::num::NonZeroUsize;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use serde::Serialize;

use super::BatterySample;

/// Период опроса батареи (не ноль).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PowerSamplingPeriod(Duration);

impl PowerSamplingPeriod {
    /// `None` для нулевого периода: опрос «без паузы» загрузил бы binder и сам изменил бы ток.
    pub fn new(period: Duration) -> Option<Self> {
        (!period.is_zero()).then_some(Self(period))
    }

    pub const fn duration(self) -> Duration {
        self.0
    }
}

/// Что запросить у источника: как часто читать и сколько непрочитанных показаний держать.
/// Оба значения — из конфига вызывающего (сценарий замера), здесь чисел по умолчанию нет.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PowerSamplingRequest {
    pub period: PowerSamplingPeriod,
    pub ring_capacity: NonZeroUsize,
    /// Единица сырого `CURRENT_NOW` на этом устройстве: платформа переводит в мкА до кольца.
    pub current_now_unit: super::CurrentNowUnit,
}

/// Что стало с показанием при записи в кольцо.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerPushOutcome {
    Stored,
    /// Записано, самое старое непрочитанное вытеснено (потребитель отстал).
    StoredEvictingOldest,
}

/// Почему очередное чтение батареи не дало показания. Сбой чтения не останавливает опрос:
/// он считается по видам и запоминается последним, чтобы отчёт мог объяснить дыры.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "failure", rename_all = "snake_case")]
pub enum PowerReadFailure {
    /// Activity не подключена — некому прочитать `BatteryManager`.
    NotAttached,
    /// Вызов платформы завершился ошибкой JNI или исключением Java.
    PlatformCallFailed { reason: String },
    /// Платформа ответила, но ответ не разобрать (несовпадение раскладки Kotlin ↔ Rust).
    MalformedSnapshot { reason: String },
}

/// Счётчики опроса с начала подписки.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct PowerSamplingStats {
    /// Показаний записано в кольцо.
    pub stored: u64,
    /// Показаний отдано потребителю через `drain_samples`.
    pub drained: u64,
    /// Непрочитанных показаний вытеснено переполнением.
    pub evicted_unread: u64,
    pub failed_not_attached: u64,
    pub failed_platform_call: u64,
    pub failed_malformed_snapshot: u64,
    /// Узлы сетки опроса, пропущенные из-за опоздания потока (заморозка, долгий binder).
    pub missed_ticks: u64,
}

impl PowerSamplingStats {
    /// Все неудачные чтения, независимо от причины.
    pub fn failed_reads(&self) -> u64 {
        self.failed_not_attached
            .saturating_add(self.failed_platform_call)
            .saturating_add(self.failed_malformed_snapshot)
    }
}

#[derive(Debug)]
struct RingState {
    pending: VecDeque<BatterySample>,
    capacity: NonZeroUsize,
    /// Последнее показание живёт отдельно от очереди: `drain` его не стирает.
    latest: Option<BatterySample>,
    last_failure: Option<PowerReadFailure>,
    stats: PowerSamplingStats,
}

/// Ограниченное кольцо непрочитанных показаний. Один писатель, любые читатели.
#[derive(Debug)]
pub struct PowerSampleRing {
    state: Mutex<RingState>,
}

impl PowerSampleRing {
    pub fn new(capacity: NonZeroUsize) -> Self {
        Self {
            state: Mutex::new(RingState {
                pending: VecDeque::with_capacity(capacity.get()),
                capacity,
                latest: None,
                last_failure: None,
                stats: PowerSamplingStats::default(),
            }),
        }
    }

    /// Записывает показание (вызывает поток опроса).
    pub fn push(&self, sample: BatterySample) -> PowerPushOutcome {
        let mut state = self.lock_state();
        let evicting = state.pending.len() == state.capacity.get();
        if evicting {
            state.pending.pop_front();
            state.stats.evicted_unread += 1;
        }
        state.pending.push_back(sample);
        state.latest = Some(sample);
        state.stats.stored += 1;
        if evicting {
            PowerPushOutcome::StoredEvictingOldest
        } else {
            PowerPushOutcome::Stored
        }
    }

    /// Учитывает неудачное чтение по его виду и запоминает его как последнее.
    pub fn record_failed_read(&self, failure: PowerReadFailure) {
        let mut state = self.lock_state();
        match failure {
            PowerReadFailure::NotAttached => state.stats.failed_not_attached += 1,
            PowerReadFailure::PlatformCallFailed { .. } => state.stats.failed_platform_call += 1,
            PowerReadFailure::MalformedSnapshot { .. } => state.stats.failed_malformed_snapshot += 1,
        }
        state.last_failure = Some(failure);
    }

    /// Учитывает пропущенные узлы сетки опроса (чтения не было вовсе — это не сбой чтения).
    pub fn record_missed_ticks(&self, count: u64) {
        let mut state = self.lock_state();
        state.stats.missed_ticks = state.stats.missed_ticks.saturating_add(count);
    }

    /// Переносит все непрочитанные показания в `into` (от старых к новым); возвращает их число.
    pub fn drain_into(&self, into: &mut Vec<BatterySample>) -> usize {
        let mut state = self.lock_state();
        let count = state.pending.len();
        into.extend(state.pending.drain(..));
        state.stats.drained += u64::try_from(count).unwrap_or(u64::MAX);
        count
    }

    pub fn latest(&self) -> Option<BatterySample> {
        self.lock_state().latest
    }

    pub fn last_failure(&self) -> Option<PowerReadFailure> {
        self.lock_state().last_failure.clone()
    }

    pub fn stats(&self) -> PowerSamplingStats {
        self.lock_state().stats
    }

    pub fn capacity(&self) -> NonZeroUsize {
        self.lock_state().capacity
    }

    fn lock_state(&self) -> MutexGuard<'_, RingState> {
        // Под замком только операции VecDeque и счётчики без паник: после чужой паники
        // состояние согласовано.
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Работающий опрос батареи. Опрос останавливается, когда подписка уничтожена.
pub struct PowerSampling {
    ring: Arc<PowerSampleRing>,
    _stop_on_drop: Box<dyn Send>,
}

impl PowerSampling {
    /// `stop_on_drop` — владелец потока опроса: его уничтожение останавливает чтение.
    pub fn new(ring: Arc<PowerSampleRing>, stop_on_drop: Box<dyn Send>) -> Self {
        Self {
            ring,
            _stop_on_drop: stop_on_drop,
        }
    }

    /// Самое свежее показание (остаётся доступным и после `drain_samples`).
    pub fn latest_sample(&self) -> Option<BatterySample> {
        self.ring.latest()
    }

    /// Забирает непрочитанные показания от старых к новым; возвращает их число.
    pub fn drain_samples(&self, into: &mut Vec<BatterySample>) -> usize {
        self.ring.drain_into(into)
    }

    pub fn stats(&self) -> PowerSamplingStats {
        self.ring.stats()
    }

    /// Последнее неудачное чтение (для объяснения дыр в отчёте).
    pub fn last_failure(&self) -> Option<PowerReadFailure> {
        self.ring.last_failure()
    }
}

impl std::fmt::Debug for PowerSampling {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PowerSampling")
            .field("ring", &self.ring)
            .finish_non_exhaustive()
    }
}

/// Почему опрос батареи не запустился.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PowerSourceError {
    #[error("источник питания недоступен: {reason}")]
    Unavailable { reason: String },
    #[error("Activity не подключена: некому читать BatteryManager")]
    NotAttached,
    #[error("опрос батареи уже запущен этим источником")]
    AlreadySampling,
    #[error("сбой платформы при запуске опроса батареи: {reason}")]
    PlatformCallFailed { reason: String },
}

/// Источник показаний батареи (на телефоне — `BatteryManager` через `android-shell`).
///
/// Контракт: `start_sampling` не ждёт платформу (не делает binder-вызовов на потоке
/// вызывающего), а возвращённая подписка читает только кольцо.
pub trait PowerSource: Send + Sync {
    fn start_sampling(&self, request: PowerSamplingRequest) -> Result<PowerSampling, PowerSourceError>;
}

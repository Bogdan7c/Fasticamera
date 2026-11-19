//! Датчики движения: гироскоп (задел V3 README) и акселерометр для ориентации.
//!
//! Гироскоп пишется в [`MotionRing`] — кольцо последних секунд в шкале `CLOCK_BOOTTIME`
//! (та же, что у `SENSOR_TIMESTAMP` камеры, V2). Будущая стабилизация и совмещение серии
//! кадров берут из него выборку за интервал экспозиции ([`MotionRing::samples_between`]).
//! Ориентация приходит событиями `ShellEvent::DeviceOrientation` только при смене.
//! Гравитация (наклон для блика) — отдельная подписка, см. [`crate::gravity`].

use std::collections::VecDeque;
use std::num::{NonZeroU32, NonZeroUsize};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use serde::Serialize;

use crate::clock::BoottimeNanos;
use crate::gravity::GravitySubscription;

/// Угловая скорость в радианах в секунду по осям устройства Android
/// (x — вправо по экрану, y — вверх по экрану, z — из экрана к пользователю).
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct AngularVelocity {
    pub x_rad_per_s: f32,
    pub y_rad_per_s: f32,
    pub z_rad_per_s: f32,
}

/// Одно показание гироскопа.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct GyroSample {
    pub time: BoottimeNanos,
    pub velocity: AngularVelocity,
}

/// Частота опроса датчика в герцах (не ноль).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct SensorRate(NonZeroU32);

impl SensorRate {
    pub const fn new(hertz: NonZeroU32) -> Self {
        Self(hertz)
    }

    pub const fn hertz(self) -> u32 {
        self.0.get()
    }

    /// Период опроса (1 / частота).
    pub fn sampling_period(self) -> Duration {
        Duration::from_secs(1) / self.0.get()
    }
}

/// Что делать с кольцом гироскопа: частота и сколько секунд истории держать (V9: память —
/// параметр, а не жёсткое число).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GyroRequest {
    pub rate: SensorRate,
    pub retained: Duration,
}

impl GyroRequest {
    /// Ёмкость кольца: частота × окно истории, с запасом 10 % на дрожание частоты датчика
    /// (BMI26x отдаёт 402 Гц при запросе 400, сессия 00) — не меньше одного показания.
    pub fn ring_capacity(self) -> NonZeroUsize {
        // Целочисленно: частота × окно в нс × 11/10, деление с округлением вверх. В f64 выражение
        // 400 × 1 с × 1,1 даёт 440.00000000000006 и ceil превращал бы его в 441.
        const NANOS_PER_SECOND: u128 = 1_000_000_000;
        const MARGIN_NUMERATOR: u128 = 11;
        const MARGIN_DENOMINATOR: u128 = 10;
        let scaled = u128::from(self.rate.hertz()) * self.retained.as_nanos() * MARGIN_NUMERATOR;
        let capacity = scaled.div_ceil(NANOS_PER_SECOND * MARGIN_DENOMINATOR);
        let capacity = usize::try_from(capacity).unwrap_or(usize::MAX);
        NonZeroUsize::new(capacity).unwrap_or(NonZeroUsize::MIN)
    }
}

/// Что стало с показанием при записи в кольцо.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RingPushOutcome {
    Stored,
    /// Записано, самое старое вытеснено (кольцо полно — штатный режим).
    StoredEvictingOldest,
    /// Отвергнуто: метка не позже последней. Кольцо держит строгий порядок времени,
    /// иначе выборка по интервалу была бы неверной.
    RejectedOutOfOrder,
}

/// Счётчики кольца с начала его жизни.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct MotionRingStats {
    pub stored: u64,
    pub evicted: u64,
    pub rejected_out_of_order: u64,
}

#[derive(Debug)]
struct RingState {
    samples: VecDeque<GyroSample>,
    capacity: NonZeroUsize,
    stats: MotionRingStats,
}

/// Кольцо последних показаний гироскопа. Один писатель (поток датчика), много читателей.
/// Замок держится только на время копирования, поэтому 400 записей в секунду дёшевы.
#[derive(Debug)]
pub struct MotionRing {
    state: Mutex<RingState>,
}

impl MotionRing {
    pub fn new(capacity: NonZeroUsize) -> Self {
        Self {
            state: Mutex::new(RingState {
                samples: VecDeque::with_capacity(capacity.get()),
                capacity,
                stats: MotionRingStats::default(),
            }),
        }
    }

    /// Записывает показание (вызывает поток датчика).
    pub fn push(&self, sample: GyroSample) -> RingPushOutcome {
        let mut state = self.lock_state();
        if state.samples.back().is_some_and(|last| sample.time <= last.time) {
            state.stats.rejected_out_of_order += 1;
            return RingPushOutcome::RejectedOutOfOrder;
        }
        let evicting = state.samples.len() == state.capacity.get();
        if evicting {
            state.samples.pop_front();
            state.stats.evicted += 1;
        }
        state.samples.push_back(sample);
        state.stats.stored += 1;
        if evicting {
            RingPushOutcome::StoredEvictingOldest
        } else {
            RingPushOutcome::Stored
        }
    }

    /// Показания с меткой в `[from, to]` (включительно), по возрастанию времени.
    pub fn samples_between(&self, from: BoottimeNanos, to: BoottimeNanos) -> Vec<GyroSample> {
        let state = self.lock_state();
        // Кольцо отсортировано по времени (push отвергает непорядок), поэтому границы — бинарный поиск.
        let start = state.samples.partition_point(|sample| sample.time < from);
        let end = state.samples.partition_point(|sample| sample.time <= to);
        if start >= end {
            return Vec::new();
        }
        state.samples.range(start..end).copied().collect()
    }

    /// Самое свежее показание.
    pub fn latest(&self) -> Option<GyroSample> {
        self.lock_state().samples.back().copied()
    }

    /// Сколько показаний сейчас в кольце.
    pub fn len(&self) -> usize {
        self.lock_state().samples.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn capacity(&self) -> NonZeroUsize {
        self.lock_state().capacity
    }

    pub fn stats(&self) -> MotionRingStats {
        self.lock_state().stats
    }

    fn lock_state(&self) -> MutexGuard<'_, RingState> {
        // Под замком нет кода, способного запаниковать посреди изменения (только операции
        // VecDeque и счётчики), поэтому после чужой паники состояние согласовано.
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Почему источник датчиков не запустился.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SensorSourceError {
    #[error("на телефоне нет датчика {sensor}")]
    SensorAbsent { sensor: &'static str },
    #[error("датчик {sensor} уже запущен этим источником")]
    AlreadyRunning { sensor: &'static str },
    #[error("сбой платформы при запуске датчика {sensor}: {reason}")]
    PlatformFailure { sensor: &'static str, reason: String },
}

/// Работающая подписка на гироскоп. Датчик выключается, когда подписка уничтожена.
pub struct GyroSubscription {
    ring: Arc<MotionRing>,
    _stop_on_drop: Box<dyn Send>,
}

impl GyroSubscription {
    /// `stop_on_drop` — объект источника, чей `Drop` останавливает датчик.
    pub fn new(ring: Arc<MotionRing>, stop_on_drop: Box<dyn Send>) -> Self {
        Self {
            ring,
            _stop_on_drop: stop_on_drop,
        }
    }

    pub fn ring(&self) -> &Arc<MotionRing> {
        &self.ring
    }
}

/// Работающая подписка на ориентацию. События идут в очередь оболочки; здесь — счётчик
/// принятых показаний акселерометра (доказательство, что датчик жив).
pub struct OrientationSubscription {
    accelerometer_samples: Arc<AtomicU64>,
    _stop_on_drop: Box<dyn Send>,
}

impl OrientationSubscription {
    pub fn new(accelerometer_samples: Arc<AtomicU64>, stop_on_drop: Box<dyn Send>) -> Self {
        Self {
            accelerometer_samples,
            _stop_on_drop: stop_on_drop,
        }
    }

    pub fn accelerometer_samples(&self) -> u64 {
        self.accelerometer_samples.load(Ordering::Relaxed)
    }
}

/// Источник датчиков движения. Датчики работают только пока жива подписка —
/// включаются по намерению, а не всегда (батарея).
pub trait MotionSource: Send + Sync {
    fn start_gyro(&self, request: GyroRequest) -> Result<GyroSubscription, SensorSourceError>;

    fn start_orientation(&self) -> Result<OrientationSubscription, SensorSourceError>;

    /// Подписка на вектор гравитации (наклон телефона) для блика стекла. Показания идут в
    /// [`crate::gravity::GravitySlot`] подписки; датчик выключается вместе с ней.
    fn start_gravity(&self) -> Result<GravitySubscription, SensorSourceError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(time_ns: i64) -> GyroSample {
        GyroSample {
            time: BoottimeNanos::from_nanos(time_ns),
            velocity: AngularVelocity {
                x_rad_per_s: 0.0,
                y_rad_per_s: 0.0,
                z_rad_per_s: time_ns as f32,
            },
        }
    }

    fn ring_of(capacity: usize) -> MotionRing {
        MotionRing::new(NonZeroUsize::new(capacity).expect("ёмкость"))
    }

    /// Синтетический поток 400 Гц: 2500 мкс между показаниями.
    const PERIOD_NS: i64 = 2_500_000;

    #[test]
    fn interval_query_returns_exactly_samples_inside_bounds() {
        let ring = ring_of(1000);
        for index in 0..400 {
            ring.push(sample(index * PERIOD_NS));
        }
        // Интервал «экспозиции» 10..20 мс: показания 4..=8 (10, 12.5, 15, 17.5, 20 мс).
        let window = ring.samples_between(
            BoottimeNanos::from_nanos(10_000_000),
            BoottimeNanos::from_nanos(20_000_000),
        );
        let times: Vec<i64> = window.iter().map(|sample| sample.time.as_nanos()).collect();
        assert_eq!(times, vec![10_000_000, 12_500_000, 15_000_000, 17_500_000, 20_000_000]);
    }

    #[test]
    fn interval_outside_history_is_empty() {
        let ring = ring_of(10);
        ring.push(sample(1_000));
        assert!(
            ring.samples_between(BoottimeNanos::from_nanos(5_000), BoottimeNanos::from_nanos(9_000))
                .is_empty()
        );
        assert!(
            ring.samples_between(BoottimeNanos::from_nanos(9_000), BoottimeNanos::from_nanos(5_000))
                .is_empty()
        );
    }

    #[test]
    fn full_ring_evicts_oldest_and_counts() {
        let ring = ring_of(3);
        for index in 1..=5 {
            ring.push(sample(index));
        }
        assert_eq!(ring.len(), 3);
        let all = ring.samples_between(BoottimeNanos::from_nanos(0), BoottimeNanos::from_nanos(10));
        assert_eq!(
            all.iter().map(|sample| sample.time.as_nanos()).collect::<Vec<_>>(),
            vec![3, 4, 5]
        );
        assert_eq!(
            ring.stats(),
            MotionRingStats {
                stored: 5,
                evicted: 2,
                rejected_out_of_order: 0
            }
        );
        assert_eq!(ring.push(sample(6)), RingPushOutcome::StoredEvictingOldest);
    }

    #[test]
    fn out_of_order_sample_is_rejected_and_ring_stays_sorted() {
        let ring = ring_of(10);
        assert_eq!(ring.push(sample(100)), RingPushOutcome::Stored);
        assert_eq!(ring.push(sample(100)), RingPushOutcome::RejectedOutOfOrder);
        assert_eq!(ring.push(sample(50)), RingPushOutcome::RejectedOutOfOrder);
        assert_eq!(ring.len(), 1);
        assert_eq!(ring.stats().rejected_out_of_order, 2);
        assert_eq!(ring.latest().map(|sample| sample.time.as_nanos()), Some(100));
    }

    #[test]
    fn capacity_covers_retained_window_with_jitter_margin() {
        let request = GyroRequest {
            rate: SensorRate::new(NonZeroU32::new(400).expect("частота")),
            retained: Duration::from_secs(5),
        };
        // 400 Гц × 5 с = 2000, +10 % на 402 Гц датчика.
        assert_eq!(request.ring_capacity().get(), 2200);
        let zero_window = GyroRequest {
            retained: Duration::ZERO,
            ..request
        };
        assert_eq!(zero_window.ring_capacity().get(), 1);
        let one_second = GyroRequest {
            retained: Duration::from_secs(1),
            ..request
        };
        // Регрессия: в f64 получалось 441 (ошибка округления 400 × 1,1).
        assert_eq!(one_second.ring_capacity().get(), 440);
    }

    #[test]
    fn sampling_period_matches_rate() {
        let rate = SensorRate::new(NonZeroU32::new(400).expect("частота"));
        assert_eq!(rate.sampling_period(), Duration::from_micros(2_500));
    }
}

//! Шкалы времени шва.
//!
//! Android отдаёт время в двух разных часах, и смешивать их молча нельзя (задел V2 README):
//! * `CLOCK_MONOTONIC` (`System.nanoTime`, `SystemClock.uptimeMillis`) — время касаний
//!   `MotionEvent`; эти часы стоят, пока телефон спит;
//! * `CLOCK_BOOTTIME` (`SystemClock.elapsedRealtimeNanos`) — время датчиков и
//!   `SENSOR_TIMESTAMP` камеры; эти часы идут и во сне.
//!
//! Поэтому это два разных типа без взаимных преобразований: сравнить касание с кадром
//! камеры можно будет только явно, через измеренное смещение часов (план `video`).

use std::time::Duration;

use serde::Serialize;

/// Момент времени по `CLOCK_MONOTONIC` в наносекундах.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct MonotonicNanos(i64);

impl MonotonicNanos {
    pub const fn from_nanos(nanos: i64) -> Self {
        Self(nanos)
    }

    pub const fn as_nanos(self) -> i64 {
        self.0
    }
}

/// Момент времени по `CLOCK_BOOTTIME` в наносекундах (шкала датчиков и камеры).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct BoottimeNanos(i64);

impl BoottimeNanos {
    pub const fn from_nanos(nanos: i64) -> Self {
        Self(nanos)
    }

    pub const fn as_nanos(self) -> i64 {
        self.0
    }

    /// Сколько прошло от `earlier` до `self`; ноль, если `earlier` позже.
    pub fn saturating_duration_since(self, earlier: BoottimeNanos) -> Duration {
        let difference = self.0.saturating_sub(earlier.0);
        // Отрицательная разница (часы «назад») — это ноль, а не паника: метки приходят
        // с разных потоков, и вызывающий не обязан заранее упорядочивать их.
        Duration::from_nanos(u64::try_from(difference).unwrap_or(0))
    }

    /// Момент на `duration` раньше; насыщается на минимуме шкалы.
    pub fn saturating_sub(self, duration: Duration) -> BoottimeNanos {
        let nanos = i64::try_from(duration.as_nanos()).unwrap_or(i64::MAX);
        BoottimeNanos(self.0.saturating_sub(nanos))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duration_since_is_positive_difference() {
        let earlier = BoottimeNanos::from_nanos(1_000);
        let later = BoottimeNanos::from_nanos(3_500);
        assert_eq!(later.saturating_duration_since(earlier), Duration::from_nanos(2_500));
    }

    #[test]
    fn duration_since_later_point_is_zero_not_panic() {
        let earlier = BoottimeNanos::from_nanos(1_000);
        let later = BoottimeNanos::from_nanos(3_500);
        assert_eq!(earlier.saturating_duration_since(later), Duration::ZERO);
    }

    #[test]
    fn saturating_sub_moves_back_and_clamps() {
        let point = BoottimeNanos::from_nanos(10_000);
        assert_eq!(point.saturating_sub(Duration::from_nanos(4_000)).as_nanos(), 6_000);
        assert_eq!(
            BoottimeNanos::from_nanos(i64::MIN + 1).saturating_sub(Duration::from_secs(1)),
            BoottimeNanos::from_nanos(i64::MIN)
        );
    }

    #[test]
    fn serializes_as_plain_number() {
        let json = serde_json::to_value(MonotonicNanos::from_nanos(42)).expect("json");
        assert_eq!(json, serde_json::json!(42));
    }
}

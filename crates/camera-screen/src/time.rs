//! Арифметика меток времени `MonotonicNanos` (шкала касаний и vsync): сложение с миллисекундами
//! и сравнение. Редьюсер часов не читает — все моменты приходят в действиях.

use shell_contract::clock::MonotonicNanos;

const NANOS_PER_MILLISECOND: i64 = 1_000_000;
const NANOS_PER_SECOND: i64 = 1_000_000_000;

/// Момент `milliseconds` после `start` (насыщается, а не переполняется).
pub(crate) fn deadline_after(start: MonotonicNanos, milliseconds: u32) -> MonotonicNanos {
    let delta = i64::from(milliseconds).saturating_mul(NANOS_PER_MILLISECOND);
    MonotonicNanos::from_nanos(start.as_nanos().saturating_add(delta))
}

/// Момент `seconds` после `start`.
pub(crate) fn deadline_after_seconds(start: MonotonicNanos, seconds: u32) -> MonotonicNanos {
    let delta = i64::from(seconds).saturating_mul(NANOS_PER_SECOND);
    MonotonicNanos::from_nanos(start.as_nanos().saturating_add(delta))
}

/// Наступил ли срок.
pub(crate) fn has_reached(now: MonotonicNanos, deadline: MonotonicNanos) -> bool {
    now >= deadline
}

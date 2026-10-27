//! Safety-модуль: чтение `CLOCK_MONOTONIC` и ожидание по нему.
//!
//! Сроки линий кадра Choreographer и `desiredPresentTime` — в `CLOCK_MONOTONIC`
//! ([`MonotonicNanos`]). `std::time::Instant` на Linux/Android идёт по тем же часам, но
//! не отдаёт абсолютное значение, поэтому сравнивать его со сроками платформы нельзя.
//! Единственный `unsafe` — вызов `clock_gettime` ниже.
//!
//! Код, которому нужно «сейчас» или сон, зависит от трейта [`MonotonicTimer`]: тесты
//! подставляют часы, время в которых двигает сам тест.

use std::time::Duration;

use shell_contract::clock::MonotonicNanos;

/// Часы не прочитались. С верными аргументами `clock_gettime(CLOCK_MONOTONIC)` не отказывает
/// (ошибки только EINVAL/EFAULT), но итог не глотается: вызывающий откатывается на безопасное
/// поведение и считает сбой.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("clock_gettime(CLOCK_MONOTONIC) отказал, errno {errno}")]
pub struct ClockReadError {
    pub errno: i32,
}

/// Источник монотонного времени и ожидания (вызывается из одного потока).
pub trait MonotonicTimer {
    /// Текущее время `CLOCK_MONOTONIC`.
    fn now(&self) -> Result<MonotonicNanos, ClockReadError>;
    /// Сон ОС не меньше `duration` (может проснуться позже — дрожание планировщика).
    fn sleep(&self, duration: Duration);
    /// Одна итерация активного ожидания (подсказка процессору, без сна).
    fn spin_once(&self);
}

/// Настоящие часы системы.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemMonotonicTimer;

impl MonotonicTimer for SystemMonotonicTimer {
    fn now(&self) -> Result<MonotonicNanos, ClockReadError> {
        let mut reading = libc::timespec { tv_sec: 0, tv_nsec: 0 };
        // SAFETY: `reading` — живая локальная структура нужного типа, указатель на неё действителен
        // на время вызова; CLOCK_MONOTONIC — поддерживаемый идентификатор часов.
        let status = unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut reading) };
        if status != 0 {
            return Err(ClockReadError {
                errno: std::io::Error::last_os_error().raw_os_error().unwrap_or(0),
            });
        }
        // Монотонные часы считают от загрузки: секунды × 1e9 далеко от предела i64 (292 года).
        let nanos = widen_to_i64(reading.tv_sec)
            .saturating_mul(1_000_000_000)
            .saturating_add(widen_to_i64(reading.tv_nsec));
        Ok(MonotonicNanos::from_nanos(nanos))
    }

    fn sleep(&self, duration: Duration) {
        std::thread::sleep(duration);
    }

    fn spin_once(&self) {
        std::hint::spin_loop();
    }
}

/// Расширение поля `timespec` до i64. Ширина `time_t`/`c_long` зависит от платформы (32 бита на
/// armv7, 64 на aarch64); обобщённая функция не даёт clippy считать преобразование лишним.
fn widen_to_i64<T: Into<i64>>(value: T) -> i64 {
    value.into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_clock_is_monotonic_and_advances_by_the_slept_time() {
        let timer = SystemMonotonicTimer;
        let before = timer.now().expect("часы читаются");
        timer.sleep(Duration::from_millis(5));
        let after = timer.now().expect("часы читаются");
        let elapsed = after.as_nanos() - before.as_nanos();
        assert!(elapsed >= 5_000_000, "прошло {elapsed} нс, а спали 5 мс");
        assert!(before.as_nanos() > 0, "часы считают от загрузки, не с нуля");
    }
}

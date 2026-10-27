//! Числа и коды ADPF (`android/performance_hint.h`) ↔ нейтральные типы (тестируется на ПК).
//!
//! Функции `APerformanceHint_*` и `AWorkDuration_*` принимают длительности в `int64_t` нс и
//! возвращают 0 при успехе или `errno` (`EINVAL` — неверное число, `EPIPE` — сбой связи с системным
//! сервисом, `EBUSY` — слишком частые вызовы). Здесь — перевод без `unsafe`: сами вызовы — в
//! `android::performance_hint_ffi`.

use std::time::Duration;

use shell_contract::performance_hint::{FrameWork, PerformanceHintError};

/// Длительность в нс для NDK. Больше `i64::MAX` нс (≈ 292 года) не бывает; насыщаемся, а не
/// оборачиваемся в отрицательное число (его платформа отвергла бы как `EINVAL`).
pub fn duration_to_platform_nanos(duration: Duration) -> i64 {
    i64::try_from(duration.as_nanos()).unwrap_or(i64::MAX)
}

/// Отчёт о кадре в числах NDK.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlatformWorkDuration {
    pub period_start_nanos: i64,
    pub total_nanos: i64,
    pub cpu_nanos: i64,
    /// 0 — GPU не измерен (так велит документация `AWorkDuration_setActualGpuDurationNanos`).
    pub gpu_nanos: i64,
}

impl PlatformWorkDuration {
    pub fn of(work: &FrameWork) -> Self {
        Self {
            period_start_nanos: work.period_start().as_nanos(),
            total_nanos: duration_to_platform_nanos(work.total()),
            cpu_nanos: duration_to_platform_nanos(work.cpu()),
            gpu_nanos: work.gpu().map_or(0, duration_to_platform_nanos),
        }
    }
}

/// Код возврата NDK → результат (0 — успех, иначе `Failed` с кодом и именем вызова).
pub fn platform_result(operation: &'static str, code: i32) -> Result<(), PerformanceHintError> {
    if code == 0 {
        Ok(())
    } else {
        Err(PerformanceHintError::Failed { operation, code })
    }
}

#[cfg(test)]
mod tests {
    use shell_contract::clock::MonotonicNanos;

    use super::*;

    #[test]
    fn work_duration_is_converted_to_nanoseconds_with_unmeasured_gpu_as_zero() {
        let work = FrameWork::new(
            MonotonicNanos::from_nanos(123_456),
            Duration::from_micros(11_500),
            Duration::from_micros(2_500),
            Some(Duration::from_millis(9)),
        )
        .expect("работа");
        assert_eq!(
            PlatformWorkDuration::of(&work),
            PlatformWorkDuration {
                period_start_nanos: 123_456,
                total_nanos: 11_500_000,
                cpu_nanos: 2_500_000,
                gpu_nanos: 9_000_000,
            }
        );
        let cpu_only = FrameWork::new(
            MonotonicNanos::from_nanos(1),
            Duration::from_millis(2),
            Duration::from_millis(2),
            None,
        )
        .expect("работа");
        assert_eq!(PlatformWorkDuration::of(&cpu_only).gpu_nanos, 0);
    }

    #[test]
    fn huge_duration_saturates_instead_of_wrapping_negative() {
        assert_eq!(duration_to_platform_nanos(Duration::MAX), i64::MAX);
    }

    #[test]
    fn nonzero_return_code_is_a_failure_with_operation_and_code() {
        assert_eq!(platform_result("report", 0), Ok(()));
        assert_eq!(
            platform_result("report", 32),
            Err(PerformanceHintError::Failed {
                operation: "report",
                code: 32
            })
        );
    }
}

//! Метки времени GPU: пул запросов и чистая арифметика их разбора.

use ash::vk;

use crate::context::GpuContext;
use crate::error::{GpuError, run_step};
use crate::gpu_timing::math::{elapsed_ticks, ticks_to_nanoseconds};
use crate::live_objects::OwnedQueryPool;

/// Четыре метки диагностики: начало и конец работы A, начало и конец работы B.
pub(super) const TIMESTAMP_COUNT: u32 = 4;

/// Номера запросов в пуле.
pub(super) mod slot {
    pub(in crate::sync::diagnostics) const WORK_A_BEGIN: u32 = 0;
    pub(in crate::sync::diagnostics) const WORK_A_END: u32 = 1;
    pub(in crate::sync::diagnostics) const WORK_B_BEGIN: u32 = 2;
    pub(in crate::sync::diagnostics) const WORK_B_END: u32 = 3;
}

/// RAII-пул запросов меток времени (учтён в `live_objects()` как `QueryPool`).
pub(super) struct TimestampPool {
    pool: OwnedQueryPool,
    context: GpuContext,
}

impl TimestampPool {
    pub(super) fn new(context: &GpuContext) -> Result<Self, GpuError> {
        let create_info = vk::QueryPoolCreateInfo::default()
            .query_type(vk::QueryType::TIMESTAMP)
            .query_count(TIMESTAMP_COUNT);
        // SAFETY: устройство валидно; create_info — локальная структура.
        let pool = run_step(
            unsafe { context.device().create_query_pool(&create_info, None) },
            "vkCreateQueryPool",
        )?;
        Ok(Self {
            pool: OwnedQueryPool::from_raw(context, pool),
            context: context.clone(),
        })
    }

    pub(super) fn handle(&self) -> vk::QueryPool {
        self.pool.handle()
    }

    /// Читает все метки (в тиках). Вызывать после завершения обеих работ (fence).
    pub(super) fn read_ticks(&self) -> Result<[u64; TIMESTAMP_COUNT as usize], GpuError> {
        let mut ticks = [0_u64; TIMESTAMP_COUNT as usize];
        // SAFETY: пул создан этим устройством; все 4 запроса записаны работами, завершёнными fence'ом.
        run_step(
            unsafe {
                self.context.device().get_query_pool_results(
                    self.pool.handle(),
                    0,
                    &mut ticks,
                    vk::QueryResultFlags::TYPE_64,
                )
            },
            "vkGetQueryPoolResults",
        )?;
        Ok(ticks)
    }
}

/// Метки в наносекундах относительно начала работы A плюс вывод о порядке.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimestampAnalysis {
    /// Начало A (всегда 0: точка отсчёта).
    pub work_a_begin_ns: u64,
    /// Конец A.
    pub work_a_end_ns: u64,
    /// Начало B.
    pub work_b_begin_ns: u64,
    /// Конец B.
    pub work_b_end_ns: u64,
    /// `true`, если B началась не раньше конца A.
    pub b_started_after_a_finished: bool,
}

/// Разбор меток: тики → наносекунды от начала A с учётом переполнения счётчика `valid_bits`.
///
/// Счётчик меток может быть короче 64 бит и переходить через ноль, поэтому разности берутся
/// по модулю `2^valid_bits`: `(tick - a_begin) & mask`. Предполагается, что вся диагностика
/// короче половины периода счётчика (на практике — секунды при периоде в минуты).
pub(super) fn analyze_timestamps(ticks: [u64; 4], valid_bits: u32, period_ns: f32) -> TimestampAnalysis {
    // Арифметика общая с `gpu_timing::GpuTimestamps` (маска valid_bits, разность по модулю, период).
    let relative_ticks =
        |index: u32| elapsed_ticks(ticks[slot::WORK_A_BEGIN as usize], ticks[index as usize], valid_bits);
    let to_ns = |relative: u64| ticks_to_nanoseconds(relative, period_ns);
    let a_end = relative_ticks(slot::WORK_A_END);
    let b_begin = relative_ticks(slot::WORK_B_BEGIN);
    let b_end = relative_ticks(slot::WORK_B_END);
    TimestampAnalysis {
        work_a_begin_ns: 0,
        work_a_end_ns: to_ns(a_end),
        work_b_begin_ns: to_ns(b_begin),
        work_b_end_ns: to_ns(b_end),
        b_started_after_a_finished: b_begin >= a_end,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordered_ticks_convert_to_nanoseconds_relative_to_a_begin() {
        // Период 2 нс/тик: A = [100, 150], B = [160, 200].
        let analysis = analyze_timestamps([100, 150, 160, 200], 64, 2.0);
        assert_eq!(analysis.work_a_begin_ns, 0);
        assert_eq!(analysis.work_a_end_ns, 100);
        assert_eq!(analysis.work_b_begin_ns, 120);
        assert_eq!(analysis.work_b_end_ns, 200);
        assert!(analysis.b_started_after_a_finished);
    }

    #[test]
    fn b_starting_before_a_end_is_reported_as_overlap() {
        let analysis = analyze_timestamps([100, 150, 140, 200], 64, 1.0);
        assert!(!analysis.b_started_after_a_finished);
    }

    #[test]
    fn b_starting_exactly_when_a_ends_counts_as_after() {
        let analysis = analyze_timestamps([100, 150, 150, 200], 64, 1.0);
        assert!(analysis.b_started_after_a_finished);
    }

    #[test]
    fn counter_wraparound_with_short_valid_bits_is_handled() {
        // 8 значимых бит: A начинается на 250, заканчивается после перехода через 0 (на 10),
        // B начинается на 12 — это позже конца A, хотя сырое число меньше.
        let analysis = analyze_timestamps([250, 10, 12, 20], 8, 1.0);
        assert_eq!(analysis.work_a_end_ns, 16);
        assert_eq!(analysis.work_b_begin_ns, 18);
        assert!(analysis.b_started_after_a_finished);
        // А если B началась на 5 (до конца A на 10), это пересечение.
        assert!(!analyze_timestamps([250, 10, 5, 20], 8, 1.0).b_started_after_a_finished);
    }
}

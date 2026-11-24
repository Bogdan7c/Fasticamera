//! Кэш вариантов пайплайна ISP в отчётах сценариев (`viewfinder`, `zoom`, пакет I5 сессии 16a).
//!
//! Зачем: компиляция пайплайна посреди кадра — рывок видоискателя, и без счётчиков его не отличить от других причин.
//! Счётчики накопительные у ISP, поэтому отчёт окна — приращение между началом и концом замера: `created_in_frame`
//! в окне после прогрева должно быть 0. Времена — наибольшие и последние за жизнь ISP (интервал приращением не
//! считается), в микросекундах: компиляция одного варианта — десятки миллисекунд, и округлять до сотых мс незачем.

use std::time::Duration;

use isp_vulkan::viewfinder::VariantCacheStats;
use serde::Serialize;

/// Что кэш вариантов делал за окно замера.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct PipelineVariantsReport {
    /// Вариантов в кэше на конец окна.
    pub in_cache: usize,
    /// Создано ВНУТРИ кадров окна (рывок): после прогрева должно быть 0.
    pub created_in_frame: u64,
    /// Создано заготовкой родственных вариантов за окно.
    pub created_by_prewarm: u64,
    /// Заготовок, которые не удались.
    pub prewarm_failed: u64,
    /// Отказов создания варианта, нужного кадру.
    pub create_failed_in_frame: u64,
    /// Вытеснено из кэша за окно (повторная компиляция при следующем переключении).
    pub evicted: u64,
    /// CPU-время последнего создания варианта в кадре, мкс (за жизнь ISP).
    pub last_frame_compile_us: u64,
    /// Наибольшее CPU-время создания одного варианта в кадре, мкс (за жизнь ISP).
    pub longest_frame_compile_us: u64,
    /// CPU-время последней заготовки целиком, мкс (за жизнь ISP).
    pub last_prewarm_us: u64,
    /// Наибольшее CPU-время одной заготовки, мкс (за жизнь ISP).
    pub longest_prewarm_us: u64,
}

fn micros(duration: Duration) -> u64 {
    u64::try_from(duration.as_micros()).unwrap_or(u64::MAX)
}

impl PipelineVariantsReport {
    /// Приращение счётчиков между началом окна (`before`) и концом (`after`); времена и число вариантов — на конец.
    pub fn between(before: &VariantCacheStats, after: &VariantCacheStats) -> Self {
        Self {
            in_cache: after.in_cache,
            created_in_frame: after.created_in_frame.saturating_sub(before.created_in_frame),
            created_by_prewarm: after.created_by_prewarm.saturating_sub(before.created_by_prewarm),
            prewarm_failed: after.prewarm_failed.saturating_sub(before.prewarm_failed),
            create_failed_in_frame: after
                .create_failed_in_frame
                .saturating_sub(before.create_failed_in_frame),
            evicted: after.evicted.saturating_sub(before.evicted),
            last_frame_compile_us: micros(after.last_frame_compile),
            longest_frame_compile_us: micros(after.longest_frame_compile),
            last_prewarm_us: micros(after.last_prewarm),
            longest_prewarm_us: micros(after.longest_prewarm),
        }
    }

    /// Заметка для отчёта, если в окне пайплайн создавался в кадре или кэш вытеснял варианты; `None` — всё спокойно.
    /// Это заметка, не нарушение: вердикт сценария она не меняет.
    pub fn note(&self) -> Option<String> {
        if self.created_in_frame == 0 && self.evicted == 0 && self.create_failed_in_frame == 0 {
            return None;
        }
        Some(format!(
            "кэш вариантов ISP: создано в кадре {} (после прогрева должно быть 0), вытеснено {}, отказов {}",
            self.created_in_frame, self.evicted, self.create_failed_in_frame
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stats(created_in_frame: u64, evicted: u64) -> VariantCacheStats {
        VariantCacheStats {
            in_cache: 12,
            created_in_frame,
            created_by_prewarm: 11,
            evicted,
            longest_frame_compile: Duration::from_millis(35),
            longest_prewarm: Duration::from_millis(410),
            ..VariantCacheStats::default()
        }
    }

    #[test]
    fn window_report_is_the_increment_of_counters_and_the_end_state_of_times() {
        let report = PipelineVariantsReport::between(&stats(1, 0), &stats(4, 2));
        assert_eq!(
            (report.created_in_frame, report.evicted, report.created_by_prewarm),
            (3, 2, 0)
        );
        assert_eq!(report.in_cache, 12);
        assert_eq!(
            (report.longest_frame_compile_us, report.longest_prewarm_us),
            (35_000, 410_000)
        );
    }

    #[test]
    fn a_calm_window_has_no_note_and_a_window_with_creations_names_them() {
        assert_eq!(PipelineVariantsReport::between(&stats(1, 0), &stats(1, 0)).note(), None);
        let note = PipelineVariantsReport::between(&stats(1, 0), &stats(3, 0))
            .note()
            .expect("создания в кадре обязаны попасть в заметку");
        assert!(note.contains("создано в кадре 2"), "{note}");
        let eviction_only = PipelineVariantsReport::between(&stats(1, 0), &stats(1, 1));
        assert!(eviction_only.note().is_some(), "вытеснение — тоже заметка");
    }

    #[test]
    fn counters_that_went_backwards_do_not_underflow() {
        let report = PipelineVariantsReport::between(&stats(5, 3), &stats(1, 0));
        assert_eq!((report.created_in_frame, report.evicted), (0, 0));
    }
}

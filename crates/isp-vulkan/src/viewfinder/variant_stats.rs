//! Счётчики кэша вариантов пайплайна видоискателя ([`VariantCacheStats`]).
//!
//! Зачем: компиляция пайплайна на потоке рендера посреди кадра — это рывок видоискателя. Без счётчиков рывок
//! от компиляции не отличить от других причин (медленный GPU, ожидание fence камеры). Поэтому кэш считает,
//! сколько вариантов создано ВНУТРИ кадра (по требованию), сколько заранее (заготовкой), сколько вытеснено и сколько
//! раз создание не удалось, и сколько CPU-времени ушло на создание. Счётчики накопительные за жизнь ISP.

use std::time::Duration;

/// Счётчики и времена кэша вариантов (накопительные, кроме `in_cache`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct VariantCacheStats {
    /// Сколько вариантов лежит в кэше сейчас (мгновенное значение).
    pub in_cache: usize,
    /// Создано ВНУТРИ кадра, вне заготовки: кадру понадобился вариант, которого не было. После прогрева в штатной
    /// работе (зум, смена основы, чередование статистики) не растёт: рост = компиляция в кадре.
    pub created_in_frame: u64,
    /// Создано заготовкой родственных вариантов (прогрев: другие основы, раскладки AWB, кадр со статистикой и без).
    pub created_by_prewarm: u64,
    /// Заготовка не удалась (кадр от этого не страдает, причина — в журнале: вариант создастся по требованию).
    pub prewarm_failed: u64,
    /// Создание варианта, нужного кадру, не удалось (кадр отвергнут ошибкой `record`).
    pub create_failed_in_frame: u64,
    /// Вытеснено из кэша: нужный позже вариант придётся создавать заново (признак тесного кэша).
    pub evicted: u64,
    /// CPU-время создания варианта в последний раз, когда кадру понадобился новый вариант (без заготовки).
    pub last_frame_compile: Duration,
    /// Наибольшее CPU-время создания одного варианта в кадре.
    pub longest_frame_compile: Duration,
    /// CPU-время последней заготовки родственных вариантов целиком (кадр, создавший ключ, ждёт и её).
    pub last_prewarm: Duration,
    /// Наибольшее CPU-время одной заготовки.
    pub longest_prewarm: Duration,
}

impl VariantCacheStats {
    /// Учитывает создание варианта внутри кадра: удачное или нет, и сколько оно длилось.
    pub(crate) fn note_frame_creation(&mut self, outcome: CreationOutcome, elapsed: Duration) {
        match outcome {
            CreationOutcome::Created => self.created_in_frame += 1,
            CreationOutcome::Failed => self.create_failed_in_frame += 1,
        }
        self.last_frame_compile = elapsed;
        self.longest_frame_compile = self.longest_frame_compile.max(elapsed);
    }

    /// Учитывает одну заготовку целиком: сколько вариантов создано, сколько не вышло, сколько она длилась.
    pub(crate) fn note_prewarm(&mut self, created: u64, failed: u64, elapsed: Duration) {
        self.created_by_prewarm += created;
        self.prewarm_failed += failed;
        self.last_prewarm = elapsed;
        self.longest_prewarm = self.longest_prewarm.max(elapsed);
    }
}

/// Чем кончилось создание варианта в кадре.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CreationOutcome {
    /// Вариант создан.
    Created,
    /// Создать не удалось.
    Failed,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_creations_are_counted_separately_from_failures_and_track_the_longest_time() {
        let mut stats = VariantCacheStats::default();
        stats.note_frame_creation(CreationOutcome::Created, Duration::from_millis(30));
        stats.note_frame_creation(CreationOutcome::Created, Duration::from_millis(10));
        stats.note_frame_creation(CreationOutcome::Failed, Duration::from_millis(1));
        assert_eq!((stats.created_in_frame, stats.create_failed_in_frame), (2, 1));
        assert_eq!(stats.last_frame_compile, Duration::from_millis(1));
        assert_eq!(stats.longest_frame_compile, Duration::from_millis(30));
        // Заготовка не смешивается со счётчиками кадра.
        assert_eq!((stats.created_by_prewarm, stats.prewarm_failed), (0, 0));
    }

    #[test]
    fn prewarm_accumulates_counts_and_keeps_the_longest_batch() {
        let mut stats = VariantCacheStats::default();
        stats.note_prewarm(11, 0, Duration::from_millis(80));
        stats.note_prewarm(3, 1, Duration::from_millis(20));
        assert_eq!((stats.created_by_prewarm, stats.prewarm_failed), (14, 1));
        assert_eq!(stats.last_prewarm, Duration::from_millis(20));
        assert_eq!(stats.longest_prewarm, Duration::from_millis(80));
        assert_eq!(stats.created_in_frame, 0, "заготовка не считается созданием в кадре");
    }
}

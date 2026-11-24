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
    /// Наибольшее CPU-время одной заготовки на потоке рендера.
    pub longest_prewarm: Duration,
    /// Из `created_by_prewarm`: сколько создал ФОНОВЫЙ поток и подобрал поток рендера между кадрами.
    pub created_in_background: u64,
    /// Из `created_in_frame`: сколько пришлось создать в кадре, пока фон ещё не выдал этот вариант (вынут из очереди
    /// или фон не уложился в лимит ожидания). После окончания фона не растёт.
    pub created_in_frame_while_background_pending: u64,
    /// Сколько раз кадр дождался варианта, который фон компилировал прямо тогда (вместо второй компиляции).
    pub waited_for_background: u64,
    /// Наибольшее ожидание кадром фона.
    pub longest_background_wait: Duration,
    /// Наибольшее CPU-время компиляции одного варианта на фоновом потоке (с учётом конкуренции с потоком рендера).
    pub longest_background_compile: Duration,
    /// Фон собрал вариант, который уже не нужен или не помещается (дубль после ожидания, кэш полон): уничтожен.
    pub background_discarded: u64,
    /// Рабочий поток фона прервался (паника задания): дальше варианты создаются по требованию.
    pub background_aborted: bool,
    /// ОС не дала поток для фона: заготовка идёт на потоке рендера.
    pub background_unavailable: bool,
    /// Через сколько после создания ISP фон выполнил последнее задание («всё семейство готово»); `None` — ещё идёт
    /// или фон не работал.
    pub background_drained_after: Option<Duration>,
    /// Наибольшая остановка потока рендера на компиляции за один кадр: создание нужного варианта, ожидание фона и
    /// заготовка на потоке рендера вместе.
    pub longest_render_stall: Duration,
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

    /// Учитывает остановку потока рендера на одном кадре (от промаха кэша до готовности варианта).
    pub(crate) fn note_render_stall(&mut self, elapsed: Duration) {
        self.longest_render_stall = self.longest_render_stall.max(elapsed);
    }

    /// Учитывает вариант, который создал фон и подобрал поток рендера.
    pub(crate) fn note_background_adopted(&mut self, compile: Duration) {
        self.longest_background_compile = self.longest_background_compile.max(compile);
        self.created_in_background += 1;
        self.created_by_prewarm += 1;
    }

    /// Учитывает ожидание кадром варианта, который компилировал фон.
    pub(crate) fn note_background_wait(&mut self, waited: Duration) {
        self.waited_for_background += 1;
        self.longest_background_wait = self.longest_background_wait.max(waited);
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
    fn background_counters_are_separate_from_frame_and_render_thread_prewarm() {
        let mut stats = VariantCacheStats::default();
        stats.note_background_adopted(Duration::from_millis(5));
        stats.note_background_adopted(Duration::from_millis(5));
        stats.note_background_wait(Duration::from_millis(40));
        assert_eq!(stats.longest_background_compile, Duration::from_millis(5));
        stats.note_background_wait(Duration::from_millis(10));
        stats.note_render_stall(Duration::from_millis(7));
        stats.note_render_stall(Duration::from_millis(3));
        assert_eq!((stats.created_in_background, stats.created_by_prewarm), (2, 2));
        assert_eq!(stats.waited_for_background, 2);
        assert_eq!(stats.longest_background_wait, Duration::from_millis(40));
        assert_eq!(stats.longest_render_stall, Duration::from_millis(7));
        assert_eq!((stats.created_in_frame, stats.longest_prewarm), (0, Duration::ZERO));
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

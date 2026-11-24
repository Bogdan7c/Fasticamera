//! Журнал времён проходов одной работы за прогон насыщения (сессия 16a).
//!
//! Работа (адаптер) после каждого досчитанного кадра кладёт сюда длительности своих проходов
//! (по timestamp-запросам GPU), а в конце прогона берёт [`PassTimeSummary`]: медиана, p95 и максимум
//! по каждому проходу. Журнал хранит кольцо последних `capacity_frames` кадров; вся память выделяется в
//! [`PassTimeLog::new`], `record_frame` и `record_unmeasured_frame` не выделяют.
//!
//! Логика чистая (без GPU) и проверяется юнит-тестами внизу.

use std::time::Duration;

/// Доля для p95 (95-й процентиль по методу «ближайшего ранга»).
const P95_PERCENT: usize = 95;

/// Журнал времён проходов одной работы. См. описание модуля.
#[derive(Debug, Clone)]
pub struct PassTimeLog {
    /// Имена проходов в порядке записи.
    labels: Vec<&'static str>,
    /// Сколько последних кадров хранит кольцо.
    capacity_frames: usize,
    /// Плоское кольцо: кадр `f` занимает `[f * passes .. (f + 1) * passes]`.
    ring: Vec<Duration>,
    /// Слот кольца, куда ляжет следующий кадр.
    next_frame_slot: usize,
    /// Сколько слотов кольца занято (≤ `capacity_frames`).
    stored_frames: usize,
    /// Всего измеренных кадров за прогон (не ограничено кольцом).
    measured_frames: u64,
    /// Кадры без замера (timestamp-ы не готовы / не поддерживаются).
    unmeasured_frames: u64,
    /// Кадры, у которых число длительностей не совпало с числом меток.
    mismatched_frames: u64,
}

impl PassTimeLog {
    /// `labels` — имена проходов в порядке записи; `capacity_frames` > 0 (ноль поднимается до единицы,
    /// чтобы журнал не мог «потерять» все кадры из-за неверной настройки).
    pub fn new(labels: &[&'static str], capacity_frames: usize) -> Self {
        let capacity_frames = capacity_frames.max(1);
        Self {
            labels: labels.to_vec(),
            capacity_frames,
            ring: vec![Duration::ZERO; capacity_frames * labels.len()],
            next_frame_slot: 0,
            stored_frames: 0,
            measured_frames: 0,
            unmeasured_frames: 0,
            mismatched_frames: 0,
        }
    }

    /// Времена проходов одного досчитанного кадра, в порядке `labels`. Длина не совпала — кадр не
    /// учитывается, растёт счётчик расхождений.
    pub fn record_frame(&mut self, pass_durations: &[Duration]) {
        let passes = self.labels.len();
        if pass_durations.len() != passes {
            self.mismatched_frames += 1;
            return;
        }
        let start = self.next_frame_slot * passes;
        self.ring[start..start + passes].copy_from_slice(pass_durations);
        self.next_frame_slot = (self.next_frame_slot + 1) % self.capacity_frames;
        self.stored_frames = (self.stored_frames + 1).min(self.capacity_frames);
        self.measured_frames += 1;
    }

    /// Кадр без замера (timestamp-ы не готовы / не поддерживаются) — отдельный счётчик, не ноль.
    pub fn record_unmeasured_frame(&mut self) {
        self.unmeasured_frames += 1;
    }

    /// Итог по кадрам, оставшимся в кольце. Выделяет память (вызывать вне цикла кадров).
    pub fn summary(&self) -> PassTimeSummary {
        let passes = self.labels.len();
        let mut series = Vec::with_capacity(self.stored_frames);
        let pass_times = self
            .labels
            .iter()
            .enumerate()
            .map(|(pass_index, &label)| {
                series.clear();
                // Порядок в кольце не важен: статистика не зависит от порядка кадров.
                series.extend((0..self.stored_frames).map(|frame| self.ring[frame * passes + pass_index]));
                series.sort_unstable();
                PassTime {
                    label,
                    median: median_of_sorted(&series),
                    p95: nearest_rank_of_sorted(&series, P95_PERCENT),
                    max: series.last().copied().unwrap_or(Duration::ZERO),
                }
            })
            .collect();
        PassTimeSummary {
            passes: pass_times,
            measured_frames: self.measured_frames,
            unmeasured_frames: self.unmeasured_frames,
            mismatched_frames: self.mismatched_frames,
        }
    }

    /// Обнуляет журнал (кольцо и все счётчики), например между прогревом и окном.
    pub fn clear(&mut self) {
        self.next_frame_slot = 0;
        self.stored_frames = 0;
        self.measured_frames = 0;
        self.unmeasured_frames = 0;
        self.mismatched_frames = 0;
    }
}

/// Медиана отсортированного ряда: середина или среднее двух средних; пустой ряд — ноль.
fn median_of_sorted(sorted: &[Duration]) -> Duration {
    match sorted.len() {
        0 => Duration::ZERO,
        len if len % 2 == 1 => sorted[len / 2],
        len => (sorted[len / 2 - 1] + sorted[len / 2]) / 2,
    }
}

/// Процентиль методом ближайшего ранга: элемент с рангом `ceil(percent/100 * n)`; пустой ряд — ноль.
fn nearest_rank_of_sorted(sorted: &[Duration], percent: usize) -> Duration {
    if sorted.is_empty() {
        return Duration::ZERO;
    }
    let rank = (percent * sorted.len()).div_ceil(100).max(1);
    sorted[rank - 1]
}

/// Итог журнала: статистика по проходам и счётчики кадров.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PassTimeSummary {
    /// Проходы в порядке записи.
    pub passes: Vec<PassTime>,
    /// Кадры с замером за всё время (не только в кольце).
    pub measured_frames: u64,
    /// Кадры без замера.
    pub unmeasured_frames: u64,
    /// Кадры, отброшенные из-за несовпадения числа проходов.
    pub mismatched_frames: u64,
}

impl PassTimeSummary {
    /// Сумма медиан проходов.
    pub fn total_median(&self) -> Duration {
        self.passes.iter().map(|pass| pass.median).sum()
    }
}

/// Статистика одного прохода по кадрам в кольце.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PassTime {
    /// Имя прохода.
    pub label: &'static str,
    /// Медиана.
    pub median: Duration,
    /// 95-й процентиль (ближайший ранг).
    pub p95: Duration,
    /// Максимум.
    pub max: Duration,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn micros(values: &[u64]) -> Vec<Duration> {
        values.iter().map(|&value| Duration::from_micros(value)).collect()
    }

    #[test]
    fn median_p95_and_max_of_known_series() {
        let mut log = PassTimeLog::new(&["a", "b"], 100);
        // Проход «a»: 1..=20 мкс (в перемешанном порядке), проход «b» — константа 7.
        for value in [11, 3, 20, 1, 7, 15, 9, 2, 18, 5, 13, 4, 19, 8, 6, 17, 10, 12, 14, 16] {
            log.record_frame(&micros(&[value, 7]));
        }
        let summary = log.summary();
        assert_eq!(summary.measured_frames, 20);
        let a = summary.passes[0];
        assert_eq!(a.label, "a");
        // Чётное число кадров: среднее 10 и 11 = 10,5 мкс.
        assert_eq!(a.median, Duration::from_nanos(10_500));
        // Ранг ceil(0,95 * 20) = 19 → 19 мкс.
        assert_eq!(a.p95, Duration::from_micros(19));
        assert_eq!(a.max, Duration::from_micros(20));
        let b = summary.passes[1];
        assert_eq!(
            (b.median, b.p95, b.max),
            (
                Duration::from_micros(7),
                Duration::from_micros(7),
                Duration::from_micros(7)
            )
        );
        assert_eq!(summary.total_median(), Duration::from_nanos(17_500));
    }

    #[test]
    fn ring_evicts_oldest_frames_but_keeps_total_count() {
        let mut log = PassTimeLog::new(&["a"], 3);
        for value in [100, 200, 300, 1, 2, 3] {
            log.record_frame(&micros(&[value]));
        }
        let summary = log.summary();
        // В кольце только 1, 2, 3: старые большие значения вытеснены.
        assert_eq!(summary.passes[0].max, Duration::from_micros(3));
        assert_eq!(summary.passes[0].median, Duration::from_micros(2));
        assert_eq!(summary.measured_frames, 6);
    }

    #[test]
    fn mismatch_and_unmeasured_frames_have_separate_counters() {
        let mut log = PassTimeLog::new(&["a", "b"], 4);
        log.record_frame(&micros(&[1, 2]));
        log.record_frame(&micros(&[1])); // неверная длина
        log.record_unmeasured_frame();
        log.record_unmeasured_frame();
        let summary = log.summary();
        assert_eq!(
            (
                summary.measured_frames,
                summary.unmeasured_frames,
                summary.mismatched_frames
            ),
            (1, 2, 1)
        );
        // Расхождение не испортило статистику.
        assert_eq!(summary.passes[0].max, Duration::from_micros(1));
        assert_eq!(summary.passes[1].max, Duration::from_micros(2));
    }

    #[test]
    fn empty_log_gives_zero_times_and_clear_resets_everything() {
        let mut log = PassTimeLog::new(&["a"], 2);
        assert_eq!(log.summary().passes[0].median, Duration::ZERO);
        log.record_frame(&micros(&[5]));
        log.record_unmeasured_frame();
        log.record_frame(&micros(&[1, 2]));
        log.clear();
        let summary = log.summary();
        assert_eq!(
            (
                summary.measured_frames,
                summary.unmeasured_frames,
                summary.mismatched_frames
            ),
            (0, 0, 0)
        );
        assert_eq!(summary.passes[0].max, Duration::ZERO);
        // После очистки журнал снова работает с начала кольца.
        log.record_frame(&micros(&[9]));
        assert_eq!(log.summary().passes[0].max, Duration::from_micros(9));
    }

    #[test]
    fn zero_capacity_is_raised_to_one_frame() {
        let mut log = PassTimeLog::new(&["a"], 0);
        log.record_frame(&micros(&[4]));
        log.record_frame(&micros(&[6]));
        assert_eq!(log.summary().passes[0].max, Duration::from_micros(6));
    }
}

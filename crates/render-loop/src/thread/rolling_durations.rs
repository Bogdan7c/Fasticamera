//! Скользящее окно длительностей с перцентилями (оценка стоимости кадра, диагностика).
//!
//! Окно — последние `capacity` замеров; перцентиль — «ближайший ранг» (значение, ниже или
//! равно которому не меньше `q` всех замеров). На 120–600 замерах выбор через
//! `select_nth_unstable` — единицы микросекунд, поэтому оценка считается на каждом кадре;
//! буфер выбора переиспользуется, выделений памяти на кадре нет.

use std::collections::VecDeque;
use std::time::Duration;

/// Сводка окна: p50/p95/max и число замеров.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DurationSummary {
    pub samples: usize,
    pub p50: Duration,
    pub p95: Duration,
    pub max: Duration,
}

/// Окно последних замеров.
#[derive(Debug)]
pub(crate) struct RollingDurations {
    samples: VecDeque<Duration>,
    capacity: usize,
    /// Переиспользуемый буфер для выбора перцентиля (окно не переупорядочивается).
    selection: Vec<Duration>,
}

impl RollingDurations {
    /// `capacity` проверен конфигом (≥ 1); ноль всё равно не ломает окно — оно просто пустое.
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            samples: VecDeque::with_capacity(capacity),
            capacity,
            selection: Vec::with_capacity(capacity),
        }
    }

    /// Добавляет замер, вытесняя самый старый сверх ёмкости.
    pub(crate) fn push(&mut self, sample: Duration) {
        if self.capacity == 0 {
            return;
        }
        if self.samples.len() == self.capacity {
            self.samples.pop_front();
        }
        self.samples.push_back(sample);
    }

    pub(crate) fn len(&self) -> usize {
        self.samples.len()
    }

    /// Перцентиль `quantile` ∈ (0, 1] по ближайшему рангу; `None` — замеров нет.
    pub(crate) fn percentile(&mut self, quantile: f64) -> Option<Duration> {
        let count = self.samples.len();
        if count == 0 {
            return None;
        }
        // Ранг ceil(q·n) от 1; q вне (0, 1] прижимается к краям окна.
        let rank = (quantile * count as f64).ceil();
        let index = if rank.is_nan() || rank < 1.0 {
            0
        } else {
            (rank as usize).min(count) - 1
        };
        self.selection.clear();
        self.selection.extend(self.samples.iter().copied());
        let (_, selected, _) = self.selection.select_nth_unstable(index);
        Some(*selected)
    }

    /// p50/p95/max окна; `None` — замеров нет.
    pub(crate) fn summary(&mut self) -> Option<DurationSummary> {
        let max = self.samples.iter().copied().max()?;
        Some(DurationSummary {
            samples: self.samples.len(),
            p50: self.percentile(0.50)?,
            p95: self.percentile(0.95)?,
            max,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn millis(values: &[u64]) -> Vec<Duration> {
        values.iter().map(|&value| Duration::from_millis(value)).collect()
    }

    #[test]
    fn p95_of_hundred_samples_is_the_ninety_fifth_value() {
        let mut window = RollingDurations::new(100);
        // 1..=100 мс в перемешанном порядке: перцентиль не зависит от порядка поступления.
        for value in (1..=100u64).rev() {
            window.push(Duration::from_millis(value));
        }
        assert_eq!(window.percentile(0.95), Some(Duration::from_millis(95)));
        assert_eq!(window.percentile(0.50), Some(Duration::from_millis(50)));
        let summary = window.summary().expect("замеры есть");
        assert_eq!(summary.max, Duration::from_millis(100));
        assert_eq!(summary.samples, 100);
    }

    #[test]
    fn oldest_samples_leave_the_window() {
        let mut window = RollingDurations::new(3);
        for sample in millis(&[50, 1, 2, 3]) {
            window.push(sample);
        }
        assert_eq!(window.len(), 3);
        assert_eq!(
            window.summary().expect("замеры").max,
            Duration::from_millis(3),
            "выброс 50 мс вытеснен"
        );
    }

    #[test]
    fn empty_window_has_no_percentile() {
        let mut window = RollingDurations::new(4);
        assert_eq!(window.percentile(0.95), None);
        assert_eq!(window.summary(), None);
    }

    #[test]
    fn single_outlier_in_twenty_does_not_move_p95() {
        let mut window = RollingDurations::new(20);
        for _ in 0..19 {
            window.push(Duration::from_millis(2));
        }
        window.push(Duration::from_millis(40));
        assert_eq!(window.percentile(0.95), Some(Duration::from_millis(2)));
        window.push(Duration::from_millis(40));
        assert_eq!(
            window.percentile(0.95),
            Some(Duration::from_millis(40)),
            "два выброса из 20 — уже p95"
        );
    }
}

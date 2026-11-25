//! Счётчик установившегося периода «каждый N-й кадр» (внутренность [`super::StatisticsSchedule`]).

use std::num::NonZeroU32;

use super::StatisticsPlan;

/// Счётчик кадров после последнего кадра со статистикой.
#[derive(Debug, Clone, Copy)]
pub(super) struct PeriodCounter {
    every_n_frames: NonZeroU32,
    /// Сколько кадров прошло после последнего кадра со статистикой.
    frames_since_statistics: u32,
}

impl PeriodCounter {
    /// Период «каждый N-й кадр» (1 — на каждом).
    pub(super) fn new(every_n_frames: NonZeroU32) -> Self {
        Self {
            every_n_frames,
            frames_since_statistics: 0,
        }
    }

    /// Установившийся период.
    pub(super) fn every_n_frames(&self) -> NonZeroU32 {
        self.every_n_frames
    }

    /// Новый поток: первый кадр обязан дать статистику.
    pub(super) fn restart(&mut self) {
        self.frames_since_statistics = self.every_n_frames.get();
    }

    /// Решение для очередного кадра. `frames_hidden` — кадры спрятаны холодным стартом: статистика нужна на
    /// каждом, а период отсчитывается заново от последнего такого кадра.
    pub(super) fn tick(&mut self, frames_hidden: bool) -> StatisticsPlan {
        if frames_hidden || self.frames_since_statistics.saturating_add(1) >= self.every_n_frames.get() {
            self.frames_since_statistics = 0;
            StatisticsPlan::Compute
        } else {
            self.frames_since_statistics += 1;
            StatisticsPlan::Skip
        }
    }
}

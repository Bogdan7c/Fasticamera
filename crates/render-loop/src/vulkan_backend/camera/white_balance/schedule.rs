//! Прореживание статистики AWB: на GPU она считается на каждом N-м обработанном кадре. Статистика нужна не только
//! старому оценщику: на ней стоит AE (общая статистика кадра).
//!
//! Стадия статистики стоит заметного тока (≈ 55 мА из ≈ 119 мА ISP, 07a), а свет сцены меняется
//! медленнее, чем идут кадры. Но пока кадры спрятаны холодным стартом (ждём ответ источника), статистика нужна на КАЖДОМ кадре —
//! иначе скрытие затянулось бы на лишние кадры.

/// Нужна ли статистика на этом кадре.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatisticsPlan {
    /// Включить стадию статистики в проходе ISP.
    Compute,
    /// Пропустить стадию: оценка свежая достаточно.
    Skip,
}

/// Расписание статистики одного потока.
#[derive(Debug, Clone, Copy)]
pub struct StatisticsSchedule {
    every_n_frames: u32,
    /// Сколько кадров прошло после последнего кадра со статистикой.
    frames_since_statistics: u32,
}

impl StatisticsSchedule {
    /// Расписание «каждый N-й кадр» (`N ≥ 1`; 1 — на каждом).
    pub fn new(every_n_frames: u32) -> Self {
        Self {
            every_n_frames: every_n_frames.max(1),
            frames_since_statistics: 0,
        }
    }

    /// Новый поток: первый кадр обязан дать статистику.
    pub fn restart(&mut self) {
        self.frames_since_statistics = self.every_n_frames;
    }

    /// Решение для очередного обработанного кадра. `waiting_for_first_estimate` — идёт холодный старт.
    pub fn decide(&mut self, waiting_for_first_estimate: bool) -> StatisticsPlan {
        if waiting_for_first_estimate || self.frames_since_statistics + 1 >= self.every_n_frames {
            self.frames_since_statistics = 0;
            StatisticsPlan::Compute
        } else {
            self.frames_since_statistics += 1;
            StatisticsPlan::Skip
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plans(schedule: &mut StatisticsSchedule, count: usize) -> Vec<StatisticsPlan> {
        (0..count).map(|_| schedule.decide(false)).collect()
    }

    #[test]
    fn every_fourth_frame_computes_statistics() {
        let mut schedule = StatisticsSchedule::new(4);
        schedule.restart();
        let expected = [
            StatisticsPlan::Compute,
            StatisticsPlan::Skip,
            StatisticsPlan::Skip,
            StatisticsPlan::Skip,
        ];
        let plans = plans(&mut schedule, 12);
        assert_eq!(plans[..4], expected);
        assert_eq!(plans[4..8], expected);
        assert_eq!(plans[8..], expected);
    }

    #[test]
    fn every_frame_when_period_is_one() {
        let mut schedule = StatisticsSchedule::new(1);
        assert!(
            plans(&mut schedule, 5)
                .iter()
                .all(|plan| *plan == StatisticsPlan::Compute)
        );
    }

    #[test]
    fn cold_start_forces_statistics_on_every_frame_and_resets_the_period() {
        let mut schedule = StatisticsSchedule::new(4);
        schedule.restart();
        for _ in 0..3 {
            assert_eq!(schedule.decide(true), StatisticsPlan::Compute);
        }
        // После первой оценки период отсчитывается от последнего кадра со статистикой.
        assert_eq!(schedule.decide(false), StatisticsPlan::Skip);
        assert_eq!(schedule.decide(false), StatisticsPlan::Skip);
        assert_eq!(schedule.decide(false), StatisticsPlan::Skip);
        assert_eq!(schedule.decide(false), StatisticsPlan::Compute);
    }

    #[test]
    fn restart_makes_the_first_frame_of_a_new_stream_compute() {
        let mut schedule = StatisticsSchedule::new(4);
        schedule.restart();
        let _ = plans(&mut schedule, 2);
        schedule.restart();
        assert_eq!(schedule.decide(false), StatisticsPlan::Compute);
    }
}

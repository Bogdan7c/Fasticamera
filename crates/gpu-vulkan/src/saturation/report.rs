//! Учёт завершённых кадров по настенным часам и итог прогона ([`SaturationReport`]).
//!
//! Логика чистая (только `Instant`), поэтому проверяется без GPU: см. тесты внизу.

use std::time::{Duration, Instant};

use super::config::SaturationConfig;

/// Почему прогон закончился.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndReason {
    /// Окно замера отработано целиком.
    Completed,
    /// Остановлен снаружи; итог содержит только то, что успели намерить.
    Cancelled,
}

/// Пропускная способность за окно замера.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Throughput {
    /// В окне не завершилось ни одного кадра (окно нулевой длины или работа тяжелее окна): цифры нет, делить не на что.
    NoFramesCompleted,
    /// Кадры в окне были.
    Measured {
        /// Кадров в секунду (по настенным часам).
        frames_per_second: f64,
        /// Цена кадра = длительность окна / число кадров.
        frame_cost: Duration,
    },
}

/// Один шаг временной шкалы: по нему видно разгон частоты GPU и троттлинг.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TimelineStep {
    /// Смещение начала шага от начала окна замера.
    pub offset: Duration,
    /// Длина шага (последний шаг может быть короче остальных).
    pub duration: Duration,
    /// Кадры, завершившиеся в этом шаге.
    pub frames: u64,
}

impl TimelineStep {
    /// Пропускная способность шага, кадров в секунду.
    pub fn frames_per_second(&self) -> f64 {
        frames_per_second(self.frames, self.duration)
    }
}

/// Итог прогона стенда насыщения.
///
/// Каждый кадр, дошедший до GPU и дождавшийся fence, попадает ровно в одну из трёх корзин:
/// прогрев ([`Self::warmup_frames`]), окно замера ([`Self::measured_frames`]), хвост после окна
/// ([`Self::drained_frames`]). Сумма корзин = число кадров, которым работа получила `release_slot_after_fence`.
#[derive(Debug, Clone, PartialEq)]
pub struct SaturationReport {
    /// Почему прогон закончился.
    pub end_reason: EndReason,
    /// Кадры, завершившиеся до начала окна (прогрев; в цифру не входят).
    pub warmup_frames: u64,
    /// Кадры, завершившиеся внутри окна замера.
    pub measured_frames: u64,
    /// Кадры, дождавшиеся fence уже после окна (хвост очереди при остановке; в цифру не входят).
    pub drained_frames: u64,
    /// Фактическая длина окна замера по настенным часам (при отмене — короче настроенной).
    pub window: Duration,
    /// Пропускная способность окна.
    pub throughput: Throughput,
    /// Шкала по шагам; сумма кадров шагов = `measured_frames`.
    pub timeline: Vec<TimelineStep>,
}

impl SaturationReport {
    /// Все кадры, закончившиеся за прогон (три корзины вместе).
    pub fn total_completed_frames(&self) -> u64 {
        self.warmup_frames + self.measured_frames + self.drained_frames
    }
}

/// Кадры в секунду; нулевая длительность даёт 0.0, а не деление на ноль.
fn frames_per_second(frames: u64, duration: Duration) -> f64 {
    let seconds = duration.as_secs_f64();
    if seconds > 0.0 { frames as f64 / seconds } else { 0.0 }
}

/// В какую корзину попал завершённый кадр.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FrameBucket {
    Warmup,
    /// Внутри окна, в шаге с этим номером.
    Measured {
        step_index: usize,
    },
    AfterWindow,
}

/// Накопитель кадров прогона. Вся память шкалы выделяется в [`Self::new`]: в цикле кадров аллокаций нет.
pub(crate) struct FrameTally {
    warmup_end: Instant,
    /// Конец окна; при отмене/завершении сдвигается на момент остановки (кадры после него — хвост).
    window_end: Instant,
    timeline_step: Duration,
    configured_steps: usize,
    frames_per_step: Vec<u64>,
    warmup_frames: u64,
    drained_frames: u64,
}

impl FrameTally {
    /// `started` — начало прогона (начало прогрева). Конфиг уже проверен.
    pub(crate) fn new(config: &SaturationConfig, started: Instant) -> Self {
        let warmup_end = started + config.warmup;
        let configured_steps = usize::try_from(config.timeline_step_count()).unwrap_or(usize::MAX);
        Self {
            warmup_end,
            window_end: warmup_end + config.measure,
            timeline_step: config.timeline_step,
            configured_steps,
            frames_per_step: vec![0; configured_steps],
            warmup_frames: 0,
            drained_frames: 0,
        }
    }

    /// Момент, после которого новые кадры не отправляются (конец окна).
    pub(crate) fn window_end(&self) -> Instant {
        self.window_end
    }

    /// Закрывает окно в момент `at` (отмена): кадры, наблюдённые позже, считаются хвостом. Окно не короче нуля
    /// и не длиннее настроенного.
    pub(crate) fn close_window_at(&mut self, at: Instant) {
        self.window_end = at.clamp(self.warmup_end, self.window_end);
    }

    /// Учитывает кадр, чей fence наблюдён сигнальным в момент `observed_at`: кадр засчитывается в тот шаг,
    /// где завершился (квантование: ошибка не больше одного кадра на границе шага).
    pub(crate) fn record_completion(&mut self, observed_at: Instant) -> FrameBucket {
        if observed_at < self.warmup_end {
            self.warmup_frames += 1;
            return FrameBucket::Warmup;
        }
        if observed_at >= self.window_end {
            self.drained_frames += 1;
            return FrameBucket::AfterWindow;
        }
        let since_window_start = observed_at.duration_since(self.warmup_end);
        // Шаг > 0 проверен конфигом; индекс ограничен числом шагов (граница window_end ≤ полное окно).
        let raw_index = since_window_start.as_nanos() / self.timeline_step.as_nanos().max(1);
        let step_index = usize::try_from(raw_index)
            .unwrap_or(usize::MAX)
            .min(self.configured_steps.saturating_sub(1));
        self.frames_per_step[step_index] += 1;
        FrameBucket::Measured { step_index }
    }

    /// Сколько кадров всего завершилось (для сообщения об ошибке).
    pub(crate) fn total_completed(&self) -> u64 {
        self.warmup_frames + self.drained_frames + self.frames_per_step.iter().sum::<u64>()
    }

    /// Собирает итог.
    pub(crate) fn finish(self, end_reason: EndReason) -> SaturationReport {
        let window = self.window_end.duration_since(self.warmup_end);
        let measured_frames: u64 = self.frames_per_step.iter().sum();
        let timeline = self.build_timeline(window);
        let throughput = if measured_frames == 0 {
            Throughput::NoFramesCompleted
        } else {
            Throughput::Measured {
                frames_per_second: frames_per_second(measured_frames, window),
                // Кадры > 0 и окно > 0 (кадр внутри окна возможен только при ненулевом окне).
                frame_cost: window.div_f64(measured_frames as f64),
            }
        };
        SaturationReport {
            end_reason,
            warmup_frames: self.warmup_frames,
            measured_frames,
            drained_frames: self.drained_frames,
            window,
            throughput,
            timeline,
        }
    }

    /// Шкала по шагам, покрывающая фактическое окно (при отмене — только пройденные шаги).
    fn build_timeline(&self, window: Duration) -> Vec<TimelineStep> {
        let step_nanos = self.timeline_step.as_nanos().max(1);
        let steps = usize::try_from(window.as_nanos().div_ceil(step_nanos))
            .unwrap_or(usize::MAX)
            .min(self.configured_steps);
        let mut timeline = Vec::with_capacity(steps);
        for (step_index, frames) in self.frames_per_step.iter().take(steps).enumerate() {
            let offset = self
                .timeline_step
                .saturating_mul(u32::try_from(step_index).unwrap_or(u32::MAX));
            timeline.push(TimelineStep {
                offset,
                duration: self.timeline_step.min(window.saturating_sub(offset)),
                frames: *frames,
            });
        }
        timeline
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(warmup_ms: u64, measure_ms: u64, step_ms: u64) -> SaturationConfig {
        SaturationConfig {
            warmup: Duration::from_millis(warmup_ms),
            measure: Duration::from_millis(measure_ms),
            timeline_step: Duration::from_millis(step_ms),
            ..SaturationConfig::default()
        }
    }

    #[test]
    fn frames_land_in_warmup_step_or_tail_by_completion_instant() {
        let start = Instant::now();
        let mut tally = FrameTally::new(&config(1000, 2000, 1000), start);
        let at = |ms: u64| start + Duration::from_millis(ms);
        assert_eq!(tally.record_completion(at(999)), FrameBucket::Warmup);
        assert_eq!(
            tally.record_completion(at(1000)),
            FrameBucket::Measured { step_index: 0 }
        );
        assert_eq!(
            tally.record_completion(at(1999)),
            FrameBucket::Measured { step_index: 0 }
        );
        assert_eq!(
            tally.record_completion(at(2000)),
            FrameBucket::Measured { step_index: 1 }
        );
        assert_eq!(
            tally.record_completion(at(2999)),
            FrameBucket::Measured { step_index: 1 }
        );
        assert_eq!(tally.record_completion(at(3000)), FrameBucket::AfterWindow);
        let report = tally.finish(EndReason::Completed);
        assert_eq!(
            (report.warmup_frames, report.measured_frames, report.drained_frames),
            (1, 4, 1)
        );
        let per_step: Vec<u64> = report.timeline.iter().map(|step| step.frames).collect();
        assert_eq!(per_step, vec![2, 2]);
        assert_eq!(report.total_completed_frames(), 6);
        match report.throughput {
            Throughput::Measured {
                frames_per_second,
                frame_cost,
            } => {
                assert!((frames_per_second - 2.0).abs() < 1e-9, "{frames_per_second}");
                assert_eq!(frame_cost, Duration::from_millis(500));
            }
            other => panic!("ожидали Measured, получили {other:?}"),
        }
    }

    #[test]
    fn window_without_frames_is_a_distinct_state_not_a_division_by_zero() {
        let start = Instant::now();
        let mut tally = FrameTally::new(&config(0, 1000, 500), start);
        tally.record_completion(start + Duration::from_millis(5000));
        let report = tally.finish(EndReason::Completed);
        assert_eq!(report.throughput, Throughput::NoFramesCompleted);
        assert_eq!(report.measured_frames, 0);
        assert_eq!(report.drained_frames, 1);
    }

    #[test]
    fn cancel_inside_warmup_gives_zero_window_and_all_later_frames_are_tail() {
        let start = Instant::now();
        let mut tally = FrameTally::new(&config(1000, 1000, 500), start);
        tally.close_window_at(start + Duration::from_millis(300));
        tally.record_completion(start + Duration::from_millis(400));
        let report = tally.finish(EndReason::Cancelled);
        assert_eq!(report.window, Duration::ZERO);
        assert_eq!(report.throughput, Throughput::NoFramesCompleted);
        assert!(report.timeline.is_empty());
        // Закрытие окна внутри прогрева сдвигает конец окна на конец прогрева: кадр до него — прогрев.
        assert_eq!((report.warmup_frames, report.drained_frames), (1, 0));
    }

    #[test]
    fn cancel_mid_window_truncates_timeline_and_last_step_is_partial() {
        let start = Instant::now();
        let mut tally = FrameTally::new(&config(0, 3000, 1000), start);
        tally.record_completion(start + Duration::from_millis(100));
        tally.record_completion(start + Duration::from_millis(1200));
        tally.close_window_at(start + Duration::from_millis(1500));
        tally.record_completion(start + Duration::from_millis(1600));
        let report = tally.finish(EndReason::Cancelled);
        assert_eq!(report.window, Duration::from_millis(1500));
        assert_eq!(report.measured_frames, 2);
        assert_eq!(report.drained_frames, 1);
        assert_eq!(report.timeline.len(), 2);
        assert_eq!(report.timeline[1].duration, Duration::from_millis(500));
        assert!((report.timeline[1].frames_per_second() - 2.0).abs() < 1e-9);
    }
}

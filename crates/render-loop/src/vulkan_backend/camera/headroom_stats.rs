//! Накопление диагностики стенда V4: счётчики и окна времени GPU по проходам.
//!
//! Владелец — `ViewfinderStats` (бэкенд), поэтому счётчики и окна переживают пересоздание surface
//! и выключение стенда (замер можно снять после выключения). Чистая логика, проверяется без GPU.

use std::time::Duration;

use isp_vulkan::quality::{HeadroomPass, HeadroomTimingCollect, HistoryUse};

use crate::thread::RollingDurations;
use crate::viewfinder::{HeadroomBench, HeadroomBenchDiagnostics, HeadroomPassTime, ViewfinderExtent};

/// Живое состояние объектов стенда (его знает только сессия показа).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct HeadroomLiveState {
    /// Размер живой нагрузки (`None` — не создана).
    pub workload_extent: Option<ViewfinderExtent>,
    /// Сколько нагрузок ждут fence для освобождения.
    pub workloads_awaiting_release: usize,
}

/// Диагностика стенда, копящаяся за жизнь бэкенда.
#[derive(Debug)]
pub struct HeadroomBenchStats {
    /// Счётчики (поля окон и живого состояния здесь не используются — заполняются в снимке).
    counters: HeadroomBenchDiagnostics,
    /// Окно на каждый проход в порядке исполнения (`HeadroomPass::ORDER`).
    passes: Vec<(HeadroomPass, RollingDurations)>,
    total: RollingDurations,
}

impl HeadroomBenchStats {
    /// Пустая статистика с окнами по `window_frames` замеров.
    pub fn new(window_frames: usize) -> Self {
        Self {
            counters: HeadroomBenchDiagnostics::default(),
            passes: HeadroomPass::ORDER
                .iter()
                .map(|pass| (*pass, RollingDurations::new(window_frames)))
                .collect(),
            total: RollingDurations::new(window_frames),
        }
    }

    /// Кадр записан через стенд в сабмит, принятый очередью.
    pub fn note_frame_recorded(&mut self, history: Option<HistoryUse>) {
        self.counters.frames_recorded += 1;
        if history == Some(HistoryUse::Restarted) {
            self.counters.history_restarts += 1;
        }
    }

    /// Приёмник занят: кадр пропущен (ничего не записано).
    pub fn note_sink_busy(&mut self) {
        self.counters.sink_busy_skips += 1;
    }

    /// Параметров качественного ISP нет: кадр через стенд не идёт.
    pub fn note_no_params(&mut self) {
        self.counters.frames_skipped_no_params += 1;
    }

    /// Поток камеры сменился (история шумодава начнётся заново).
    pub fn note_stream_change(&mut self) {
        self.counters.stream_changes += 1;
    }

    /// Создана нагрузка.
    pub fn note_workload_created(&mut self) {
        self.counters.workloads_created += 1;
    }

    /// Освобождено `count` нагрузок после fence.
    pub fn note_workloads_released(&mut self, count: usize) {
        self.counters.workloads_released += u64::try_from(count).unwrap_or(u64::MAX);
    }

    /// Отказ стенда с причиной.
    pub fn note_failure(&mut self, reason: String) {
        self.counters.failures += 1;
        self.counters.last_failure = Some(reason);
    }

    /// Метки времени стенда не собрались.
    pub fn note_timing_failure(&mut self) {
        self.counters.timing_failures += 1;
    }

    /// Время проходов слота (собрано после его fence). Не готовые и пустые слоты — не замер.
    pub fn note_timings(&mut self, collected: &HeadroomTimingCollect) {
        let HeadroomTimingCollect::Ready(timings) = collected else {
            return;
        };
        for timing in &timings.passes {
            if let Some((_, window)) = self.passes.iter_mut().find(|(pass, _)| *pass == timing.pass) {
                window.push(Duration::from_nanos(timing.duration_ns));
            }
        }
        self.total.push(Duration::from_nanos(timings.total_ns));
    }

    /// Снимок для статуса.
    pub fn snapshot(&mut self, mode: HeadroomBench, live: HeadroomLiveState) -> HeadroomBenchDiagnostics {
        let pass_times = self
            .passes
            .iter_mut()
            .filter_map(|(pass, window)| {
                window.summary().map(|gpu_time| HeadroomPassTime {
                    pass: pass.label(),
                    gpu_time,
                })
            })
            .collect();
        HeadroomBenchDiagnostics {
            mode,
            workload_extent: live.workload_extent,
            workloads_awaiting_release: live.workloads_awaiting_release,
            pass_times,
            total: self.total.summary(),
            ..self.counters.clone()
        }
    }
}

#[cfg(test)]
mod tests {
    use isp_vulkan::quality::{HeadroomTimings, MenonPass, PassTiming};

    use super::*;

    fn timings(levels_ns: u64, demosaic_ns: u64, output_ns: u64) -> HeadroomTimingCollect {
        HeadroomTimingCollect::Ready(HeadroomTimings {
            passes: vec![
                PassTiming {
                    pass: HeadroomPass::Levels,
                    duration_ns: levels_ns,
                },
                PassTiming {
                    pass: HeadroomPass::Demosaic(MenonPass::Decision),
                    duration_ns: demosaic_ns,
                },
                PassTiming {
                    pass: HeadroomPass::Output,
                    duration_ns: output_ns,
                },
            ],
            total_ns: levels_ns + demosaic_ns + output_ns,
        })
    }

    #[test]
    fn pass_windows_are_reported_by_name_in_execution_order() {
        let mut stats = HeadroomBenchStats::new(8);
        stats.note_timings(&timings(1_000_000, 2_000_000, 500_000));
        stats.note_timings(&timings(3_000_000, 2_000_000, 500_000));
        stats.note_timings(&HeadroomTimingCollect::NotReady);
        stats.note_timings(&HeadroomTimingCollect::NothingRecorded);
        let snapshot = stats.snapshot(HeadroomBench::Enabled, HeadroomLiveState::default());
        let names: Vec<&str> = snapshot.pass_times.iter().map(|time| time.pass).collect();
        assert_eq!(
            names,
            vec![
                HeadroomPass::Levels.label(),
                MenonPass::Decision.label(),
                HeadroomPass::Output.label()
            ],
            "проходы без замеров не перечисляются, порядок — исполнения"
        );
        let levels = snapshot.pass_times[0].gpu_time;
        assert_eq!(levels.samples, 2, "NotReady и пустой слот — не замеры");
        assert_eq!(levels.max, Duration::from_millis(3));
        assert_eq!(levels.p50, Duration::from_millis(1));
        assert_eq!(
            snapshot.total.map(|total| total.max),
            Some(Duration::from_micros(5_500))
        );
    }

    #[test]
    fn counters_accumulate_and_live_state_comes_from_the_snapshot_argument() {
        let mut stats = HeadroomBenchStats::new(4);
        stats.note_workload_created();
        stats.note_frame_recorded(Some(HistoryUse::Restarted));
        stats.note_frame_recorded(Some(HistoryUse::Blended));
        stats.note_frame_recorded(None);
        stats.note_sink_busy();
        stats.note_no_params();
        stats.note_stream_change();
        stats.note_failure("первый".to_owned());
        stats.note_failure("второй".to_owned());
        stats.note_timing_failure();
        stats.note_workloads_released(1);
        let live = HeadroomLiveState {
            workload_extent: Some(ViewfinderExtent { width: 64, height: 48 }),
            workloads_awaiting_release: 2,
        };
        let snapshot = stats.snapshot(HeadroomBench::Disabled, live);
        assert_eq!(snapshot.mode, HeadroomBench::Disabled);
        assert_eq!(snapshot.workload_extent, live.workload_extent);
        assert_eq!(snapshot.workloads_awaiting_release, 2);
        assert_eq!((snapshot.workloads_created, snapshot.workloads_released), (1, 1));
        assert_eq!(snapshot.frames_recorded, 3);
        assert_eq!(snapshot.history_restarts, 1, "смешивание с историей — не перезапуск");
        assert_eq!(
            (
                snapshot.sink_busy_skips,
                snapshot.frames_skipped_no_params,
                snapshot.stream_changes
            ),
            (1, 1, 1)
        );
        assert_eq!(snapshot.failures, 2);
        assert_eq!(snapshot.last_failure.as_deref(), Some("второй"));
        assert_eq!(snapshot.timing_failures, 1);
        assert!(snapshot.pass_times.is_empty() && snapshot.total.is_none());
    }
}

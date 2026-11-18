//! Общие числа сценариев видоискателя: время GPU в миллисекундах, приращения счётчиков ISP и
//! линии кадра за окно, медиана, имя цветового пространства, тепло. Чистые функции без
//! платформы — проверяются на ПК (`viewfinder_metrics_tests.rs`).

use std::time::{Duration, Instant};

use raw_frame_stream::RawStreamCounters;
use render_loop::thread::{DurationSummary, PresentTargetDiagnostics, VsyncShiftHistogram};
use render_loop::viewfinder::{ViewfinderColorSpace, ViewfinderCounters};
use serde::Serialize;

use super::raw_stream_deltas::stream_delta;
use super::raw_stream_samples::LatencySampler;
use super::viewfinder_pacing_report::PacingWindowReport;
use crate::camera_presentation::CameraPresentation;
use crate::raw_stream_services::RawStreamServices;

/// Миллисекунды с дробной частью.
pub fn millis(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1_000.0
}

/// Округление до сотых — для отчёта (бюджет строки результата).
pub fn round2(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

/// Сводка окна времени GPU в миллисекундах.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct GpuTimeMs {
    pub samples: usize,
    pub p50_ms: f64,
    pub p95_ms: f64,
    pub max_ms: f64,
}

impl GpuTimeMs {
    pub fn of(summary: &DurationSummary) -> Self {
        Self {
            samples: summary.samples,
            p50_ms: round2(millis(summary.p50)),
            p95_ms: round2(millis(summary.p95)),
            max_ms: round2(millis(summary.max)),
        }
    }

    pub fn of_optional(summary: Option<&DurationSummary>) -> Option<Self> {
        summary.map(Self::of)
    }
}

/// Медиана (для чётного числа — среднее двух средних); пусто или NaN — `None`.
pub fn median(values: &[f64]) -> Option<f64> {
    if values.is_empty() || values.iter().any(|value| value.is_nan()) {
        return None;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let middle = sorted.len() / 2;
    Some(if sorted.len().is_multiple_of(2) {
        (sorted[middle - 1] + sorted[middle]) / 2.0
    } else {
        sorted[middle]
    })
}

/// Имя цветового пространства экрана для отчёта: честно `srgb`, если P3 не вышло.
pub fn color_space_name(space: Option<ViewfinderColorSpace>) -> &'static str {
    match space {
        Some(ViewfinderColorSpace::DisplayP3) => "display_p3",
        Some(ViewfinderColorSpace::Srgb) => "srgb",
        None => "unknown",
    }
}

/// Приращения счётчиков ISP за окно (накопительные счётчики потока рендера).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct IspCountersDelta {
    pub frames_processed: u64,
    pub recomposed: u64,
    pub isp_record_failures: u64,
    pub compose_failures: u64,
    pub params_resolved: u64,
    pub params_failed: u64,
    pub frames_dropped_no_params: u64,
    pub frames_rejected_geometry: u64,
    pub statistics_published: u64,
    /// Обработано ISP, но скрыто на холодном старте AWB (решение 65).
    pub frames_hidden_cold_start: u64,
    /// Кадры брекетинга, возвращённые камере без ISP: в `frames_processed` не входят.
    pub bracket_frames_skipped: u64,
    pub gpu_timing_failures: u64,
    pub cpu_copies: u64,
}

impl IspCountersDelta {
    pub fn between(before: &ViewfinderCounters, after: &ViewfinderCounters) -> Self {
        let delta = |pick: fn(&ViewfinderCounters) -> u64| pick(after).saturating_sub(pick(before));
        Self {
            frames_processed: delta(|counters| counters.frames_processed),
            recomposed: delta(|counters| counters.recomposed),
            isp_record_failures: delta(|counters| counters.isp_record_failures),
            compose_failures: delta(|counters| counters.compose_failures),
            params_resolved: delta(|counters| counters.params_resolved),
            params_failed: delta(|counters| counters.params_failed),
            frames_dropped_no_params: delta(|counters| counters.frames_dropped_no_params),
            frames_rejected_geometry: delta(|counters| counters.frames_rejected_geometry),
            statistics_published: delta(|counters| counters.statistics_published),
            frames_hidden_cold_start: delta(|counters| counters.frames_hidden_cold_start),
            bracket_frames_skipped: delta(|counters| counters.bracket_frames_skipped),
            gpu_timing_failures: delta(|counters| counters.gpu_timing_failures),
            cpu_copies: delta(|counters| counters.cpu_copies),
        }
    }
}

/// Опоздания показа за окно: на сколько vsync кадр показан позже задуманной линии.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct VsyncShiftDelta {
    pub on_time: u64,
    pub late_1: u64,
    pub late_2: u64,
    pub late_3_plus: u64,
    pub early: u64,
}

impl VsyncShiftDelta {
    pub fn between(before: &VsyncShiftHistogram, after: &VsyncShiftHistogram) -> Self {
        let bucket = |index: usize| after.late_by_periods[index].saturating_sub(before.late_by_periods[index]);
        Self {
            on_time: bucket(0),
            late_1: bucket(1),
            late_2: bucket(2),
            late_3_plus: bucket(3),
            early: after.early.saturating_sub(before.early),
        }
    }

    /// Кадров с известным опозданием.
    pub fn total(&self) -> u64 {
        self.on_time + self.late_1 + self.late_2 + self.late_3_plus
    }

    /// Доля кадров, показанных позже задуманного vsync; `None` — кадров нет.
    pub fn miss_ratio(&self) -> Option<f64> {
        let total = self.total();
        (total > 0).then(|| (self.late_1 + self.late_2 + self.late_3_plus) as f64 / total as f64)
    }
}

/// Линия кадра за окно: опоздания, откаты, поздний захват «в обрез».
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct PresentTargetDelta {
    pub frames_planned: u64,
    pub fallbacks_to_preferred: u64,
    pub latch_already_passed: u64,
    /// Бэкенд отказался ждать момент захвата (срок дальше предела ожидания).
    pub latch_waits_refused: u64,
    pub chosen_earlier_than_preferred: u64,
    /// Тиков, пропущенных ради схода на ближнюю линию (решение 54).
    pub skipped_ticks_for_depth: u64,
    /// Сходов на ближнюю линию, подтверждённых опубликованным кадром.
    pub depth_reductions: u64,
    /// Пропусков тика без сработавшего схода (кадр не выиграл период).
    pub depth_skips_without_reduction: u64,
    pub vsync_shift: VsyncShiftDelta,
}

impl PresentTargetDelta {
    pub fn between(before: &PresentTargetDiagnostics, after: &PresentTargetDiagnostics) -> Self {
        Self {
            frames_planned: after.frames_planned.saturating_sub(before.frames_planned),
            fallbacks_to_preferred: after
                .fallbacks_to_preferred
                .saturating_sub(before.fallbacks_to_preferred),
            latch_already_passed: after.latch_already_passed.saturating_sub(before.latch_already_passed),
            latch_waits_refused: after.latch_waits_refused.saturating_sub(before.latch_waits_refused),
            chosen_earlier_than_preferred: after
                .chosen_earlier_than_preferred
                .saturating_sub(before.chosen_earlier_than_preferred),
            skipped_ticks_for_depth: after
                .skipped_ticks_for_depth
                .saturating_sub(before.skipped_ticks_for_depth),
            depth_reductions: after.depth_reductions.saturating_sub(before.depth_reductions),
            depth_skips_without_reduction: after
                .depth_skips_without_reduction
                .saturating_sub(before.depth_skips_without_reduction),
            vsync_shift: VsyncShiftDelta::between(&before.vsync_shift, &after.vsync_shift),
        }
    }
}

/// Отношение «a к b» с защитой от нуля.
pub fn ratio(numerator: f64, denominator: f64) -> Option<f64> {
    (denominator > 0.0 && numerator.is_finite()).then(|| numerator / denominator)
}

/// Точка отсчёта окна замера: накопительные счётчики на его начало.
#[derive(Debug, Clone)]
pub struct WindowBaseline {
    at: Instant,
    camera_frames_presented: u64,
    /// Повторов кадра на экране (`camera_frame_redraws` рендера) на начало окна.
    camera_frame_redraws: u64,
    isp: ViewfinderCounters,
    present_target: PresentTargetDiagnostics,
    stream: RawStreamCounters,
}

impl WindowBaseline {
    pub fn capture(at: Instant, presentation: &CameraPresentation, stream: RawStreamCounters) -> Self {
        Self {
            at,
            camera_frames_presented: presentation.diagnostics.camera_frames_presented,
            camera_frame_redraws: presentation.diagnostics.camera_frame_redraws,
            isp: presentation
                .viewfinder
                .as_ref()
                .map(|viewfinder| viewfinder.counters.clone())
                .unwrap_or_default(),
            present_target: presentation.present_target.clone(),
            stream,
        }
    }

    /// Подача кадров видоискателя с начала окна до `now` (определения — `viewfinder_pacing_report`).
    pub fn pacing_since(
        &self,
        now: Instant,
        presentation: &CameraPresentation,
        stream: &RawStreamCounters,
    ) -> PacingWindowReport {
        let window_ms = u64::try_from(now.saturating_duration_since(self.at).as_millis()).unwrap_or(u64::MAX);
        let repeats = presentation
            .diagnostics
            .camera_frame_redraws
            .saturating_sub(self.camera_frame_redraws);
        PacingWindowReport::of_window(&stream_delta(&self.stream, stream), repeats, window_ms)
    }
}

/// Что изменилось за окно.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize)]
pub struct WindowDelta {
    pub window_ms: u64,
    /// Новых кадров камеры, поставленных на экран.
    pub camera_frames_presented: u64,
    pub isp: IspCountersDelta,
    pub present_target: PresentTargetDelta,
    /// CPU-копии кадров в потоке RAW (лизинг) за окно.
    pub stream_cpu_copies: u64,
}

impl WindowDelta {
    pub fn since(
        baseline: &WindowBaseline,
        now: Instant,
        presentation: &CameraPresentation,
        stream: &RawStreamCounters,
    ) -> Self {
        let isp_now = presentation
            .viewfinder
            .as_ref()
            .map(|viewfinder| viewfinder.counters.clone())
            .unwrap_or_default();
        Self {
            window_ms: u64::try_from(now.saturating_duration_since(baseline.at).as_millis()).unwrap_or(u64::MAX),
            camera_frames_presented: presentation
                .diagnostics
                .camera_frames_presented
                .saturating_sub(baseline.camera_frames_presented),
            isp: IspCountersDelta::between(&baseline.isp, &isp_now),
            present_target: PresentTargetDelta::between(&baseline.present_target, &presentation.present_target),
            stream_cpu_copies: stream.cpu_copies.saturating_sub(baseline.stream.cpu_copies),
        }
    }

    /// Кадров через ISP в секунду; `None` — окно пустое.
    pub fn isp_frames_per_second(&self) -> Option<f64> {
        ratio(self.isp.frames_processed as f64 * 1_000.0, self.window_ms as f64)
    }

    /// Кадров камеры, которые рендер принял и учёл (ISP + кадры брекетинга, возвращённые без ISP), в секунду:
    /// именно её сравнивают с частотой сенсора, чтобы брекетинг не выглядел потерей кадров.
    pub fn accounted_frames_per_second(&self) -> Option<f64> {
        let accounted = self.isp.frames_processed + self.isp.bracket_frames_skipped;
        ratio(accounted as f64 * 1_000.0, self.window_ms as f64)
    }

    /// Кадров, показанных на экране ровно один раз, по учёту ISP: обработанные минус скрытые на холодном старте.
    pub fn isp_frames_shown(&self) -> u64 {
        self.isp
            .frames_processed
            .saturating_sub(self.isp.frames_hidden_cold_start)
    }
}

/// Складывает задержки «сенсор → экран» из снимка показа (часы читаются здесь же; сбой часов
/// записывается в выборку, а не теряется).
pub fn collect_latency(
    sampler: &mut LatencySampler,
    raw_stream: &RawStreamServices,
    presentation: &CameraPresentation,
) {
    match raw_stream.sample_clock_offset() {
        Ok(offset) => sampler.collect(
            &presentation.diagnostics.present_history,
            &presentation.recent_presentations,
            offset,
        ),
        Err(error) => sampler.note_clock_failure(&error),
    }
}

#[cfg(test)]
#[path = "viewfinder_metrics_tests.rs"]
mod tests;

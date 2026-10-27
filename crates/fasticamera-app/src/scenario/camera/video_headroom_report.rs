//! Отчёт сценария `video-headroom-bench` (V4, сессия 07): сколько GPU остаётся под видео-ISP.
//!
//! Числа — для плана `video`, в продукт стенд не встраивается. Поэтому «не уложился в 16,7 мс» и
//! «экран не дал 120 Гц» — НЕ провал, а честные цифры в отчёте (`fits_frame_budget`,
//! `display.refresh_hz`). Провал ([`evaluate_headroom`]) — только негодный прогон: отказ
//! камеры/рендера, стенд не включился или не записал ни одного кадра, отказы стенда, нет замеров
//! времени, прогон не дошёл до конца, регулировка не дошла до рендера.

use render_loop::viewfinder::{HeadroomBench, HeadroomBenchDiagnostics};
use serde::Serialize;
use serde_json::Value;

use super::detail::to_value;
use super::engine_watch::RunFailure;
use super::result_budget::bounded_list;
use super::viewfinder_metrics::{GpuTimeMs, millis, round2};
use super::viewfinder_session::SessionSummary;
use crate::power_measurement::EnduranceReport;
use shell_contract::commands::FrameRateOutcome;

/// Время одного прохода стенда.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct PassTimeMs {
    pub pass: &'static str,
    pub gpu_time: GpuTimeMs,
}

/// Стенд на конец прогона.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BenchSnapshot {
    pub enabled: bool,
    pub workload_extent: Option<[u32; 2]>,
    /// Кадров записано стендом за прогон.
    pub frames_recorded: u64,
    pub sink_busy_skips: u64,
    pub frames_skipped_no_params: u64,
    pub failures: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_failure: Option<String>,
    pub timing_failures: u64,
    pub passes: Vec<PassTimeMs>,
    pub total: Option<GpuTimeMs>,
}

impl BenchSnapshot {
    /// Снимок с приращением записанных кадров от `frames_at_start`.
    pub fn of(bench: &HeadroomBenchDiagnostics, frames_at_start: u64) -> Self {
        Self {
            enabled: bench.mode == HeadroomBench::Enabled,
            workload_extent: bench.workload_extent.map(|extent| [extent.width, extent.height]),
            frames_recorded: bench.frames_recorded.saturating_sub(frames_at_start),
            sink_busy_skips: bench.sink_busy_skips,
            frames_skipped_no_params: bench.frames_skipped_no_params,
            failures: bench.failures,
            last_failure: bench.last_failure.clone(),
            timing_failures: bench.timing_failures,
            passes: bench
                .pass_times
                .iter()
                .map(|pass| PassTimeMs {
                    pass: pass.pass,
                    gpu_time: GpuTimeMs::of(&pass.gpu_time),
                })
                .collect(),
            total: bench.total.as_ref().map(GpuTimeMs::of),
        }
    }
}

impl BenchSnapshot {
    /// Снимок для строки результата: время проходов — только p50/p95 (число замеров и максимум —
    /// в подробностях), причина отказа укорачивается общим правилом бюджета (`result_budget`).
    pub fn summary(&self) -> Value {
        serde_json::json!({
            "enabled": self.enabled,
            "workload_extent": self.workload_extent,
            "frames_recorded": self.frames_recorded,
            "sink_busy_skips": self.sink_busy_skips,
            "frames_skipped_no_params": self.frames_skipped_no_params,
            "failures": self.failures,
            "last_failure": self.last_failure,
            "timing_failures": self.timing_failures,
            "passes": self
                .passes
                .iter()
                .map(|pass| serde_json::json!({
                    "pass": pass.pass, "p50_ms": pass.gpu_time.p50_ms, "p95_ms": pass.gpu_time.p95_ms,
                }))
                .collect::<Vec<_>>(),
            "total": self.total,
        })
    }
}

/// Экран во время стенда.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct DisplayDuringBench {
    pub requested_hz: f32,
    /// Ответ платформы на просьбу о частоте.
    pub request: Option<FrameRateOutcome>,
    /// Частота по периоду обновления от драйвера (`VK_GOOGLE_display_timing`); `None` — неизвестна.
    pub refresh_hz: Option<f64>,
    /// Ритм показа (окно пейсинга рендера; оно судит относительно цели рендера, а не 120 Гц).
    pub pacing_miss_ratio: f64,
    pub pacing_intervals: u64,
}

impl DisplayDuringBench {
    pub fn refresh_hz_of(cycle: Option<std::time::Duration>) -> Option<f64> {
        cycle
            .map(millis)
            .filter(|period_ms| *period_ms > 0.0)
            .map(|period_ms| round2(1_000.0 / period_ms))
    }
}

/// Минута стенда (в дополнение к минуте тока и тепла).
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct BenchMinute {
    pub minute: u32,
    pub total: Option<GpuTimeMs>,
    pub refresh_hz: Option<f64>,
}

/// Наблюдения прогона.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct HeadroomObservation {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure: Option<RunFailure>,
    pub session: SessionSummary,
    pub sensor_fps: Option<f64>,
    pub frame_budget_ms: f64,
    pub bench: Option<BenchSnapshot>,
    /// p95 всего кадра стенда укладывается в бюджет кадра сенсора (справка, не правило).
    pub fits_frame_budget: Option<bool>,
    pub display: DisplayDuringBench,
    pub expected_minutes: u32,
    pub minutes: Vec<BenchMinute>,
    pub endurance: EnduranceReport,
    pub undelivered: Vec<String>,
}

/// Укладывается ли стенд в бюджет кадра (по p95 всего кадра стенда).
pub fn fits_frame_budget(bench: Option<&BenchSnapshot>, frame_budget_ms: f64) -> Option<bool> {
    bench
        .and_then(|bench| bench.total)
        .map(|total| total.p95_ms <= frame_budget_ms)
}

/// Годен ли прогон.
pub fn evaluate_headroom(observation: &HeadroomObservation) -> Vec<String> {
    if let Some(failure) = &observation.failure {
        return vec![failure.describe()];
    }
    let mut violations: Vec<String> = observation
        .undelivered
        .iter()
        .map(|reason| format!("регулировка рендера не доставлена: {reason}"))
        .collect();
    match &observation.bench {
        None => violations.push("в статусе рендера нет диагностики видоискателя и стенда".to_owned()),
        Some(bench) => {
            if !bench.enabled {
                violations.push("стенд к концу прогона не включён".to_owned());
            }
            if bench.frames_recorded == 0 {
                violations.push("стенд не записал ни одного кадра".to_owned());
            }
            if bench.failures != 0 {
                violations.push(format!(
                    "отказов стенда {}: {}",
                    bench.failures,
                    bench.last_failure.as_deref().unwrap_or("причина не записана")
                ));
            }
            if bench.total.is_none() {
                violations.push("нет замеров времени кадра стенда".to_owned());
            }
        }
    }
    let minutes = u32::try_from(observation.endurance.minutes.len()).unwrap_or(u32::MAX);
    if minutes < observation.expected_minutes {
        violations.push(format!("записано минут {minutes} из {}", observation.expected_minutes));
    }
    violations
}

impl HeadroomObservation {
    pub fn summary(&self) -> Value {
        serde_json::json!({
            "failure": self.failure,
            "configured": self.session.configured,
            "state_at_end": self.session.state_at_end,
            "sensor_fps": self.sensor_fps.map(round2),
            "frame_budget_ms": round2(self.frame_budget_ms),
            "fits_frame_budget": self.fits_frame_budget,
            "bench": self.bench.as_ref().map(BenchSnapshot::summary),
            "display": self.display,
            "expected_minutes": self.expected_minutes,
            "thermal": {
                "worst": self.endurance.totals.worst_thermal_status,
                "peak_headroom": self.endurance.totals.peak_headroom,
                "mean_discharge_ma": self.endurance.totals.mean_discharge_ma,
            },
            "undelivered": bounded_list(&self.undelivered),
        })
    }

    pub fn detail_items(&self) -> Vec<Value> {
        let mut items: Vec<Value> = self.minutes.iter().map(to_value).collect();
        items.extend(self.endurance.detail_items());
        // В сводке снимок стенда сжат (бюджет строки результата) — целиком он здесь, включая
        // полную причину последнего отказа.
        if let Some(bench) = &self.bench {
            items.push(serde_json::json!({ "bench": to_value(bench) }));
            if let Some(last_failure) = &bench.last_failure {
                items.push(serde_json::json!({ "bench_last_failure": last_failure }));
            }
        }
        items.push(serde_json::json!({ "session": to_value(&self.session) }));
        items
    }
}

#[cfg(test)]
#[path = "video_headroom_report_tests.rs"]
mod tests;

//! Вид отчёта `ae-trace`: сводка для строки результата и подробности для строк `detail_ae_trace`.
//!
//! Форматирование отдельно от разбора (`ae_trace_analysis`) и сбора (`ae_trace.rs`): здесь только раскладка чисел
//! по JSON и округление.
//!
//! Строка результата — одна запись logcat в 4000 байт, поэтому в ней сводка, а ряд ушёл в подробности:
//! * ступеньки идут КОРОТКИМИ СТРОКАМИ-МАССИВАМИ в порядке `step_cols` (объект на ступеньку не влез бы вместе с
//!   текстом нарушений, до 700 байт);
//! * числа округлены до тысячных (EV) и десятых (мс/с).
//!
//! Подробности: первая запись — легенда (колонки, коды сходимости и мерцания, моменты фаз и подсказок), дальше
//! строки ряда `[t_ms, observed_ev, target_ev, sensor_ev, gain_ev, convergence, flicker]` с шагом
//! `detail_decimation_ms`.

use serde_json::{Value, json};

use super::ae_trace_analysis::{PhaseAnalysis, RunAnalysis};
use super::ae_trace_observation::AeTraceObservation;
use super::ae_trace_plan::Timeline;
use super::ae_trace_series::{FlickerReading, TraceSample, convergence_code};
use super::ae_trace_stats::{FlickerStats, QuietStats};
use super::ae_trace_steps::StepResult;
use super::result_budget::bounded_list;
use crate::ae_trace_scenario_config::AeTraceScenarioConfig;

/// Колонки строки ступеньки в сводке. Контракт с `scripts/smoke_checks_ae_trace.py`.
pub const STEP_COLUMNS: [&str; 12] = [
    "cue",
    "kind",
    "step_ev",
    "onset_ms",
    "sensor_settle_ms",
    "screen_settle_ms",
    "overshoot_ev",
    "sensor_overshoot_ev",
    "max_abs_gain_ev",
    "gain_at_end_ev",
    "proposals",
    "generations",
];

/// Колонки строки ряда в подробностях.
const SERIES_COLUMNS: [&str; 7] = [
    "t_ms",
    "observed_ev",
    "target_ev",
    "sensor_ev",
    "gain_ev",
    "convergence",
    "flicker",
];

/// Округление до тысячных: EV с точностью глаза, строка короче.
fn round3(value: f64) -> f64 {
    (value * 1_000.0).round() / 1_000.0
}

/// Округление до десятых: мс и доли в секунду.
fn round1(value: f64) -> f64 {
    (value * 10.0).round() / 10.0
}

/// Значение, которого может не быть (нет данных — `null`, не ноль).
fn opt3(value: Option<f64>) -> Value {
    value.map_or(Value::Null, |number| json!(round3(number)))
}

/// Сводка для строки результата.
pub fn summary(observation: &AeTraceObservation) -> Value {
    let mut report = json!({
        "real_steps": observation.analysis.as_ref().map_or(0, RunAnalysis::measured_step_count),
        "samples": observation.series.len(),
        "dropped_samples": observation.dropped_samples,
        "ticks_without_ae": observation.ticks_without_ae,
        "time_to_streaming_ms": observation.timings.time_to_streaming_ms,
        "time_to_live_ms": observation.timings.time_to_live_ms,
        "haptics": {
            "queued": observation.haptics.queued,
            "failed": bounded_list(&observation.haptics.failures),
        },
        "gain_limit_ev": round3(observation.gain_limit_ev),
        "restore_failure": observation.restore_failure,
        "step_cols": STEP_COLUMNS,
    });
    if let (Some(analysis), Some(fields)) = (&observation.analysis, report.as_object_mut()) {
        fields.insert("max_abs_gain_ev".to_owned(), json!(round3(analysis.max_abs_gain_ev)));
        fields.insert(
            "phases".to_owned(),
            Value::Array(analysis.phases.iter().map(phase_summary).collect()),
        );
    }
    report
}

fn phase_summary(phase: &PhaseAnalysis) -> Value {
    let mut entry = json!({
        "phase": phase.phase.name(),
        "profile": format!("{:?}", phase.profile).to_lowercase(),
        "profile_confirmed": phase.profile_confirmed,
        "samples": phase.samples,
        "max_abs_gain_ev": round3(phase.max_abs_gain_ev),
        "flicker": flicker_summary(&phase.flicker),
    });
    let Some(fields) = entry.as_object_mut() else {
        return entry;
    };
    if phase.phase.has_cues() {
        fields.insert(
            "steps".to_owned(),
            Value::Array(phase.steps.iter().map(step_row).collect()),
        );
        fields.insert(
            "median_screen_settle_ms".to_owned(),
            phase
                .median_screen_settle_ms()
                .map_or(Value::Null, |ms| json!(round1(ms))),
        );
        fields.insert(
            "worst_overshoot_ev".to_owned(),
            json!(round3(phase.worst_overshoot_ev())),
        );
    }
    if let Some(quiet) = &phase.quiet {
        fields.insert("quiet".to_owned(), quiet_summary(quiet));
    }
    entry
}

/// Строка ступеньки в порядке `STEP_COLUMNS`; у непройденных ступенек метрик нет — `null`.
fn step_row(step: &StepResult) -> Value {
    let metrics = step.metrics.as_ref();
    json!([
        step.cue_index + 1,
        step.kind.name(),
        opt3(step.step_ev),
        metrics.and_then(|m| m.onset_ms),
        metrics.and_then(|m| m.sensor_settle_ms),
        metrics.and_then(|m| m.screen_settle_ms),
        opt3(metrics.map(|m| m.overshoot_ev)),
        opt3(metrics.map(|m| m.sensor_overshoot_ev)),
        opt3(metrics.map(|m| m.max_abs_gain_ev)),
        opt3(metrics.and_then(|m| m.gain_at_end_ev)),
        metrics.map(|m| m.proposals_published),
        metrics.and_then(|m| m.directive_generations),
    ])
}

fn flicker_summary(flicker: &FlickerStats) -> Value {
    json!({
        "readings": flicker.readings,
        "long_frames": flicker.long_exposure_frames,
        "multiple_frames": flicker.multiple_of_period_frames,
        "multiple_share": opt3(flicker.multiple_share()),
        "max_exposure_ms": flicker.max_exposure_ms.map(round1),
    })
}

fn quiet_summary(quiet: &QuietStats) -> Value {
    json!({
        "analyzed_ms": quiet.analyzed_ms,
        "target_range_ev": opt3(quiet.target_range_ev),
        "sensor_range_ev": opt3(quiet.sensor_range_ev),
        "screen_range_ev": opt3(quiet.screen_range_ev),
        "proposals": quiet.proposals_published,
        "generations": quiet.directive_generations,
        "proposals_per_s": round1(quiet.proposals_per_s),
        "generations_per_s": quiet.generations_per_s.map(round1),
    })
}

/// Элементы подробностей: легенда и прореженный ряд.
pub fn detail_items(config: &AeTraceScenarioConfig, observation: &AeTraceObservation) -> Vec<Value> {
    let mut items = vec![legend(config, observation.timeline.as_ref())];
    items.extend(
        decimate(&observation.series, config.detail_decimation_ms)
            .into_iter()
            .map(series_row),
    );
    items
}

fn legend(config: &AeTraceScenarioConfig, timeline: Option<&Timeline>) -> Value {
    let phases: Vec<Value> = timeline.map_or_else(Vec::new, |timeline| {
        timeline
            .phases()
            .iter()
            .map(|window| {
                json!({
                    "phase": window.phase.name(),
                    "start_ms": window.start_ms,
                    "end_ms": window.end_ms,
                    "cues_ms": window.cues_ms,
                })
            })
            .collect()
    });
    json!({
        "legend": {
            "columns": SERIES_COLUMNS,
            "unit": "EV = log2 от «выдержка × ISO» в с·ISO; экран = sensor_ev + gain_ev",
            "decimation_ms": config.detail_decimation_ms,
            "convergence": {"0": "нет предложений", "1": "searching", "2": "settled", "3": "holding"},
            "flicker": flicker_codes(),
            "phases": phases,
        }
    })
}

/// Расшифровка кодов ответа детектора для легенды: `{"0": "absent", ...}`.
fn flicker_codes() -> Value {
    let codes: serde_json::Map<String, Value> = FlickerReading::ALL
        .iter()
        .map(|reading| (reading.code().to_string(), json!(reading.name())))
        .collect();
    Value::Object(codes)
}

/// Первый отсчёт каждого интервала `step_ms`: ряд сводится к ровной сетке, а не к каждому тику.
pub fn decimate(samples: &[TraceSample], step_ms: u64) -> Vec<&TraceSample> {
    let mut next_ms = 0_u64;
    let mut kept = Vec::new();
    for sample in samples {
        if sample.t_ms >= next_ms {
            kept.push(sample);
            next_ms = (sample.t_ms / step_ms + 1) * step_ms;
        }
    }
    kept
}

fn series_row(sample: &TraceSample) -> Value {
    json!([
        sample.t_ms,
        opt3(sample.observed_ev),
        opt3(sample.target_ev),
        opt3(sample.sensor_ev),
        round3(sample.gain_ev),
        convergence_code(sample.convergence),
        sample
            .latest_flicker
            .map_or(Value::Null, |reading| json!(reading.code())),
    ])
}

#[cfg(test)]
#[path = "ae_trace_report_tests.rs"]
mod tests;

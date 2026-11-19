//! Малые чистые расчёты над рядом `ae-trace`: окна по времени, медиана, размах, мерцание, «дыхание» статики.

use std::collections::BTreeMap;

use super::ae_trace_series::{FlickerReading, TraceSample};
use crate::ae_trace_scenario_config::AeTraceScenarioConfig;

/// Отсчёты ряда с `from_ms` включительно до `to_ms` не включительно. Ряд упорядочен по времени.
pub fn window(samples: &[TraceSample], from_ms: u64, to_ms: u64) -> &[TraceSample] {
    let start = samples.partition_point(|sample| sample.t_ms < from_ms);
    let end = samples.partition_point(|sample| sample.t_ms < to_ms);
    samples.get(start..end.max(start)).unwrap_or(&[])
}

/// Медиана (пустой набор — `None`). Входы конечные: нечисла в ряд не попадают.
pub fn median(values: &[f64]) -> Option<f64> {
    if values.is_empty() {
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

/// Размах «максимум минус минимум» (пустой набор — `None`).
pub fn spread(values: &[f64]) -> Option<f64> {
    let max = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let min = values.iter().copied().fold(f64::INFINITY, f64::min);
    (!values.is_empty()).then_some(max - min)
}

/// Приращение накопительного счётчика на участке: последний отсчёт минус первый (меньше двух отсчётов — 0).
pub fn counter_delta(samples: &[TraceSample]) -> u64 {
    match (samples.first(), samples.last()) {
        (Some(first), Some(last)) => last.proposals_published.saturating_sub(first.proposals_published),
        _ => 0,
    }
}

/// На сколько выросло поколение указания на участке (`None` — поколения не было ни в одном отсчёте).
pub fn generation_delta(samples: &[TraceSample]) -> Option<u64> {
    let first = samples.iter().find_map(|sample| sample.directive_generation)?;
    let last = samples.iter().rev().find_map(|sample| sample.directive_generation)?;
    Some(last.saturating_sub(first))
}

/// Мерцание фазы: ответы детектора и кратность выдержки периоду.
#[derive(Debug, Clone, PartialEq)]
pub struct FlickerStats {
    /// Сколько кадров повторяющегося запроса дало каждый ответ детектора; нулевые ответы присутствуют.
    pub readings: BTreeMap<&'static str, u64>,
    /// Кадры с выдержкой не короче `flicker_min_exposure_ms`.
    pub long_exposure_frames: u64,
    /// Из них выдержка, кратная периоду в пределах допуска.
    pub multiple_of_period_frames: u64,
    /// Наибольшая выдержка, мс.
    pub max_exposure_ms: Option<f64>,
}

impl FlickerStats {
    /// Доля кратных периоду среди длинных кадров (`None` — длинных кадров не было).
    pub fn multiple_share(&self) -> Option<f64> {
        (self.long_exposure_frames > 0)
            .then(|| self.multiple_of_period_frames as f64 / self.long_exposure_frames as f64)
    }
}

/// Кратна ли выдержка периоду мерцания в пределах допуска (квантование по строке сенсора).
pub fn is_multiple_of_period(exposure_ms: f64, config: &AeTraceScenarioConfig) -> bool {
    let remainder = exposure_ms % config.flicker_period_ms;
    remainder.min(config.flicker_period_ms - remainder) <= config.flicker_tolerance_ms
}

/// Считает мерцание по новым кадрам отсчётов (каждый кадр — один раз, дубли убрал сборщик отсчётов).
pub fn flicker_stats(config: &AeTraceScenarioConfig, samples: &[TraceSample]) -> FlickerStats {
    let mut readings: BTreeMap<&'static str, u64> =
        FlickerReading::ALL.iter().map(|reading| (reading.name(), 0)).collect();
    let mut long_exposure_frames = 0_u64;
    let mut multiple_of_period_frames = 0_u64;
    let mut max_exposure_ms: Option<f64> = None;
    for frame in samples.iter().filter_map(|sample| sample.new_frame) {
        *readings.entry(frame.flicker.name()).or_insert(0) += 1;
        let Some(exposure_ms) = frame.exposure_ms else {
            continue;
        };
        max_exposure_ms = Some(max_exposure_ms.map_or(exposure_ms, |max| max.max(exposure_ms)));
        if exposure_ms >= config.flicker_min_exposure_ms {
            long_exposure_frames += 1;
            if is_multiple_of_period(exposure_ms, config) {
                multiple_of_period_frames += 1;
            }
        }
    }
    FlickerStats {
        readings,
        long_exposure_frames,
        multiple_of_period_frames,
        max_exposure_ms,
    }
}

/// «Дыхание» неподвижной сцены: насколько ходят уровни и как часто перестраивается запрос.
#[derive(Debug, Clone, PartialEq)]
pub struct QuietStats {
    /// Сколько мс разбиралось (без разогрева).
    pub analyzed_ms: u64,
    pub target_range_ev: Option<f64>,
    pub sensor_range_ev: Option<f64>,
    pub screen_range_ev: Option<f64>,
    pub proposals_published: u64,
    /// Рост поколения указания ≈ число перестроек повторяющегося запроса.
    pub directive_generations: Option<u64>,
    pub proposals_per_s: f64,
    pub generations_per_s: Option<f64>,
}

/// Считает «дыхание» по отсчётам участка (участок уже обрезан от разогрева).
pub fn quiet_stats(samples: &[TraceSample], analyzed_ms: u64) -> QuietStats {
    let targets: Vec<f64> = samples.iter().filter_map(|sample| sample.target_ev).collect();
    let sensors: Vec<f64> = samples.iter().filter_map(|sample| sample.sensor_ev).collect();
    let screens: Vec<f64> = samples.iter().filter_map(TraceSample::screen_ev).collect();
    let proposals_published = counter_delta(samples);
    let directive_generations = generation_delta(samples);
    let seconds = analyzed_ms as f64 / 1_000.0;
    QuietStats {
        analyzed_ms,
        target_range_ev: spread(&targets),
        sensor_range_ev: spread(&sensors),
        screen_range_ev: spread(&screens),
        proposals_published,
        directive_generations,
        proposals_per_s: if seconds > 0.0 {
            proposals_published as f64 / seconds
        } else {
            0.0
        },
        generations_per_s: directive_generations
            .filter(|_| seconds > 0.0)
            .map(|count| count as f64 / seconds),
    }
}

#[cfg(test)]
#[path = "ae_trace_stats_tests.rs"]
mod tests;

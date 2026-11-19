//! Разбор одной ступеньки света: подсказка владельца -> отрезок до следующей подсказки или конца фазы.
//!
//! Чистые функции над рядом отсчётов. Ступенька настоящая, если уровень сцены по сырому замеру в конце отрезка
//! отличается от уровня до подсказки не меньше `min_step_ev`; иначе владелец подсказку пропустил (`NoStep`) и
//! отрезок в вердикт не идёт. Для настоящей ступеньки считаются:
//! * время установки сенсора и экрана (`*_settle_ms`) — от подсказки до ПОСЛЕДНЕГО выхода из полосы
//!   `settle_tolerance_ev` вокруг конечного значения (первый заход в полосу не считается: ряд может уйти обратно);
//! * перерегулирование — наибольший заход за конечное значение в сторону ступеньки;
//! * `onset_ms` — когда замер сцены покинул полосу базового уровня: время реакции владельца, которое входит в
//!   время установки от подсказки и в десятки раз больше постоянной времени профиля.

use super::ae_trace_series::TraceSample;
use super::ae_trace_stats::{counter_delta, generation_delta, median, window};
use crate::ae_trace_scenario_config::AeTraceScenarioConfig;

/// Чем кончился разбор отрезка.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepKind {
    /// Настоящая ступенька: метрики посчитаны.
    Measured,
    /// Уровень сцены не изменился — владелец подсказку пропустил. В вердикт не идёт.
    NoStep,
    /// В окне до подсказки или в конце отрезка мало замеров: судить нечем. В вердикт не идёт.
    InsufficientData,
}

impl StepKind {
    pub fn name(self) -> &'static str {
        match self {
            StepKind::Measured => "measured",
            StepKind::NoStep => "no_step",
            StepKind::InsufficientData => "insufficient_data",
        }
    }
}

/// Метрики настоящей ступеньки.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StepMetrics {
    /// Когда замер сцены вышел из полосы базового уровня, мс от подсказки (реакция владельца).
    pub onset_ms: Option<u64>,
    /// Через сколько мс после подсказки уровень сенсора остался в полосе до конца отрезка (`None` — не установился).
    pub sensor_settle_ms: Option<u64>,
    /// То же для уровня на экране (сенсор плюс подтяжка ISP).
    pub screen_settle_ms: Option<u64>,
    /// Перерегулирование уровня на экране, EV, не меньше нуля.
    pub overshoot_ev: f64,
    /// То же для сенсора без подтяжки.
    pub sensor_overshoot_ev: f64,
    /// Наибольшая по модулю подтяжка ISP на отрезке, EV.
    pub max_abs_gain_ev: f64,
    /// Подтяжка в конце отрезка (медиана последнего окна), EV: после схождения должна вернуться к 0.
    pub gain_at_end_ev: Option<f64>,
    /// Предложений опубликовано на отрезке.
    pub proposals_published: u64,
    /// Поколений указания движка на отрезке (≈ перестроек запроса).
    pub directive_generations: Option<u64>,
}

/// Итог разбора одной подсказки.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StepResult {
    /// Номер подсказки внутри фазы с нуля.
    pub cue_index: usize,
    /// Момент подсказки, мс от начала расписания.
    pub cue_ms: u64,
    pub kind: StepKind,
    /// Ступенька по сырому замеру сцены, EV (`None` — мало замеров).
    pub step_ev: Option<f64>,
    /// Метрики; `Some` только у `Measured`.
    pub metrics: Option<StepMetrics>,
}

/// Отрезок, который разбирается: подсказка и конец (следующая подсказка или конец фазы).
#[derive(Debug, Clone, Copy)]
pub struct Segment {
    pub cue_index: usize,
    pub cue_ms: u64,
    pub end_ms: u64,
}

/// Разбирает отрезок `segment` по всему ряду `samples`.
pub fn analyze_step(config: &AeTraceScenarioConfig, samples: &[TraceSample], segment: Segment) -> StepResult {
    let unjudged = |kind, step_ev| StepResult {
        cue_index: segment.cue_index,
        cue_ms: segment.cue_ms,
        kind,
        step_ev,
        metrics: None,
    };
    let window_ms = config.step_window_ms;
    let before = window(samples, segment.cue_ms.saturating_sub(window_ms), segment.cue_ms);
    let tail = window(samples, segment.end_ms.saturating_sub(window_ms), segment.end_ms);
    let baseline = enough(config, before.iter().filter_map(|sample| sample.observed_ev).collect());
    let final_level = enough(config, tail.iter().filter_map(|sample| sample.observed_ev).collect());
    let (Some(baseline), Some(final_level)) = (baseline, final_level) else {
        return unjudged(StepKind::InsufficientData, None);
    };
    let step_ev = final_level - baseline;
    if step_ev.abs() < config.min_step_ev {
        return unjudged(StepKind::NoStep, Some(step_ev));
    }
    let metrics = measure(config, samples, segment, baseline, step_ev.signum());
    StepResult {
        cue_index: segment.cue_index,
        cue_ms: segment.cue_ms,
        kind: StepKind::Measured,
        step_ev: Some(step_ev),
        metrics: Some(metrics),
    }
}

/// Медиана окна, если в нём хватает замеров.
fn enough(config: &AeTraceScenarioConfig, values: Vec<f64>) -> Option<f64> {
    (values.len() as u64 >= config.min_window_samples)
        .then(|| median(&values))
        .flatten()
}

/// Метрики настоящей ступеньки; `direction` — знак ступеньки (+1 — сцена темнее, нужно больше света).
fn measure(
    config: &AeTraceScenarioConfig,
    samples: &[TraceSample],
    segment: Segment,
    baseline_ev: f64,
    direction: f64,
) -> StepMetrics {
    let in_segment = window(samples, segment.cue_ms, segment.end_ms);
    let tail = window(
        samples,
        segment.end_ms.saturating_sub(config.step_window_ms),
        segment.end_ms,
    );
    let sensor = level_series(in_segment, |sample| sample.sensor_ev);
    let screen = level_series(in_segment, TraceSample::screen_ev);
    let sensor_final = final_level(tail, |sample| sample.sensor_ev);
    let screen_final = final_level(tail, TraceSample::screen_ev);
    let gains: Vec<f64> = tail.iter().map(|sample| sample.gain_ev).collect();
    let tolerance = config.settle_tolerance_ev;
    StepMetrics {
        onset_ms: onset_ms(in_segment, segment.cue_ms, baseline_ev, tolerance),
        sensor_settle_ms: sensor_final.and_then(|level| settle_ms(&sensor, segment.cue_ms, level, tolerance)),
        screen_settle_ms: screen_final.and_then(|level| settle_ms(&screen, segment.cue_ms, level, tolerance)),
        overshoot_ev: screen_final.map_or(0.0, |level| overshoot_ev(&screen, level, direction)),
        sensor_overshoot_ev: sensor_final.map_or(0.0, |level| overshoot_ev(&sensor, level, direction)),
        max_abs_gain_ev: in_segment.iter().map(|sample| sample.gain_ev.abs()).fold(0.0, f64::max),
        gain_at_end_ev: median(&gains),
        proposals_published: counter_delta(in_segment),
        directive_generations: generation_delta(in_segment),
    }
}

/// Пары «время, уровень» для отсчётов, у которых уровень известен.
fn level_series(samples: &[TraceSample], level: impl Fn(&TraceSample) -> Option<f64>) -> Vec<(u64, f64)> {
    samples
        .iter()
        .filter_map(|sample| level(sample).map(|value| (sample.t_ms, value)))
        .collect()
}

/// Конечное значение отрезка: медиана уровня в последнем окне.
fn final_level(tail: &[TraceSample], level: impl Fn(&TraceSample) -> Option<f64>) -> Option<f64> {
    let values: Vec<f64> = tail.iter().filter_map(level).collect();
    median(&values)
}

/// Через сколько мс после подсказки ряд остаётся в полосе `tolerance` вокруг `final_level` до конца отрезка.
/// `None` — последний отсчёт отрезка ещё вне полосы (не установился) или отсчётов нет.
pub fn settle_ms(series: &[(u64, f64)], cue_ms: u64, final_level: f64, tolerance: f64) -> Option<u64> {
    match series
        .iter()
        .rposition(|(_, value)| (value - final_level).abs() > tolerance)
    {
        // Ряд всё время в полосе: ступенька для него ничего не изменила.
        None => series.first().map(|(t_ms, _)| t_ms.saturating_sub(cue_ms)),
        Some(last_outside) => series
            .get(last_outside + 1)
            .map(|(t_ms, _)| t_ms.saturating_sub(cue_ms)),
    }
}

/// Наибольший заход ряда за конечное значение в сторону ступеньки, EV (≥ 0).
pub fn overshoot_ev(series: &[(u64, f64)], final_level: f64, direction: f64) -> f64 {
    series
        .iter()
        .map(|(_, value)| direction * (value - final_level))
        .fold(0.0, f64::max)
}

/// Когда замер сцены впервые вышел из полосы вокруг базового уровня, мс от подсказки.
fn onset_ms(in_segment: &[TraceSample], cue_ms: u64, baseline_ev: f64, tolerance: f64) -> Option<u64> {
    in_segment
        .iter()
        .find(|sample| {
            sample
                .observed_ev
                .is_some_and(|observed| (observed - baseline_ev).abs() > tolerance)
        })
        .map(|sample| sample.t_ms.saturating_sub(cue_ms))
}

#[cfg(test)]
#[path = "ae_trace_steps_tests.rs"]
mod tests;

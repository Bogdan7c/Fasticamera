//! Разбор прогона `ae-trace` целиком: ряд отсчётов + расписание -> итоги по фазам (чистая функция).
//!
//! Ввода-вывода и форматирования здесь нет: на входе ряд и расписание, на выходе структуры с числами.
//! Ступеньки — `ae_trace_steps`, мерцание и «дыхание» — `ae_trace_stats`, вердикт — `ae_trace_verdict`,
//! вид отчёта — `ae_trace_report`.

use camera_contract::three_a::ThreeAProfile;

use super::ae_trace_plan::{PhaseWindow, Timeline, TracePhase};
use super::ae_trace_series::TraceSample;
use super::ae_trace_stats::{FlickerStats, QuietStats, flicker_stats, median, quiet_stats, window};
use super::ae_trace_steps::{Segment, StepKind, StepResult, analyze_step};
use crate::ae_trace_scenario_config::AeTraceScenarioConfig;

/// Итоги одной фазы.
#[derive(Debug, Clone, PartialEq)]
pub struct PhaseAnalysis {
    pub phase: TracePhase,
    /// Профиль, который фаза обязана держать.
    pub profile: ThreeAProfile,
    /// Движок держал нужный профиль к концу фазы (иначе числа «видео» могут быть числами «фото»).
    pub profile_confirmed: bool,
    pub start_ms: u64,
    pub end_ms: u64,
    /// Сколько отсчётов попало в фазу.
    pub samples: usize,
    /// Разбор каждой подсказки по порядку (в `static` пусто).
    pub steps: Vec<StepResult>,
    pub flicker: FlickerStats,
    /// «Дыхание» неподвижной сцены; только у фазы без подсказок.
    pub quiet: Option<QuietStats>,
    /// Наибольшая по модулю подтяжка ISP в фазе, EV.
    pub max_abs_gain_ev: f64,
}

impl PhaseAnalysis {
    /// Настоящие ступеньки (по ним судит вердикт).
    pub fn measured_steps(&self) -> impl Iterator<Item = (&StepResult, &super::ae_trace_steps::StepMetrics)> {
        self.steps
            .iter()
            .filter_map(|step| step.metrics.as_ref().map(|metrics| (step, metrics)))
    }

    /// Сколько подсказок закончилось указанным исходом.
    pub fn count(&self, kind: StepKind) -> usize {
        self.steps.iter().filter(|step| step.kind == kind).count()
    }

    /// Медиана времени установки экрана по настоящим ступенькам, мс (`None` — таких нет).
    pub fn median_screen_settle_ms(&self) -> Option<f64> {
        let settles: Vec<f64> = self
            .measured_steps()
            .filter_map(|(_, metrics)| metrics.screen_settle_ms.map(|ms| ms as f64))
            .collect();
        median(&settles)
    }

    /// Худшее перерегулирование экрана среди настоящих ступенек, EV.
    pub fn worst_overshoot_ev(&self) -> f64 {
        self.measured_steps()
            .map(|(_, metrics)| metrics.overshoot_ev)
            .fold(0.0, f64::max)
    }
}

/// Итоги прогона.
#[derive(Debug, Clone, PartialEq)]
pub struct RunAnalysis {
    pub phases: Vec<PhaseAnalysis>,
    /// Наибольшая по модулю подтяжка ISP за весь прогон, EV.
    pub max_abs_gain_ev: f64,
    pub samples: usize,
}

impl RunAnalysis {
    /// Итог фазы по имени.
    #[cfg(test)]
    pub fn phase(&self, phase: TracePhase) -> Option<&PhaseAnalysis> {
        self.phases.iter().find(|analysis| analysis.phase == phase)
    }

    /// Сколько настоящих ступенек во всём прогоне.
    pub fn measured_step_count(&self) -> usize {
        self.phases.iter().map(|phase| phase.count(StepKind::Measured)).sum()
    }
}

/// Разбирает ряд по расписанию.
pub fn analyze_run(config: &AeTraceScenarioConfig, timeline: &Timeline, samples: &[TraceSample]) -> RunAnalysis {
    RunAnalysis {
        phases: timeline
            .phases()
            .iter()
            .map(|window| analyze_phase(config, window, samples))
            .collect(),
        max_abs_gain_ev: max_abs_gain(samples),
        samples: samples.len(),
    }
}

fn analyze_phase(config: &AeTraceScenarioConfig, phase_window: &PhaseWindow, samples: &[TraceSample]) -> PhaseAnalysis {
    let in_phase = window(samples, phase_window.start_ms, phase_window.end_ms);
    let phase = phase_window.phase;
    let steps = phase_window
        .cues_ms
        .iter()
        .enumerate()
        .map(|(cue_index, cue_ms)| {
            let segment = Segment {
                cue_index,
                cue_ms: *cue_ms,
                end_ms: phase_window.segment_end_ms(cue_index),
            };
            analyze_step(config, samples, segment)
        })
        .collect();
    let quiet = (!phase.has_cues()).then(|| {
        let from_ms = phase_window.start_ms.saturating_add(config.static_warmup_ms);
        quiet_stats(
            window(samples, from_ms, phase_window.end_ms),
            phase_window.end_ms.saturating_sub(from_ms),
        )
    });
    PhaseAnalysis {
        phase,
        profile: phase.profile(),
        profile_confirmed: in_phase.last().is_some_and(|sample| sample.profile == phase.profile()),
        start_ms: phase_window.start_ms,
        end_ms: phase_window.end_ms,
        samples: in_phase.len(),
        steps,
        flicker: flicker_stats(config, in_phase),
        quiet,
        max_abs_gain_ev: max_abs_gain(in_phase),
    }
}

fn max_abs_gain(samples: &[TraceSample]) -> f64 {
    samples.iter().map(|sample| sample.gain_ev.abs()).fold(0.0, f64::max)
}

#[cfg(test)]
#[path = "ae_trace_analysis_tests.rs"]
mod tests;

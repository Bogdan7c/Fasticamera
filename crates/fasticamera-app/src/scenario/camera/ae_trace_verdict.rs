//! Вердикт сценария `ae-trace`: чистая функция «наблюдения + конфиг -> список нарушений».
//!
//! Судим только жёсткое: поломку стенда, подтяжку ISP выше предела контроллера, перерегулирование на настоящей
//! ступеньке выше порога, непринятые подсказки, не подтверждённый профиль. Время схождения, скорость
//! публикаций, «дыхание», мерцание и сравнение «фото против видео» — данные для владельца, не вердикт.
//! Пропущенные подсказки (`no_step`) и отрезки с малым числом замеров в вердикт не идут.

use super::ae_trace_analysis::RunAnalysis;
use super::ae_trace_observation::AeTraceObservation;
use crate::ae_trace_scenario_config::AeTraceScenarioConfig;

/// Сколько непринятых подсказок перечислять в нарушении: остальные видны числом.
const LISTED_PROMPT_FAILURES: usize = 3;

/// Нарушения прогона (пусто — сценарий прошёл).
pub fn evaluate_ae_trace(config: &AeTraceScenarioConfig, observation: &AeTraceObservation) -> Vec<String> {
    let mut violations = Vec::new();
    if let Some(failure) = &observation.failure {
        violations.push(failure.clone());
    }
    if let Some(failure) = &observation.restore_failure {
        violations.push(format!("профиль 3A не возвращён в «Фото»: {failure}"));
    }
    violations.extend(prompt_violations(observation));
    if observation.dropped_samples > 0 {
        violations.push(format!(
            "ряд отсчётов переполнен: потеряно {} отсчётов (расписание не влезло в series_capacity)",
            observation.dropped_samples
        ));
    }
    if let Some(analysis) = &observation.analysis {
        // Пропущенную подсказку не судим, но прогон без единой настоящей ступеньки света — не приёмка:
        // проверять схождение, перерегулирование и подтяжку было не на чем (владелец не щёлкал лампой
        // или замер сцены не дошёл до сценария).
        if analysis.measured_step_count() == 0 {
            violations.push(
                "ни одной настоящей ступеньки света: схождение, перерегулирование и подтяжка не проверены".to_owned(),
            );
        }
        violations.extend(profile_violations(analysis));
        violations.extend(gain_violations(config, observation));
        violations.extend(overshoot_violations(config, analysis));
    }
    violations
}

fn prompt_violations(observation: &AeTraceObservation) -> Vec<String> {
    let failures = &observation.prompts.failures;
    if failures.is_empty() {
        return Vec::new();
    }
    let listed = failures
        .iter()
        .take(LISTED_PROMPT_FAILURES)
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    vec![format!(
        "владельцу не показан сигнал на экране ({} из {} подсказок): {listed}",
        failures.len(),
        failures.len() + observation.prompts.queued as usize
    )]
}

fn profile_violations(analysis: &RunAnalysis) -> Vec<String> {
    analysis
        .phases
        .iter()
        .filter(|phase| !phase.profile_confirmed)
        .map(|phase| {
            format!(
                "фаза {}: движок не удержал профиль {:?} к концу фазы — числа фазы нельзя приписывать этому профилю",
                phase.phase.name(),
                phase.profile
            )
        })
        .collect()
}

/// Подтяжка ISP выше предела контроллера плюс допуск округления — одно нарушение с худшим значением.
fn gain_violations(config: &AeTraceScenarioConfig, observation: &AeTraceObservation) -> Vec<String> {
    let Some(analysis) = &observation.analysis else {
        return Vec::new();
    };
    let allowed = observation.gain_limit_ev + config.gain_limit_epsilon_ev;
    if analysis.max_abs_gain_ev <= allowed {
        return Vec::new();
    }
    vec![format!(
        "подтяжка ISP {:.3} EV выше предела {:.3} EV (предел контроллера {:.3} + допуск {:.3})",
        analysis.max_abs_gain_ev, allowed, observation.gain_limit_ev, config.gain_limit_epsilon_ev
    )]
}

fn overshoot_violations(config: &AeTraceScenarioConfig, analysis: &RunAnalysis) -> Vec<String> {
    let mut violations = Vec::new();
    for phase in &analysis.phases {
        for (step, metrics) in phase.measured_steps() {
            if metrics.overshoot_ev > config.max_overshoot_ev {
                violations.push(format!(
                    "фаза {}, подсказка {}: перерегулирование {:.3} EV выше порога {:.3} EV",
                    phase.phase.name(),
                    step.cue_index + 1,
                    metrics.overshoot_ev,
                    config.max_overshoot_ev
                ));
            }
        }
    }
    violations
}

#[cfg(test)]
#[path = "ae_trace_verdict_tests.rs"]
mod tests;

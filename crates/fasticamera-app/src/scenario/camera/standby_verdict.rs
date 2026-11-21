//! Вердикт сценария `standby` (13b): чистая функция «отчёт → нарушения». Каждое правило проверено тестом, который
//! портит РОВНО одно значение здорового отчёта и ждёт ровно это нарушение.
//!
//! Правила (решение владельца): обмен ролей — именно повышение тёплой, а не переоткрытие; ни одного отказа тёплой;
//! `leaked_leases = 0`; каждая задняя камера держит ≥ `min_back_fps`, пока стримят обе.

use camera_core::lens_status::LensSwitchMethod;

use super::standby_config::StandbyScenarioConfig;
use super::standby_report::StandbyObservationReport;

/// Сколько обменов обязан сделать прогон: туда и обратно.
pub const EXPECTED_SWAPS: usize = 2;

pub fn evaluate_standby(config: &StandbyScenarioConfig, report: &StandbyObservationReport) -> Vec<String> {
    let mut violations = Vec::new();
    if let Some(failure) = &report.run_failure {
        violations.push(format!("прогон провален: {failure}"));
    }
    swap_violations(report, &mut violations);
    failure_violations(report, &mut violations);
    stream_violations(report, &mut violations);
    fps_violations(config, report, &mut violations);
    if report.run_failure.is_none() {
        end_state_violations(report, &mut violations);
    }
    violations
}

fn swap_violations(report: &StandbyObservationReport, violations: &mut Vec<String>) {
    if report.record.swaps.len() < EXPECTED_SWAPS {
        violations.push(format!(
            "обменов ролей {} из {EXPECTED_SWAPS}: прогон не дошёл до конца",
            report.record.swaps.len()
        ));
    }
    for swap in report.swaps_not_promoted() {
        violations.push(format!(
            "обмен {:?} на роль {} исполнен как {}, ожидалось повышение тёплой без переоткрытия",
            swap.leg,
            swap.to_role,
            method_name(swap.method),
        ));
    }
    if report.engine.promotions < EXPECTED_SWAPS as u64 {
        violations.push(format!(
            "движок повысил тёплую {} раз, ожидалось не меньше {EXPECTED_SWAPS}",
            report.engine.promotions
        ));
    }
}

fn failure_violations(report: &StandbyObservationReport, violations: &mut Vec<String>) {
    if report.engine.standby_failures > 0 {
        violations.push(format!("тёплая отказывала {} раз", report.engine.standby_failures));
    }
    if report.engine.promotion_fallbacks > 0 {
        violations.push(format!(
            "бэкенд отверг обмен ролей {} раз: движок переоткрывал камеру",
            report.engine.promotion_fallbacks
        ));
    }
    if report.engine.switches_failed > 0 {
        violations.push(format!("неудачных смен объектива: {}", report.engine.switches_failed));
    }
}

fn stream_violations(report: &StandbyObservationReport, violations: &mut Vec<String>) {
    if report.stream.leaked_leases > 0 {
        violations.push(format!(
            "утекло лизингов кадров: {}, ожидалось 0",
            report.stream.leaked_leases
        ));
    }
    if report.stream.standby_frames_returned == 0 {
        violations.push("кадры тёплой не возвращались камере: она не стримила".to_owned());
    }
    if report.stream.standby_promotions < EXPECTED_SWAPS as u64 {
        violations.push(format!(
            "поток кадров провёл обменов {}, ожидалось не меньше {EXPECTED_SWAPS}",
            report.stream.standby_promotions
        ));
    }
    if report.stream.standby_begins_refused > 0 {
        violations.push(format!(
            "поток кадров отказал в тёплой эпохе {} раз (max_standby_streams)",
            report.stream.standby_begins_refused
        ));
    }
    if report.backend_results_offered() == 0 {
        violations.push("бэкенд не предложил потоку ни одного результата тёплой".to_owned());
    }
}

fn fps_violations(config: &StandbyScenarioConfig, report: &StandbyObservationReport, violations: &mut Vec<String>) {
    let mut windows = 0_usize;
    for window in report.both_windows() {
        windows += 1;
        for (camera, fps) in [("показываемая", window.primary_fps), ("тёплая", window.standby_fps)] {
            if fps < config.min_back_fps {
                violations.push(format!(
                    "окно {}: {camera} камера {fps:.1} к/с, ниже порога {:.0}",
                    window.name, config.min_back_fps
                ));
            }
        }
    }
    // Без окон «обе камеры» частота ничем не доказана: тихий проход недопустим.
    if windows == 0 && report.run_failure.is_none() {
        violations.push("не замерена частота, пока стримят обе камеры".to_owned());
    }
}

fn end_state_violations(report: &StandbyObservationReport, violations: &mut Vec<String>) {
    if report.record.release_ms.is_none() {
        violations.push("тёплые не закрылись после «не нужно»: одна открытая камера не достигнута".to_owned());
    }
    if report.final_role != report.record.home_role {
        violations.push(format!(
            "в конце активна роль {:?}, а «дом» — {:?}",
            report.final_role, report.record.home_role
        ));
    }
}

fn method_name(method: LensSwitchMethod) -> &'static str {
    match method {
        LensSwitchMethod::Reopened => "reopened",
        LensSwitchMethod::PromotedStandby => "promoted_standby",
    }
}

#[cfg(test)]
#[path = "standby_verdict_tests.rs"]
mod tests;

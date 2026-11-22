//! Вердикт сценария `capture` (сессия 14): чистая функция «конфиг + отчёт → список нарушений».
//!
//! Каждое правило названо в тексте нарушения числами, по которым видно, что подвело. Пустой список — прогон прошёл.

use std::collections::BTreeSet;

use super::capture_burst::BurstRecord;
use super::capture_plan::{STEP_BURST, STEP_SINGLE_HEIF, STEP_SINGLE_HEIF_DNG};
use super::capture_report::{CaptureObservationReport, ShotOutcome, ShotRecord};
use crate::capture_backend::CaptureMode;
use crate::capture_scenario_config::CaptureScenarioConfig;

pub fn evaluate_capture(config: &CaptureScenarioConfig, report: &CaptureObservationReport) -> Vec<String> {
    let mut violations = Vec::new();
    if let Some(failure) = &report.run_failure {
        violations.push(failure.clone());
    }
    if !report.unfinished.is_empty() {
        violations.push(format!("шаги не дошли до конца: {}", report.unfinished.join(", ")));
    }
    for (when, mode) in [("при старте", &report.mode_at_start), ("в конце", &report.mode_at_end)] {
        if *mode != CaptureMode::Real {
            violations.push(format!("сервис снимка {when} не настоящий: {mode:?}"));
        }
    }
    if report.phase_entries_lost > 0 {
        violations.push(format!(
            "журнал итогов снимков переполнился: потеряно записей {}",
            report.phase_entries_lost
        ));
    }
    for shot in &report.shots {
        violations.extend(shot_violations(shot));
    }
    violations.extend(address_violations(report));
    violations.extend(single_step_violations(report));
    match &report.burst {
        Some(burst) => violations.extend(burst_violations(config, burst)),
        None if !report.unfinished.contains(&STEP_BURST) => {
            violations.push("серия выполнена, но её учёта нет".to_owned());
        }
        None => {}
    }
    violations.extend(render_violations(report));
    violations
}

/// Правила одного снимка: итог, формат, RAW.
fn shot_violations(shot: &ShotRecord) -> Vec<String> {
    let mut violations = Vec::new();
    let label = format!("снимок {} ({})", shot.number, shot.step);
    match &shot.outcome {
        ShotOutcome::Saved { raw, .. } => {
            if shot.raw_expected && raw.is_none() {
                violations.push(format!("{label}: RAW просили, а адреса DNG нет"));
            }
            if !shot.raw_expected && raw.is_some() {
                violations.push(format!("{label}: RAW не просили, а DNG записан"));
            }
        }
        ShotOutcome::Failed { failure } => violations.push(format!("{label} сорвался: {failure}")),
        ShotOutcome::Unfinished { secured } => violations.push(format!(
            "{label}: итога нет (кадр захвачен: {})",
            if *secured { "да" } else { "нет" }
        )),
    }
    if shot.raw_requested != shot.raw_expected {
        violations.push(format!(
            "{label}: экран просил RAW={}, шаг ждал {}",
            shot.raw_requested, shot.raw_expected
        ));
    }
    if shot.format != "heif" {
        violations.push(format!("{label}: основной формат {}, ждали heif", shot.format));
    }
    violations
}

/// Адреса файлов не повторяются: два снимка в один файл — потеря снимка.
fn address_violations(report: &CaptureObservationReport) -> Vec<String> {
    let uris = report.uris();
    let unique: BTreeSet<&str> = uris.iter().copied().collect();
    if unique.len() == uris.len() {
        return Vec::new();
    }
    vec![format!(
        "адреса файлов повторяются: {} адресов, уникальных {}",
        uris.len(),
        unique.len()
    )]
}

/// Одиночные шаги: ровно по одному сохранённому снимку (если шаг дошёл до конца).
fn single_step_violations(report: &CaptureObservationReport) -> Vec<String> {
    let mut violations = Vec::new();
    for name in [STEP_SINGLE_HEIF, STEP_SINGLE_HEIF_DNG] {
        let reached = report
            .steps
            .iter()
            .any(|step| step.name == name && step.failure.is_none());
        if !reached {
            continue;
        }
        let saved = report
            .shots
            .iter()
            .filter(|shot| shot.step == name && matches!(shot.outcome, ShotOutcome::Saved { .. }))
            .count();
        if saved != 1 {
            violations.push(format!("шаг {name}: сохранённых снимков {saved}, ждали ровно 1"));
        }
    }
    violations
}

/// Учёт серии: ни одно нажатие не пропало бесследно, всё принятое получило итог, видоискатель не просел.
fn burst_violations(config: &CaptureScenarioConfig, burst: &BurstRecord) -> Vec<String> {
    let plan = &config.burst;
    let mut violations = Vec::new();
    if burst.presses != plan.presses {
        violations.push(format!("серия: нажато {} из {}", burst.presses, plan.presses));
    }
    let accounted = burst.saved + burst.failed + burst.unfinished;
    if u64::from(accounted) != burst.accepted_by_service {
        violations.push(format!(
            "серия: принято службой {}, а итогов {} (сохранено {}, сорвано {}, без итога {})",
            burst.accepted_by_service, accounted, burst.saved, burst.failed, burst.unfinished
        ));
    }
    if burst.accepted_missing_from_journal > 0 {
        violations.push(format!(
            "серия: {} принятых запросов нет в журнале запросов",
            burst.accepted_missing_from_journal
        ));
    }
    // Отказ экрана обязан оставить след в его счётчике отказов: иначе нажатие пропало молча.
    if burst.refused_by_screen > burst.screen_actions_rejected {
        violations.push(format!(
            "серия: до сервиса не дошло {} нажатий, а экран записал отказов {}: нажатия пропали молча",
            burst.refused_by_screen, burst.screen_actions_rejected
        ));
    }
    if burst.failed > 0 {
        violations.push(format!("серия: сорвалось снимков {}", burst.failed));
    }
    if burst.unfinished > 0 {
        violations.push(format!("серия: без итога осталось снимков {}", burst.unfinished));
    }
    if burst.saved < plan.min_saved_shots {
        violations.push(format!(
            "серия: сохранено {}, нужно не меньше {}",
            burst.saved, plan.min_saved_shots
        ));
    }
    let view = &burst.viewfinder;
    let counter_limits = [
        (
            "исчерпан пул кадров камеры",
            view.pool_exhausted,
            plan.max_pool_exhausted,
        ),
        (
            "выброшено кадров джиттер-буфером",
            view.jitter_overflow,
            plan.max_jitter_overflow,
        ),
        (
            "чёрных кадров, где ждали картинку",
            view.black_frames_while_picture_expected,
            0,
        ),
        ("удержаний картинки", view.holds_started, 0),
    ];
    for (what, actual, limit) in counter_limits {
        if actual > limit {
            violations.push(format!("серия, видоискатель: {what} {actual} (допустимо {limit})"));
        }
    }
    violations
}

/// Рендер принял столько команд снимка, сколько принято службой, и ничего не потерял.
fn render_violations(report: &CaptureObservationReport) -> Vec<String> {
    let Some(delta) = report.render_still else {
        return Vec::new();
    };
    let mut violations = Vec::new();
    for (what, count) in [
        ("снимок не умеет бэкенд рендера", delta.begins_unsupported),
        ("цель кодера отброшена рендером", delta.targets_dropped),
        (
            "команды потеряны из-за сбоя бэкенда рендера",
            delta.lost_to_backend_fault,
        ),
    ] {
        if count > 0 {
            violations.push(format!("рендер: {what}: {count}"));
        }
    }
    let accepted = u64::try_from(report.shots.len()).unwrap_or(u64::MAX);
    if delta.begins_accepted != accepted {
        violations.push(format!(
            "рендер принял команд снимка {}, а службой принято {}",
            delta.begins_accepted, accepted
        ));
    }
    violations
}

#[cfg(test)]
#[path = "capture_verdict_tests.rs"]
mod tests;

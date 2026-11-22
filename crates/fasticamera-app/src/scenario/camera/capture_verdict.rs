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
    violations.extend(frame_violations(config, report));
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
    // Все ПРИНЯТЫЕ нажатия обязаны быть сохранены (сорванные и без итога названы выше); отказывать можно только
    // явно (экран: затвор занят / набор сохраняемых полон; служба: занята) и не больше `max_refused_presses`.
    let refused = burst.refused_by_screen + burst.rejected_by_service;
    if refused > u64::from(plan.max_refused_presses) {
        violations.push(format!(
            "серия: отказано нажатий {refused} (экран {}, служба {}), допустимо {}",
            burst.refused_by_screen, burst.rejected_by_service, plan.max_refused_presses
        ));
    }
    if u64::from(burst.saved) != burst.accepted_by_service {
        violations.push(format!(
            "серия: принято службой {}, а сохранено {}",
            burst.accepted_by_service, burst.saved
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

/// «Снимок без задержки затвора»: кадр был на экране, начало его экспозиции недалеко от нажатия, кольцо достаточно
/// глубоко, GPU снимков не терялся.
fn frame_violations(config: &CaptureScenarioConfig, report: &CaptureObservationReport) -> Vec<String> {
    let mut violations = Vec::new();
    let summary = report.frame_summary();
    if !report.shots.is_empty() {
        if summary.choices_unknown > 0 {
            violations.push(format!(
                "выбор кадра неизвестен у снимков: {} из {} (рендер не сообщил или журнал вытеснил)",
                summary.choices_unknown,
                report.shots.len()
            ));
        }
        match summary.exact_share {
            Some(share) if share < config.min_exact_frame_share => violations.push(format!(
                "точно показанный кадр взят у {} из {} снимков (доля {share:.2}), нужна не меньше {:.2}",
                summary.exact, summary.choices_known, config.min_exact_frame_share
            )),
            _ => {}
        }
        match summary.max_press_to_frame_ms {
            Some(max) if max > config.max_press_to_frame_ms => violations.push(format!(
                "задержка «нажатие → начало экспозиции кадра» до {max:.1} мс (медиана {}), допустимо {:.1}",
                summary
                    .p50_press_to_frame_ms
                    .map_or_else(|| "?".to_owned(), |value| format!("{value:.1}")),
                config.max_press_to_frame_ms
            )),
            Some(_) => {}
            None => violations.push(format!(
                "задержку нажатия не измерили ни у одного снимка: {}",
                summary
                    .delay_unavailable_reason
                    .as_deref()
                    .unwrap_or("причина не названа")
            )),
        }
    }
    // Вытеснение показанного кадра из кольца — недостаточная глубина кольца (решение 139): берём большее из
    // журнала выбора и счётчиков рендера, чтобы не пропустить, если один из источников недоступен.
    let evicted_by_render = report
        .render_still_detail
        .map_or(0, |delta| delta.picks.substituted_evicted);
    let evicted = u64::from(summary.substituted_evicted).max(evicted_by_render);
    if evicted > 0 {
        violations.push(format!(
            "показанный кадр вытеснен из кольца у {evicted} снимков: кольцо мелкое (решение 139)"
        ));
    }
    if let Some(delta) = report.render_still_detail
        && delta.gpu_lost > 0
    {
        violations.push(format!(
            "рендер: GPU снимков объявлен потерянным {} раз (по сроку {})",
            delta.gpu_lost, delta.gpu_lost_by_deadline
        ));
    }
    violations
}

#[cfg(test)]
#[path = "capture_verdict_tests.rs"]
mod tests;

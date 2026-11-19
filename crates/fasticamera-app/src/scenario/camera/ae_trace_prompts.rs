//! Тексты подсказок владельцу в сценарии `ae-trace` (чистые функции: событие расписания -> что показать).
//!
//! Владелец смотрит на экран с лампой в руке, поэтому надписи короткие и крупные, а смысл виден по цвету
//! (`Prepare` — приготовься, `Act` — щёлкай сейчас, `Info` — справка). Здесь только слова: когда показывать,
//! решает расписание (`ae_trace_plan`), как показывать — платформа.

use shell_contract::operator_prompt::{OperatorPrompt, OperatorPromptKind};

use super::ae_trace_plan::{PlanEvent, Timeline, TracePhase};

/// Название фазы для человека.
fn phase_title(phase: TracePhase) -> &'static str {
    match phase {
        TracePhase::Photo => "фото",
        TracePhase::Video => "видео",
        TracePhase::Static => "статика",
    }
}

/// Номер фазы с единицы: «фаза 2 из 3».
fn phase_number(phase: TracePhase) -> usize {
    TracePhase::ALL
        .iter()
        .position(|candidate| *candidate == phase)
        .map_or(0, |position| position + 1)
}

/// Что делать владельцу в фазе (пояснение под названием).
fn phase_instruction(phase: TracePhase) -> &'static str {
    if phase.has_cues() {
        "жди отсчёта, потом щёлкай лампой"
    } else {
        "ничего не трогай, свет не меняй"
    }
}

/// «ступенька 3 из 5 · фото».
fn step_detail(timeline: &Timeline, phase: TracePhase, index: usize) -> String {
    format!(
        "ступенька {} из {} · {}",
        index + 1,
        timeline.cue_count(phase),
        phase_title(phase)
    )
}

/// Подсказка для события расписания; `timeline` нужен для счёта «ступенька i из N».
pub fn prompt_for(event: PlanEvent, timeline: &Timeline) -> OperatorPrompt {
    match event {
        PlanEvent::PhaseStart(phase) => OperatorPrompt::new(
            OperatorPromptKind::Info,
            format!(
                "ФАЗА {} ИЗ {}: {}",
                phase_number(phase),
                TracePhase::ALL.len(),
                phase_title(phase).to_uppercase()
            ),
            phase_instruction(phase),
        ),
        PlanEvent::Countdown {
            phase,
            index,
            seconds_left,
        } => OperatorPrompt::new(
            OperatorPromptKind::Prepare,
            format!("ПРИГОТОВЬСЯ… {seconds_left}"),
            step_detail(timeline, phase, index),
        ),
        PlanEvent::Cue { phase, index } => OperatorPrompt::new(
            OperatorPromptKind::Act,
            "ЩЁЛКНИ ЛАМПОЙ",
            step_detail(timeline, phase, index),
        ),
        PlanEvent::CueShown { phase, index } => OperatorPrompt::new(
            OperatorPromptKind::Info,
            "ЖДИ СЛЕДУЮЩЕЙ СТУПЕНЬКИ",
            format!("{} — сделана", step_detail(timeline, phase, index)),
        ),
    }
}

/// Когда подсказка понадобилась — для журнала и текста нарушения («подсказка 1 фазы photo»).
pub fn moment_of(event: PlanEvent) -> String {
    match event {
        PlanEvent::PhaseStart(phase) => format!("старт {}", phase.name()),
        PlanEvent::Countdown {
            phase,
            index,
            seconds_left,
        } => format!("отсчёт {seconds_left} к подсказке {} фазы {}", index + 1, phase.name()),
        PlanEvent::Cue { phase, index } => format!("подсказка {} фазы {}", index + 1, phase.name()),
        PlanEvent::CueShown { phase, index } => {
            format!("«жди» после подсказки {} фазы {}", index + 1, phase.name())
        }
    }
}

#[cfg(test)]
#[path = "ae_trace_prompts_tests.rs"]
mod tests;

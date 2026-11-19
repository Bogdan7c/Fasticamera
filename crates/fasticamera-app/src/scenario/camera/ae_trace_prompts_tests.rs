//! Тексты подсказок `ae-trace`: вид (цвет) и слова для каждого события расписания.

use super::*;
use crate::ae_trace_scenario_config::AeTraceScenarioConfig;

fn timeline() -> Timeline {
    Timeline::from_config(&AeTraceScenarioConfig::default())
}

#[test]
fn phase_starts_name_the_phase_and_tell_what_to_do() {
    let timeline = timeline();
    let photo = prompt_for(PlanEvent::PhaseStart(TracePhase::Photo), &timeline);
    assert_eq!(photo.kind(), OperatorPromptKind::Info);
    assert_eq!(photo.headline(), "ФАЗА 1 ИЗ 3: ФОТО");
    assert_eq!(photo.detail(), "жди отсчёта, потом щёлкай лампой");
    let statics = prompt_for(PlanEvent::PhaseStart(TracePhase::Static), &timeline);
    assert_eq!(statics.headline(), "ФАЗА 3 ИЗ 3: СТАТИКА");
    assert_eq!(statics.detail(), "ничего не трогай, свет не меняй");
}

#[test]
fn countdown_is_prepare_act_is_act_and_both_count_the_step() {
    let timeline = timeline();
    let countdown = prompt_for(
        PlanEvent::Countdown {
            phase: TracePhase::Video,
            index: 2,
            seconds_left: 2,
        },
        &timeline,
    );
    assert_eq!(countdown.kind(), OperatorPromptKind::Prepare);
    assert_eq!(countdown.headline(), "ПРИГОТОВЬСЯ… 2");
    assert_eq!(countdown.detail(), "ступенька 3 из 5 · видео");

    let cue = prompt_for(
        PlanEvent::Cue {
            phase: TracePhase::Video,
            index: 2,
        },
        &timeline,
    );
    assert_eq!(cue.kind(), OperatorPromptKind::Act);
    assert_eq!(cue.headline(), "ЩЁЛКНИ ЛАМПОЙ");
    assert_eq!(cue.detail(), "ступенька 3 из 5 · видео");
}

#[test]
fn after_the_cue_the_label_turns_into_waiting() {
    let shown = prompt_for(
        PlanEvent::CueShown {
            phase: TracePhase::Photo,
            index: 0,
        },
        &timeline(),
    );
    assert_eq!(shown.kind(), OperatorPromptKind::Info);
    assert_eq!(shown.headline(), "ЖДИ СЛЕДУЮЩЕЙ СТУПЕНЬКИ");
    assert_eq!(shown.detail(), "ступенька 1 из 5 · фото — сделана");
}

#[test]
fn moments_name_the_event_for_violations() {
    assert_eq!(moment_of(PlanEvent::PhaseStart(TracePhase::Video)), "старт video");
    assert_eq!(
        moment_of(PlanEvent::Cue {
            phase: TracePhase::Photo,
            index: 0
        }),
        "подсказка 1 фазы photo"
    );
    assert_eq!(
        moment_of(PlanEvent::Countdown {
            phase: TracePhase::Photo,
            index: 4,
            seconds_left: 3
        }),
        "отсчёт 3 к подсказке 5 фазы photo"
    );
}

//! Вердикт `capture`: здоровый прогон проходит, каждое правило ловит своё нарушение и называет число.

use super::super::capture_report::ShotOutcome;
use super::super::capture_test_support::{Behaviour, World, drive, fast_config, healthy_report, report_of};
use super::*;
use crate::capture_scenario_config::CaptureScenarioConfig;
use std::time::Duration;

fn config() -> CaptureScenarioConfig {
    fast_config()
}

fn violations_of(mutate: impl FnOnce(&mut CaptureObservationReport)) -> Vec<String> {
    let mut report = healthy_report();
    mutate(&mut report);
    evaluate_capture(&config(), &report)
}

fn has(violations: &[String], needle: &str) -> bool {
    violations.iter().any(|violation| violation.contains(needle))
}

#[test]
fn a_healthy_run_has_no_violations() {
    assert_eq!(evaluate_capture(&config(), &healthy_report()), Vec::<String>::new());
}

#[test]
fn a_fake_service_at_start_or_end_fails_the_run() {
    let at_start = violations_of(|report| {
        report.mode_at_start = CaptureMode::FakeFallback {
            reason: "нет рендера".to_owned(),
        }
    });
    assert!(
        has(&at_start, "при старте не настоящий") && has(&at_start, "нет рендера"),
        "{at_start:?}"
    );
    let at_end = violations_of(|report| report.mode_at_end = CaptureMode::FakeForScenario { scenario: "screen" });
    assert!(has(&at_end, "в конце не настоящий"), "{at_end:?}");
}

#[test]
fn a_failed_or_unfinished_shot_is_a_violation_naming_the_shot() {
    let failed = violations_of(|report| {
        report.shots[0].outcome = ShotOutcome::Failed {
            failure: "кодирование: HEIC".to_owned(),
        }
    });
    assert!(
        has(&failed, "снимок 1 (single_heif) сорвался: кодирование: HEIC"),
        "{failed:?}"
    );
    let unfinished = violations_of(|report| report.shots[1].outcome = ShotOutcome::Unfinished { secured: true });
    assert!(
        has(&unfinished, "снимок 2 (single_heif_dng): итога нет (кадр захвачен: да)"),
        "{unfinished:?}"
    );
}

#[test]
fn raw_must_match_what_the_step_promised_in_the_request_and_in_the_files() {
    let missing_dng = violations_of(|report| {
        if let ShotOutcome::Saved { raw, .. } = &mut report.shots[1].outcome {
            *raw = None;
        }
    });
    assert!(has(&missing_dng, "RAW просили, а адреса DNG нет"), "{missing_dng:?}");
    let extra_dng = violations_of(|report| {
        if let ShotOutcome::Saved { raw, .. } = &mut report.shots[0].outcome {
            *raw = Some("content://media/x".to_owned());
        }
    });
    assert!(has(&extra_dng, "RAW не просили, а DNG записан"), "{extra_dng:?}");
    let raw_left_on = violations_of(|report| report.shots[5].raw_requested = true);
    assert!(
        has(&raw_left_on, "экран просил RAW=true, шаг ждал false"),
        "{raw_left_on:?}"
    );
}

#[test]
fn the_primary_format_must_be_heif() {
    let violations = violations_of(|report| report.shots[0].format = "jpeg");
    assert!(has(&violations, "основной формат jpeg, ждали heif"), "{violations:?}");
}

#[test]
fn two_shots_in_one_file_are_a_violation() {
    let violations = violations_of(|report| {
        let duplicate = match &report.shots[0].outcome {
            ShotOutcome::Saved { primary, .. } => primary.clone(),
            other => panic!("{other:?}"),
        };
        if let ShotOutcome::Saved { primary, .. } = &mut report.shots[3].outcome {
            *primary = duplicate;
        }
    });
    assert!(
        has(&violations, "адреса файлов повторяются: 13 адресов, уникальных 12"),
        "{violations:?}"
    );
}

#[test]
fn unreached_steps_and_the_run_failure_are_reported() {
    let violations = violations_of(|report| {
        report.unfinished = vec!["burst"];
        report.run_failure = Some("канал входа экрана не принял вложение сценария".to_owned());
        report.burst = None;
    });
    assert!(has(&violations, "шаги не дошли до конца: burst"), "{violations:?}");
    assert!(has(&violations, "канал входа экрана не принял"), "{violations:?}");
    assert!(
        !has(&violations, "учёта нет"),
        "серия не дошла, её учёта и не должно быть"
    );
}

#[test]
fn a_finished_burst_without_accounting_is_a_violation() {
    let violations = violations_of(|report| report.burst = None);
    assert!(has(&violations, "серия выполнена, но её учёта нет"), "{violations:?}");
}

#[test]
fn burst_accounting_must_add_up_and_every_failure_is_named() {
    let lost = violations_of(|report| {
        let burst = report.burst.as_mut().unwrap();
        burst.saved -= 1; // принято 10, а итогов 9: один снимок пропал бесследно
    });
    assert!(has(&lost, "принято службой 10, а итогов 9"), "{lost:?}");

    let silent_refusals = violations_of(|report| {
        let burst = report.burst.as_mut().unwrap();
        burst.refused_by_screen = 3;
        burst.screen_actions_rejected = 1;
    });
    assert!(
        has(&silent_refusals, "не дошло 3 нажатий, а экран записал отказов 1"),
        "{silent_refusals:?}"
    );

    let failed = violations_of(|report| {
        let burst = report.burst.as_mut().unwrap();
        burst.saved -= 1;
        burst.failed += 1;
    });
    assert!(has(&failed, "серия: сорвалось снимков 1"), "{failed:?}");

    let too_few = violations_of(|report| {
        let burst = report.burst.as_mut().unwrap();
        burst.saved = 0;
        burst.accepted_by_service = 0;
    });
    assert!(has(&too_few, "сохранено 0, нужно не меньше 1"), "{too_few:?}");
}

#[test]
fn a_burst_that_drops_viewfinder_frames_fails_with_the_counter() {
    let violations = violations_of(|report| {
        let view = &mut report.burst.as_mut().unwrap().viewfinder;
        view.pool_exhausted = 1;
        view.jitter_overflow = 2;
        view.black_frames_while_picture_expected = 1;
        view.holds_started = 1;
        view.late_frames = 500; // опоздавшие кадры — только число, не провал
    });
    assert!(
        has(&violations, "исчерпан пул кадров камеры 1 (допустимо 0)"),
        "{violations:?}"
    );
    assert!(
        has(&violations, "выброшено кадров джиттер-буфером 2 (допустимо 1)"),
        "{violations:?}"
    );
    assert!(
        has(&violations, "чёрных кадров, где ждали картинку 1"),
        "{violations:?}"
    );
    assert!(has(&violations, "удержаний картинки 1"), "{violations:?}");
    assert!(
        !has(&violations, "опоздавших") && !has(&violations, "late"),
        "{violations:?}"
    );
    // Один выброшенный кадр в пределах допуска — не нарушение.
    let within = violations_of(|report| report.burst.as_mut().unwrap().viewfinder.jitter_overflow = 1);
    assert!(within.is_empty(), "{within:?}");
}

#[test]
fn the_render_must_have_accepted_every_command_and_lost_none() {
    let violations = violations_of(|report| {
        let delta = report.render_still.as_mut().unwrap();
        delta.begins_accepted -= 1;
        delta.begins_unsupported = 2;
        delta.targets_dropped = 1;
        delta.lost_to_backend_fault = 1;
    });
    assert!(
        has(&violations, "рендер принял команд снимка 11, а службой принято 12"),
        "{violations:?}"
    );
    assert!(has(&violations, "снимок не умеет бэкенд рендера: 2"), "{violations:?}");
    assert!(has(&violations, "цель кодера отброшена рендером: 1"), "{violations:?}");
    assert!(
        has(&violations, "потеряны из-за сбоя бэкенда рендера: 1"),
        "{violations:?}"
    );
    // Рендер не прочитан — правило молчит (его отсутствие видно в отчёте полем null).
    assert!(violations_of(|report| report.render_still = None).is_empty());
}

#[test]
fn lost_journal_entries_are_a_violation() {
    let violations = violations_of(|report| report.phase_entries_lost = 4);
    assert!(has(&violations, "потеряно записей 4"), "{violations:?}");
}

#[test]
fn a_run_with_a_failed_single_step_is_judged_on_the_runner_failure_without_extra_noise() {
    let mut world = World::new(Behaviour {
        failing_requests: [1].into_iter().collect(),
        ..Behaviour::default()
    });
    let mut runner = super::super::capture_runner::CaptureRunner::new(
        fast_config(),
        super::super::capture_plan::planned_steps(),
        world.now(),
    );
    drive(
        &mut runner,
        &mut world,
        Duration::from_millis(10),
        Duration::from_secs(60),
    );

    let violations = evaluate_capture(&config(), &report_of(runner.into_records()));

    assert!(has(&violations, "снимок 1 сорвался"), "{violations:?}");
    assert!(
        has(&violations, "шаги не дошли до конца: single_heif_dng, burst"),
        "{violations:?}"
    );
    assert!(!has(&violations, "учёта нет"), "{violations:?}");
}

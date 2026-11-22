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

    // Принято меньше, чем нажато: явные отказы допустимы, пока их не больше порога (`max_refused_presses` = 3).
    let tolerated = violations_of(|report| {
        let burst = report.burst.as_mut().unwrap();
        burst.refused_by_screen = 3;
        burst.screen_actions_rejected = 3;
        burst.accepted_by_service = 7;
        burst.submitted = 7;
        burst.saved = 7;
        report.shots.truncate(9);
    });
    assert!(!has(&tolerated, "отказано нажатий"), "{tolerated:?}");

    let too_many = violations_of(|report| {
        let burst = report.burst.as_mut().unwrap();
        burst.refused_by_screen = 3;
        burst.rejected_by_service = 1;
        burst.screen_actions_rejected = 3;
    });
    assert!(
        has(&too_many, "отказано нажатий 4 (экран 3, служба 1), допустимо 3"),
        "{too_many:?}"
    );

    // Все принятые обязаны быть сохранены, даже если отказов в пределах порога.
    let accepted_not_saved = violations_of(|report| {
        let burst = report.burst.as_mut().unwrap();
        burst.accepted_by_service = 10;
        burst.saved = 9;
        burst.unfinished = 1;
    });
    assert!(
        has(&accepted_not_saved, "принято службой 10, а сохранено 9"),
        "{accepted_not_saved:?}"
    );
}

#[test]
fn a_burst_that_drops_viewfinder_frames_fails_with_the_counter() {
    let violations = violations_of(|report| {
        let view = &mut report.burst.as_mut().unwrap().viewfinder;
        view.pool_exhausted = 1;
        // 20 выброшенных на 80 показанных — доля 0,2 > 0,15 (решение 147).
        view.jitter_overflow = 20;
        view.live_frames_presented = 80;
        view.black_frames_while_picture_expected = 1;
        view.holds_started = 1;
        view.late_frames = 500; // опоздавшие кадры — только число, не провал
    });
    assert!(
        has(&violations, "исчерпан пул кадров камеры 1 (допустимо 0)"),
        "{violations:?}"
    );
    assert!(
        has(
            &violations,
            "выброшено джиттер-буфером 20 из 100 кадров (доля 0.200), допустимо 0.15"
        ),
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
    // Доля ровно на пороге — не нарушение (15 из 100).
    let within = violations_of(|report| {
        let view = &mut report.burst.as_mut().unwrap().viewfinder;
        view.jitter_overflow = 15;
        view.live_frames_presented = 85;
    });
    assert!(within.is_empty(), "{within:?}");
    // Чуть выше порога — нарушение (16 из 100).
    let above = violations_of(|report| {
        let view = &mut report.burst.as_mut().unwrap().viewfinder;
        view.jitter_overflow = 16;
        view.live_frames_presented = 84;
    });
    assert!(has(&above, "доля 0.160"), "{above:?}");
    // Ни одного кадра за серию — доля не считается (деления на ноль нет).
    let empty = violations_of(|report| {
        let view = &mut report.burst.as_mut().unwrap().viewfinder;
        view.jitter_overflow = 0;
        view.live_frames_presented = 0;
    });
    assert!(!has(&empty, "джиттер"), "{empty:?}");
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

// ---- «Снимок без задержки затвора» ----------------------------------------------------------------------------

use super::super::capture_frame_timing::FrameChoiceView;
use super::super::capture_render_still::StillDiagnosticsDelta;
use still_contract::frame_choice::SubstitutionReason;

fn substitute(shot: &mut super::super::capture_report::ShotRecord, reason: SubstitutionReason) {
    shot.frame.frame_choice = FrameChoiceView::Substituted { reason };
    shot.frame.shown_before_press_ms = None;
}

#[test]
fn a_healthy_run_has_exact_frames_with_the_expected_delay() {
    let report = healthy_report();
    let summary = report.frame_summary();
    assert_eq!((summary.exact, summary.choices_unknown), (12, 0));
    assert_eq!(summary.max_press_to_frame_ms, Some(80.0));
}

#[test]
fn too_few_exact_frames_fail_with_the_share_and_the_threshold() {
    // Порог 0,9: 12 снимков, 2 замены → доля 0,83.
    let violations = violations_of(|report| {
        substitute(&mut report.shots[0], SubstitutionReason::PresentationTimingUnknown);
        substitute(&mut report.shots[1], SubstitutionReason::PresentationTimingUnknown);
    });
    assert!(
        has(&violations, "взят у 10 из 12 снимков (доля 0.83), нужна не меньше 0.90"),
        "{violations:?}"
    );
    // Одна замена из 12 — 0,92: в пределах порога.
    let within =
        violations_of(|report| substitute(&mut report.shots[0], SubstitutionReason::PresentationTimingUnknown));
    assert!(within.is_empty(), "{within:?}");
}

#[test]
fn a_late_frame_fails_with_the_max_the_median_and_the_limit() {
    let violations = violations_of(|report| report.shots[3].frame.press_to_frame_ms = Some(212.5));
    assert!(
        has(&violations, "до 212.5 мс (медиана 80.0), допустимо 150.0"),
        "{violations:?}"
    );
    // Ровно на пороге — не нарушение.
    let at_limit = violations_of(|report| report.shots[3].frame.press_to_frame_ms = Some(150.0));
    assert!(at_limit.is_empty(), "{at_limit:?}");
}

#[test]
fn a_run_whose_delay_was_never_measured_fails_with_the_clock_reason() {
    let violations = violations_of(|report| {
        for shot in &mut report.shots {
            shot.frame.press_to_frame_ms = None;
            shot.frame.press_to_frame_unavailable = Some("нет смещения часов: errno 1".to_owned());
        }
    });
    assert!(
        has(
            &violations,
            "задержку нажатия не измерили ни у одного снимка: нет смещения часов: errno 1"
        ),
        "{violations:?}"
    );
}

#[test]
fn shots_without_a_known_choice_are_named_not_silently_skipped() {
    let violations = violations_of(|report| {
        report.shots[0].frame = super::super::capture_frame_timing::FrameTiming::of(
            None,
            "нет записи",
            shell_contract::clock::MonotonicNanos::from_nanos(0),
            &super::super::capture_frame_timing::ClockBridge::Unavailable { reason: "x".to_owned() },
        );
    });
    assert!(
        has(&violations, "выбор кадра неизвестен у снимков: 1 из 12"),
        "{violations:?}"
    );
}

#[test]
fn an_evicted_shown_frame_fails_the_ring_depth_check_from_either_source() {
    let by_journal = violations_of(|report| substitute(&mut report.shots[0], SubstitutionReason::ShownFrameEvicted));
    assert!(has(&by_journal, "вытеснен из кольца у 1 снимков"), "{by_journal:?}");

    let by_render = violations_of(|report| {
        let mut delta = StillDiagnosticsDelta::default();
        delta.picks.substituted_evicted = 2;
        report.render_still_detail = Some(delta);
    });
    assert!(has(&by_render, "вытеснен из кольца у 2 снимков"), "{by_render:?}");
}

#[test]
fn a_lost_still_gpu_fails_the_run() {
    let violations = violations_of(|report| {
        report.render_still_detail = Some(StillDiagnosticsDelta {
            gpu_lost: 1,
            gpu_lost_by_deadline: 1,
            ..StillDiagnosticsDelta::default()
        });
    });
    assert!(
        has(&violations, "GPU снимков объявлен потерянным 1 раз (по сроку 1)"),
        "{violations:?}"
    );
}

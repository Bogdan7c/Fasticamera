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
        has(&violations, "адреса файлов повторяются: 15 адресов, уникальных 14"),
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
        has(&violations, "рендер принял команд снимка 13, а службой принято 14"),
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
        has(
            &violations,
            "шаги не дошли до конца: single_heif_dng, single_flash_on, single_flash_auto, burst"
        ),
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
    assert_eq!((summary.exact, summary.choices_unknown), (14, 0));
    assert_eq!(summary.max_press_to_frame_ms, Some(80.0));
}

#[test]
fn too_few_exact_frames_fail_with_the_share_and_the_threshold() {
    // Порог 0,9: 14 снимков, 2 замены → доля 0,86.
    let violations = violations_of(|report| {
        substitute(&mut report.shots[0], SubstitutionReason::PresentationTimingUnknown);
        substitute(&mut report.shots[1], SubstitutionReason::PresentationTimingUnknown);
    });
    assert!(
        has(&violations, "взят у 12 из 14 снимков (доля 0.86), нужна не меньше 0.90"),
        "{violations:?}"
    );
    // Одна замена из 14 — 0,93: в пределах порога.
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
        has(&violations, "выбор кадра неизвестен у снимков: 1 из 14"),
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

#[test]
fn an_unlit_or_never_arrived_flash_frame_fails_the_run() {
    let violations = violations_of(|report| {
        let mut delta = StillDiagnosticsDelta::default();
        delta.flash.not_lit = 1;
        delta.flash.never_arrived = 2;
        report.render_still_detail = Some(delta);
    });
    assert!(
        has(
            &violations,
            "кадр со вспышкой не освещён 1 раз, не дошёл до кольца 2 раз"
        ),
        "{violations:?}"
    );
    // Освещённые и «не решаемые» кадры прогон не валят.
    let fine = violations_of(|report| {
        let mut delta = StillDiagnosticsDelta::default();
        delta.flash.lit_check_passed = 1;
        delta.flash.lit_check_skipped = 1;
        report.render_still_detail = Some(delta);
    });
    assert!(!has(&fine, "кадр со вспышкой"), "{fine:?}");
}

// ---- Вспышка (14d) ----------------------------------------------------------------------------------------------

use super::super::capture_flash_record::CameraFlashRecord;
use crate::still_capture_service::FlashModeName;

/// Номер снимка шага `single_flash_on` в здоровом отчёте.
const FLASH_ON: usize = 2;
/// Номер снимка шага `single_flash_auto` в здоровом отчёте.
const FLASH_AUTO: usize = 3;

#[test]
fn a_healthy_flash_step_is_not_judged_by_the_press_to_frame_threshold() {
    // Кадр вспышки снят после нажатия: даже «огромная» формальная задержка его не касается (её у него нет).
    let healthy = healthy_report();
    assert!(healthy.shots[FLASH_ON].frame.is_flash_frame());
    assert_eq!(healthy.shots[FLASH_ON].frame.press_to_frame_ms, None);
    assert!(evaluate_capture(&config(), &healthy).is_empty());
    assert_eq!(healthy.frame_summary().flash_frames, 1);
    assert_eq!(healthy.frame_summary().delays_measured, 13, "только снимки без вспышки");
}

#[test]
fn flash_on_without_a_flash_frame_is_a_violation_naming_the_choice() {
    let violations = violations_of(|report| {
        report.shots[FLASH_ON].frame.frame_choice = FrameChoiceView::Exact;
    });
    assert!(
        has(&violations, "вспышка включена, а взят не кадр со вспышкой (exact)"),
        "{violations:?}"
    );
}

#[test]
fn flash_on_needs_the_service_route_and_a_fired_camera_report() {
    let no_route = violations_of(|report| {
        report.shots[FLASH_ON].flash.as_mut().unwrap().route_uses_flash = Some(false);
    });
    assert!(
        has(&no_route, "маршрут сервиса со вспышкой не подтверждён"),
        "{no_route:?}"
    );
    let camera_failed = violations_of(|report| {
        report.shots[FLASH_ON].flash.as_mut().unwrap().camera = Some(CameraFlashRecord::Failed {
            failure: "PreflashNotMeasured".to_owned(),
        });
    });
    assert!(
        has(&camera_failed, "камера сообщила срыв вспышки: PreflashNotMeasured"),
        "{camera_failed:?}"
    );
    let silent_camera = violations_of(|report| {
        report.shots[FLASH_ON].flash.as_mut().unwrap().camera = None;
    });
    assert!(
        has(&silent_camera, "камера не сообщила итог вспышки"),
        "{silent_camera:?}"
    );
}

#[test]
fn a_flash_frame_where_the_flash_is_off_and_a_wrong_screen_setting_are_violations() {
    let off_with_flash_frame = violations_of(|report| {
        report.shots[0].frame.frame_choice = FrameChoiceView::FlashFrame;
    });
    assert!(
        has(&off_with_flash_frame, "вспышка выключена, а взят кадр со вспышкой"),
        "{off_with_flash_frame:?}"
    );
    let wrong_setting = violations_of(|report| report.shots[5].flash_requested = FlashModeName::On);
    assert!(
        has(&wrong_setting, "экран просил вспышку On, шаг ждал Off"),
        "{wrong_setting:?}"
    );
}

#[test]
fn auto_flash_accepts_either_outcome_but_the_frame_must_match_the_service_decision() {
    // Сервис не вспыхивал, кадр показанный — наблюдаемый исход «вспышка не понадобилась»: без нарушений.
    assert!(violations_of(|_| {}).is_empty());
    // Сервис решил вспыхнуть, а кадр показанный: расхождение решения и кадра.
    let mismatch = violations_of(|report| {
        report.shots[FLASH_AUTO].flash.as_mut().unwrap().route_uses_flash = Some(true);
    });
    assert!(
        has(&mismatch, "Auto: сервис решил вспышка=true, а взят кадр «exact»"),
        "{mismatch:?}"
    );
    // Сервис решил вспыхнуть и кадр вспышки: исход допустим.
    let fired = violations_of(|report| {
        report.shots[FLASH_AUTO].frame.frame_choice = FrameChoiceView::FlashFrame;
        report.shots[FLASH_AUTO].flash.as_mut().unwrap().route_uses_flash = Some(true);
    });
    assert!(fired.is_empty(), "{fired:?}");
}

#[test]
fn a_flash_step_must_leave_exactly_one_saved_shot() {
    let violations = violations_of(|report| {
        report.shots[FLASH_ON].step = "single_flash_auto";
    });
    assert!(
        has(&violations, "шаг single_flash_on: сохранённых снимков 0, ждали ровно 1"),
        "{violations:?}"
    );
    assert!(
        has(
            &violations,
            "шаг single_flash_auto: сохранённых снимков 2, ждали ровно 1"
        ),
        "{violations:?}"
    );
}

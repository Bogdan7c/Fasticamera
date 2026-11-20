//! Вердикт `zoom`: здоровый отчёт проходит, и каждое правило ловит РОВНО свою порчу (портится одно поле).

use camera_contract::ids::{LensFacing, LensRole};

use super::*;
use crate::scenario::camera::zoom_record::{HapticWindow, MirrorCheck, SwitchTiming};
use crate::scenario::camera::zoom_report::FinalState;
use crate::scenario::camera::zoom_reveal::RevealFaults;
use crate::scenario::camera::zoom_test_support::{healthy_report, healthy_switch_step};

fn config() -> ZoomScenarioConfig {
    ZoomScenarioConfig::default()
}

fn violations_of(report: &ZoomObservationReport) -> Vec<String> {
    evaluate_zoom(&config(), report)
}

fn assert_single(violations: &[String], needle: &str) {
    assert_eq!(
        violations.len(),
        1,
        "ждали одно нарушение про «{needle}», получили {violations:?}"
    );
    assert!(
        violations[0].contains(needle),
        "нарушение «{}» не про «{needle}»",
        violations[0]
    );
}

#[test]
fn a_healthy_report_passes() {
    assert_eq!(violations_of(&healthy_report()), Vec::<String>::new());
}

#[test]
fn a_single_black_frame_where_a_picture_was_expected_fails_and_names_the_step() {
    let mut report = healthy_report();
    report.steps[0].black_frames_while_picture_expected = 1;
    report.totals.black_frames_while_picture_expected = 1;
    let violations = violations_of(&report);
    assert_single(&violations, "чёрных кадров");
    assert!(violations[0].contains("01_tap_0.6"));
}

#[test]
fn a_failed_lens_switch_in_the_engine_counters_fails() {
    let mut report = healthy_report();
    report.totals.switches_failed = 1;
    assert_single(&violations_of(&report), "неудачных сменах");
}

#[test]
fn a_step_failure_is_reported_with_its_name_and_reason() {
    let mut report = healthy_report();
    report.steps[1].failure = Some("смена объектива не удалась".to_owned());
    let violations = violations_of(&report);
    assert_single(&violations, "02_tap_2: смена объектива не удалась");
}

#[test]
fn unfinished_steps_are_named_when_nothing_else_explains_the_stop() {
    let mut report = healthy_report();
    report.unfinished = vec!["04_tap_1".to_owned()];
    assert_single(&violations_of(&report), "04_tap_1");
}

#[test]
fn a_run_failure_outside_the_steps_is_the_reported_reason() {
    let mut report = healthy_report();
    report.run_failure = Some("экран камеры не принимает вход".to_owned());
    report.unfinished = vec!["01_tap_0.6".to_owned()];
    assert_single(&violations_of(&report), "не принимает вход");
}

#[test]
fn a_slow_engine_event_and_a_slow_first_live_frame_are_separate_violations() {
    let mut report = healthy_report();
    report.steps[0].switch = Some(SwitchTiming {
        observed_ms: config().max_switch_ms + 1,
        ..report.steps[0].switch.expect("смена")
    });
    assert_single(&violations_of(&report), "событие смены объектива");

    let mut report = healthy_report();
    report.steps[0].first_live_frame_ms = Some(config().max_first_live_frame_ms as f64 + 1.0);
    assert_single(&violations_of(&report), "первый живой кадр");
}

#[test]
fn a_missing_live_frame_is_a_violation_unless_the_ring_lost_records() {
    let mut report = healthy_report();
    report.steps[0].first_live_frame_ms = None;
    assert_single(&violations_of(&report), "не найден");

    report.steps[0].ring_overran = true;
    assert_eq!(
        violations_of(&report),
        Vec::<String>::new(),
        "потеря записей — «не измерено», а не провал времени"
    );
}

#[test]
fn frame_rate_below_the_tolerated_floor_fails_per_camera_kind() {
    let mut report = healthy_report();
    report.steps[0].measured_fps = Some(53.9); // порог 60 × 0,9 = 54
    assert_single(&violations_of(&report), "частота потока");
    report.steps[0].measured_fps = Some(54.1);
    assert_eq!(violations_of(&report), Vec::<String>::new());

    // Фронтальная держит 30: 29 проходит, 26 — нет.
    report.steps[0].expected_fps = Some(30);
    report.steps[0].measured_fps = Some(29.0);
    assert_eq!(violations_of(&report), Vec::<String>::new());
    report.steps[0].measured_fps = Some(26.0);
    assert_single(&violations_of(&report), "частота потока");
}

#[test]
fn an_unmeasured_frame_rate_fails() {
    let mut report = healthy_report();
    report.steps[1].measured_fps = None;
    assert_single(&violations_of(&report), "не измерена");
}

#[test]
fn a_wrong_mirror_fails_and_an_unseen_camera_fails_unless_the_ring_lost_records() {
    let mut report = healthy_report();
    report.steps[0].mirror = MirrorCheck::Wrong { frames: 2 };
    assert_single(&violations_of(&report), "зеркало не то");

    let mut report = healthy_report();
    report.steps[0].mirror = MirrorCheck::NoFrameObserved;
    assert_single(&violations_of(&report), "зеркало не измерено");
    report.steps[0].ring_overran = true;
    assert_eq!(violations_of(&report), Vec::<String>::new());
}

#[test]
fn the_wheel_drag_must_move_the_zoom_and_play_a_plausible_number_of_haptics() {
    let mut report = healthy_report();
    report.steps[2].screen_ratio = 1.0;
    assert_single(&violations_of(&report), "не сдвинула зум");

    let mut report = healthy_report();
    report.steps[2].haptics = Some(HapticWindow {
        count: 1,
        window_ms: 1_200.0,
    });
    assert_single(&violations_of(&report), "вибраций за протяжку 1 при минимуме");

    // Окно 1200 мс при ограничителе 30 мс пропускает не больше 41 вибрации.
    let mut report = healthy_report();
    report.steps[2].haptics = Some(HapticWindow {
        count: 42,
        window_ms: 1_200.0,
    });
    assert_single(&violations_of(&report), "при потолке 41");
}

#[test]
fn haptic_gaps_are_not_judged_so_two_ticks_in_one_ui_frame_cannot_false_fail() {
    // Прежняя проверка промежутка роняла прогон, когда два образца попадали в один кадр UI (0 мс на часах
    // исполнителя). Теперь судится только число вибраций.
    let mut report = healthy_report();
    report.steps[2].haptics = Some(HapticWindow {
        count: 15,
        window_ms: 480.0,
    });
    assert_eq!(violations_of(&report), Vec::<String>::new());
}

#[test]
fn a_reveal_by_timeout_fails_the_step_and_the_run_with_a_clear_reason() {
    let mut report = healthy_report();
    report.steps[0].reveal_faults.revealed_by_timeout = 1;
    report.totals.reveal_faults.revealed_by_timeout = 1;
    let violations = violations_of(&report);
    assert_eq!(violations.len(), 2, "про шаг и про прогон: {violations:?}");
    assert!(
        violations
            .iter()
            .any(|text| text.starts_with("01_tap_0.6: переход не раскрылся по условию")
                && text.contains("раскрытий по сроку")),
        "{violations:?}"
    );
    assert!(
        violations.iter().any(|text| text.contains("шаги: 01_tap_0.6")
            && text.contains("раскрытий по сроку вместо условия готовности: 1")),
        "{violations:?}"
    );
}

#[test]
fn an_honest_black_ending_fails_even_when_no_black_frame_was_expected() {
    for (name, faults) in [
        (
            "picture_unavailable",
            RevealFaults {
                black_picture_unavailable: 3,
                ..RevealFaults::default()
            },
        ),
        (
            "camera_unavailable",
            RevealFaults {
                black_camera_unavailable: 3,
                ..RevealFaults::default()
            },
        ),
        (
            "ended_without_presentable_frame",
            RevealFaults {
                ended_without_presentable_frame: 1,
                ..RevealFaults::default()
            },
        ),
        (
            "ended_without_stream",
            RevealFaults {
                ended_without_stream: 1,
                ..RevealFaults::default()
            },
        ),
    ] {
        let mut report = healthy_report();
        report.steps[1].reveal_faults = faults;
        report.totals.reveal_faults = faults;
        let violations = violations_of(&report);
        assert_eq!(violations.len(), 2, "{name}: {violations:?}");
        assert!(
            violations.iter().all(|text| text.contains("не раскрылся по условию")),
            "{name}: {violations:?}"
        );
        assert_eq!(
            report.totals.black_frames_while_picture_expected, 0,
            "{name}: счётчик «ждали картинку» молчит — прежний вердикт пропускал это"
        );
    }
}

#[test]
fn no_published_lens_intents_fails_the_run() {
    let mut report = healthy_report();
    report.totals.lens_intents_published = 0;
    assert_single(&violations_of(&report), "ни одного намерения");
}

#[test]
fn the_screen_must_be_left_on_one_x_of_the_main_camera() {
    for broken in [
        FinalState {
            ratio_milli: 590,
            facing: LensFacing::Back,
            active_role: Some(LensRole::Wide),
        },
        FinalState {
            ratio_milli: 1_000,
            facing: LensFacing::Front,
            active_role: Some(LensRole::Wide),
        },
        FinalState {
            ratio_milli: 1_000,
            facing: LensFacing::Back,
            active_role: Some(LensRole::UltraWide),
        },
        FinalState {
            ratio_milli: 1_000,
            facing: LensFacing::Back,
            active_role: None,
        },
    ] {
        let mut report = healthy_report();
        report.final_state = Some(broken);
        assert_single(&violations_of(&report), "не на 1× основной камеры");
    }
    let mut near = healthy_report();
    near.final_state = Some(FinalState {
        ratio_milli: 1_015,
        facing: LensFacing::Back,
        active_role: Some(LensRole::Wide),
    });
    assert_eq!(violations_of(&near), Vec::<String>::new(), "в допуске 2 % — норма");
}

#[test]
fn a_healthy_switch_step_alone_has_no_violations() {
    assert_eq!(
        step_violations(&config(), &healthy_switch_step("x")),
        Vec::<String>::new()
    );
}

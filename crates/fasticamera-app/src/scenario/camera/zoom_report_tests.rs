//! Отчёт `zoom`: сводка компактна и несёт нужные числа, подробности — по записи на шаг, худший случай влезает в строку.

use render_loop::viewfinder::ContinuityCounters;

use super::*;
use crate::scenario::camera::result_budget::judge_within_budget;
use crate::scenario::camera::result_budget::worst_case::assert_failure_line_fits;
use crate::scenario::camera::zoom_test_support::{healthy_report, healthy_switch_step};

#[test]
fn summary_carries_the_numbers_the_phone_check_reads() {
    let summary = healthy_report().summary();
    assert_eq!(summary["steps_planned"], 3);
    assert_eq!(summary["steps_done"], 3);
    assert_eq!(summary["switches_observed"], 1);
    assert_eq!(summary["max_switch_ms"], 400);
    assert_eq!(summary["max_first_live_frame_ms"], 520.0);
    assert_eq!(summary["black_frames_while_picture_expected"], 0);
    assert_eq!(summary["final"]["ratio_milli"], 1_000);
    assert_eq!(summary["final"]["facing"], "back");
    assert_eq!(summary["final"]["role"], "wide");
}

#[test]
fn details_have_totals_first_and_one_record_per_step() {
    let report = healthy_report();
    let items = report.detail_items();
    assert_eq!(items.len(), 1 + report.steps.len());
    assert!(items[0].get("totals").is_some());
    assert_eq!(items[1]["name"], "01_tap_0.6");
    assert_eq!(items[3]["haptics"]["count"], 15);
}

#[test]
fn totals_take_continuity_deltas_not_absolute_counters() {
    let start = ContinuityCounters {
        held_frames_presented: 10,
        black_frames_while_picture_expected: 0,
        holds_revealed_by_timeout: 1,
        black_frames_picture_unavailable: 4,
        ..ContinuityCounters::default()
    };
    let end = ContinuityCounters {
        held_frames_presented: 70,
        black_frames_while_picture_expected: 2,
        holds_revealed_by_timeout: 1,
        black_frames_picture_unavailable: 6,
        ..ContinuityCounters::default()
    };
    let totals = ZoomTotals::default().with_continuity(&start, &end);
    assert_eq!(
        (totals.held_frames, totals.black_frames_while_picture_expected),
        (60, 2)
    );
    assert_eq!(
        totals.reveal_faults,
        RevealFaults {
            black_picture_unavailable: 2,
            ..RevealFaults::default()
        },
        "прирост считается от начала окна: старый таймаут (1 → 1) не в счёт"
    );
}

#[test]
fn max_times_ignore_steps_without_a_switch() {
    let mut report = healthy_report();
    report.steps.push(healthy_switch_step("04_x"));
    report.steps[3].switch = None;
    report.steps[3].first_live_frame_ms = None;
    assert_eq!(report.max_switch_ms(), Some(400));
    assert_eq!(report.max_first_live_frame_ms(), Some(520.0));
}

#[test]
fn the_worst_failing_summary_fits_the_result_line() {
    let mut report = full_plan_report();
    // Худший случай: длинные кириллические причины в сводке и в нарушениях.
    report.run_failure = Some(crate::scenario::camera::result_budget::worst_case::long_reason("zoom"));
    // Кэш вариантов пайплайна на пределе разрядов и с заметкой в сводке: строка всё равно обязана влезть.
    report.totals.pipeline_variants = crate::scenario::camera::pipeline_variants_report::PipelineVariantsReport {
        in_cache: usize::MAX,
        created_in_frame: u64::MAX,
        created_by_prewarm: u64::MAX,
        prewarm_failed: u64::MAX,
        create_failed_in_frame: u64::MAX,
        evicted: u64::MAX,
        last_frame_compile_us: u64::MAX,
        longest_frame_compile_us: u64::MAX,
        last_prewarm_us: u64::MAX,
        longest_prewarm_us: u64::MAX,
        bg_created: u64::MAX,
        frame_while_bg: u64::MAX,
        bg_waited: u64::MAX,
        stall_max_us: u64::MAX,
        bg_done_us: Some(u64::MAX),
        bg_aborted: true,
        bg_no_thread: true,
        pc: "oversized",
    };
    assert!(
        !report.summary()["notes"].as_array().unwrap().is_empty(),
        "заметка присутствует"
    );
    let violations: Vec<String> = (0..12)
        .map(|index| crate::scenario::camera::result_budget::worst_case::long_reason(&format!("шаг {index}")))
        .collect();
    let verdict = judge_within_budget(report.summary(), violations);
    assert_failure_line_fits("zoom", verdict.outcome);
}

#[test]
fn black_frames_before_first_picture_grow_into_totals_and_the_summary() {
    let start = ContinuityCounters {
        black_frames_before_first_picture: 5,
        ..ContinuityCounters::default()
    };
    let end = ContinuityCounters {
        black_frames_before_first_picture: 9,
        ..ContinuityCounters::default()
    };
    let mut report = healthy_report();
    report.totals = ZoomTotals::default().with_continuity(&start, &end);
    assert_eq!(
        report.totals.black_frames_before_first_picture, 4,
        "холодный старт до окна не в счёт"
    );
    assert_eq!(report.summary()["black_frames_before_first_picture"], 4);
}

/// Отчёт полного плана: 5 тапов, две ноги переворота, протяжка, три тапа 1 и все бесшовные шаги (по именам плана).
fn full_plan_report() -> ZoomObservationReport {
    use crate::scenario::camera::zoom_test_support::{
        healthy_crop_step, healthy_release_step, healthy_seamless_switch_step, healthy_switch_step,
    };
    let mut report = healthy_report();
    let mut steps = vec![
        healthy_switch_step("01_tap_0.6"),
        healthy_switch_step("02_tap_1"),
        healthy_crop_step("03_tap_2"),
        healthy_switch_step("04_tap_1"),
        healthy_switch_step("05_tap_0.6"),
        healthy_switch_step("06_flip_to_front"),
        healthy_switch_step("07_flip_to_back"),
    ];
    for (name, leg) in [
        ("11_wheel_seamless_out", "out"),
        ("12_wheel_seamless_back", "back"),
        ("14_pinch_seamless_out", "out"),
        ("15_pinch_seamless_back", "back"),
    ] {
        steps.push(healthy_seamless_switch_step(name, leg));
    }
    steps.push(healthy_release_step("13_wheel_seamless_release"));
    steps.push(healthy_release_step("16_pinch_seamless_release"));
    report.steps = steps;
    report
}

#[test]
fn the_summary_lists_every_seamless_step_and_the_hidden_frames_of_every_tap() {
    let mut report = full_plan_report();
    report.steps[0].continuity.frames_hidden_by_continuity = 2;
    report.steps[1].continuity.frames_hidden_by_continuity = 9;
    let summary = report.summary();

    let seamless = summary["seamless"].as_array().expect("список бесшовных шагов");
    assert_eq!(seamless.len(), 6);
    assert_eq!(seamless[0]["step"], "11_wheel_seamless_out");
    assert_eq!(seamless[0]["method"], "promoted_standby");
    assert_eq!(seamless[0]["held"], 0);
    assert_eq!(seamless[0]["hidden"], 0);
    assert_eq!(seamless[0]["fps_min"], 59.2);
    assert_eq!(seamless[4]["closed_ms"], 3700.0);

    let taps = summary["tap_hidden"].as_array().expect("тапы");
    assert_eq!(taps.len(), 5, "только кнопки ленты, без переворота и бесшовных шагов");
    assert_eq!(taps[0], serde_json::json!({"step": "01_tap_0.6", "hidden": 2}));
    assert_eq!(taps[1]["hidden"], 9);
}

#[test]
fn the_summary_reports_the_swap_count_and_the_screen_ratio_of_the_switch_legs() {
    let mut report = full_plan_report();
    let mut hover = crate::scenario::camera::zoom_test_support::healthy_release_step("13_wheel_seamless_hover");
    {
        let seamless = hover.seamless.as_mut().expect("запись");
        seamless.leg = "hover";
        seamless.switch_events = 7;
    }
    report.steps.push(hover);
    let summary = report.summary();
    let entries = summary["seamless"].as_array().expect("список бесшовных шагов");
    let hover_entry = entries.last().expect("колебания в сводке");
    assert_eq!(
        (hover_entry["leg"].as_str(), hover_entry["events"].as_u64()),
        (Some("hover"), Some(7))
    );
    let back = &entries[1];
    assert_eq!(
        (back["tgt_milli"].as_u64(), back["sw_milli"].as_u64()),
        (Some(1000), Some(1004))
    );
}

/// Создание варианта пайплайна в кадре после прогрева — заметка в сводке, а вердикт не меняется (это не провал).
#[test]
fn pipeline_creation_in_a_frame_within_the_switch_allowance_is_a_note_and_not_a_violation() {
    use crate::scenario::camera::pipeline_variants_report::PipelineVariantsReport;
    use crate::scenario::camera::zoom_verdict::evaluate_zoom;
    use crate::zoom_scenario_config::ZoomScenarioConfig;

    let calm = healthy_report();
    assert_eq!(
        calm.summary()["notes"],
        serde_json::json!([]),
        "тихий прогон без заметок"
    );

    let mut noisy = healthy_report();
    // По одному созданию на смену объектива — законно (см. вердикт): две смены покрывают два создания.
    noisy.totals.switches_completed = 2;
    noisy.totals.pipeline_variants = PipelineVariantsReport {
        created_in_frame: 2,
        longest_frame_compile_us: 41_000,
        ..PipelineVariantsReport::default()
    };
    let summary = noisy.summary();
    assert_eq!(summary["variants_created_in_frame"], 2);
    // Полный отчёт кэша — в подробностях (`totals`), а не в строке результата.
    let details = noisy.detail_items();
    assert_eq!(
        details[0]["totals"]["pipeline_variants"]["longest_frame_compile_us"],
        41_000
    );
    let notes = summary["notes"].as_array().expect("заметки — список");
    assert_eq!(notes.len(), 1);
    assert!(notes[0].as_str().unwrap().contains("создано в кадре 2"), "{notes:?}");
    // Вердикт тот же, что у тихого прогона.
    let config = ZoomScenarioConfig::default();
    assert_eq!(evaluate_zoom(&config, &noisy), evaluate_zoom(&config, &calm));
}

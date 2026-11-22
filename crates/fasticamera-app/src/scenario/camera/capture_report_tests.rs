//! Отчёт `capture`: адреса файлов и их место в строке результата, укладывание в 4000 байт в худшем случае.

use super::super::capture_frame_timing::FrameChoiceView;
use super::super::capture_render_still::StillDiagnosticsDelta;
use super::super::capture_test_support::healthy_report;
use super::*;
use crate::logging::record::LOGCAT_MAX_TEXT_BYTES;
use crate::result_line::format_result_line;
use crate::scenario::ScenarioReport;

#[test]
fn uris_list_every_saved_file_and_skip_unsaved_shots() {
    let mut report = healthy_report();
    assert_eq!(report.uris().len(), 15, "1 + 2 + 1 + 1 + 10 файлов");
    report.shots[0].outcome = ShotOutcome::Failed {
        failure: "x".to_owned(),
    };
    report.shots[2].outcome = ShotOutcome::Unfinished { secured: true };

    let uris = report.uris();

    assert_eq!(uris.len(), 13);
    assert!(
        uris.contains(&"content://media/external_primary/file/2"),
        "DNG второго снимка в списке: {uris:?}"
    );
    assert!(
        !uris.iter().any(|uri| uri.ends_with("/images/media/1")),
        "сорванный снимок без адреса"
    );
}

#[test]
fn the_summary_carries_the_uris_counts_and_the_burst_accounting() {
    let summary = healthy_report().summary();

    assert_eq!(summary["shots_total"], 14);
    assert_eq!(summary["shots_saved"], 14);
    assert_eq!(summary["uris"].as_array().expect("адреса").len(), 15);
    assert_eq!(summary["burst"]["presses"], 10);
    assert_eq!(summary["burst"]["saved"], 10);
    assert_eq!(summary["mode"]["mode"], "real");
    assert_eq!(summary["full_frame"]["width_px"], 4_096);
    assert_eq!(summary["shots"][1]["outcome"], "saved_with_raw");
    assert_eq!(summary["shots"][0]["secured_ms"], 30);
}

#[test]
fn the_summary_omits_defaults_and_names_the_flash_shot_with_the_camera_report() {
    let summary = healthy_report().summary();
    let plain = summary["shots"][0].as_object().expect("снимок");
    for key in ["fmt", "aspect", "zoom_milli", "flash", "choice", "flash_cam"] {
        assert!(!plain.contains_key(key), "умолчание {key} не пишется: {plain:?}");
    }
    let flash = &summary["shots"][2];
    assert_eq!(flash["flash"], "on");
    assert_eq!(flash["choice"], "flash_frame");
    assert_eq!(flash["flash_cam"]["state"], "fired");
    assert_eq!(flash["flash_cam"]["iso"], 400);
    assert_eq!(summary["shots"][3]["flash"], "auto");
    assert!(
        summary["shots"][3].get("choice").is_none(),
        "Auto не вспыхнул: показанный кадр — умолчание"
    );
    assert_eq!(summary["frames"]["flash"], 1);
}

#[test]
fn the_details_carry_the_frame_summary_the_render_diagnostics_place_and_per_shot_choice() {
    let mut report = healthy_report();
    let mut delta = StillDiagnosticsDelta::default();
    delta.picks.exact = 14;
    report.render_still_detail = Some(delta);
    let items = report.detail_items();

    assert_eq!(items[0]["frame_summary"]["exact"], 14);
    assert_eq!(items[0]["frame_summary"]["max_press_to_frame_ms"], 80.0);
    assert_eq!(items[0]["render_still_detail"]["picks"]["exact"], 14);
    // Затем шаги, серия и 14 снимков с выбором кадра и задержкой.
    assert_eq!(items.len(), 1 + 5 + 1 + 14);
    let shot = items.last().expect("снимок");
    assert_eq!(shot["frame_choice"]["kind"], "exact");
    assert_eq!(shot["press_to_frame_ms"], 80.0);
    assert_eq!(shot["shown_before_press_ms"], 10.0);
}

#[test]
fn the_summary_reports_the_exact_share_the_delay_and_a_substitution_reason_stays_in_the_details() {
    let mut report = healthy_report();
    report.shots[0].frame.frame_choice = FrameChoiceView::Substituted {
        reason: still_contract::frame_choice::SubstitutionReason::ShownFrameEvicted,
    };
    let summary = report.summary();

    assert_eq!(summary["frames"]["exact"], 13);
    assert_eq!(summary["frames"]["evicted"], 1);
    assert_eq!(summary["frames"]["p2f_max_ms"], 80.0);
    assert!((summary["frames"]["share"].as_f64().unwrap() - 13.0 / 14.0).abs() < 1e-3);
    let first = &report.detail_items()[1 + 5 + 1];
    assert_eq!(first["frame_choice"]["reason"], "shown_frame_evicted");
}

#[test]
fn the_press_to_thumbnail_time_is_in_the_summary_only_for_shots_that_have_it() {
    let mut report = healthy_report();
    report.shots[0].press_to_thumbnail_ms = Some(310);
    report.shots[1].press_to_thumbnail_ms = None;
    let burst = report
        .shots
        .iter()
        .position(|shot| shot.step == STEP_BURST)
        .expect("в отчёте есть серия");
    report.shots[burst].press_to_thumbnail_ms = Some(777);

    let summary = report.summary();

    assert_eq!(summary["shots"][0]["thumb_ms"], 310);
    assert!(summary["shots"][1].get("thumb_ms").is_none(), "нет замера — нет ключа");
    assert!(
        summary["shots"][burst].get("thumb_ms").is_none(),
        "снимки серии в сводку не идут (строка результата ограничена)"
    );
    let details = report.detail_items();
    assert!(
        details.iter().any(|item| item["press_to_thumbnail_ms"] == 310),
        "в подробностях полное имя поля"
    );
    assert!(
        details.iter().any(|item| item["press_to_thumbnail_ms"] == 777),
        "замер снимка серии лежит в подробностях"
    );
}

#[test]
fn a_healthy_summary_with_frame_numbers_still_fits_the_logcat_record() {
    use super::super::result_budget::judge_within_budget;
    let verdict = judge_within_budget(healthy_report().summary(), Vec::new());
    let (line, _) = format_result_line(&ScenarioReport {
        scenario: "capture".to_owned(),
        outcome: verdict.outcome,
    });
    assert!(line.len() <= LOGCAT_MAX_TEXT_BYTES, "строка {} байт", line.len());
}

/// Худший случай: все 13 адресов длинные, у снимков сорванные тексты, режим — запасной фейк с длинной причиной.
#[test]
fn the_worst_case_result_line_with_thirteen_long_uris_fits_the_logcat_record() {
    use super::super::result_budget::judge_within_budget;

    let mut report = healthy_report();
    let long_uri = |index: usize| {
        format!(
            "content://media/external_primary/images/media/{index}{}",
            "9".repeat(12)
        )
    };
    for (index, shot) in report.shots.iter_mut().enumerate() {
        shot.outcome = ShotOutcome::Saved {
            primary: long_uri(index),
            raw: (index == 1).then(|| long_uri(100 + index)),
        };
        // Замер «нажатие → картинка» (14g) есть у каждого снимка: в сводку из них попадают одиночные, и они обязаны
        // уместиться в запись даже в худшем случае.
        shot.press_to_thumbnail_ms = Some(1_234);
    }
    report.mode_at_start = CaptureMode::FakeFallback {
        reason: "отказ драйвера при записи прохода ".repeat(40),
    };
    report.run_failure = Some("значок RAW не переключил настройку ".repeat(30));
    let violations: Vec<String> = (0..40)
        .map(|index| format!("нарушение {index}: {}", "причина ".repeat(60)))
        .collect();

    let verdict = judge_within_budget(report.summary(), violations);

    let (line, json_text) = format_result_line(&ScenarioReport {
        scenario: "capture".to_owned(),
        outcome: verdict.outcome,
    });

    let json: serde_json::Value = serde_json::from_str(&json_text).expect("строка результата — JSON");

    assert!(line.len() <= LOGCAT_MAX_TEXT_BYTES, "строка {} байт", line.len());
    assert_eq!(
        json["status"], "failed",
        "это провал со сводкой, а не отчёт «не помещается»: {line}"
    );
    assert_eq!(
        json["report"]["uris"].as_array().expect("адреса в сводке").len(),
        15,
        "адреса не потеряны"
    );
}

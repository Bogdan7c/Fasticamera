//! Отчёт `capture`: адреса файлов и их место в строке результата, укладывание в 4000 байт в худшем случае.

use super::super::capture_test_support::healthy_report;
use super::*;
use crate::logging::record::LOGCAT_MAX_TEXT_BYTES;
use crate::result_line::format_result_line;
use crate::scenario::ScenarioReport;

#[test]
fn uris_list_every_saved_file_and_skip_unsaved_shots() {
    let mut report = healthy_report();
    assert_eq!(report.uris().len(), 13, "1 + 2 + 10 файлов");
    report.shots[0].outcome = ShotOutcome::Failed {
        failure: "x".to_owned(),
    };
    report.shots[2].outcome = ShotOutcome::Unfinished { secured: true };

    let uris = report.uris();

    assert_eq!(uris.len(), 11);
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

    assert_eq!(summary["shots_total"], 12);
    assert_eq!(summary["shots_saved"], 12);
    assert_eq!(summary["uris"].as_array().expect("адреса").len(), 13);
    assert_eq!(summary["burst"]["presses"], 10);
    assert_eq!(summary["burst"]["saved"], 10);
    assert_eq!(summary["mode"]["mode"], "real");
    assert_eq!(summary["full_frame"]["width_px"], 4_096);
    assert_eq!(summary["shots"][1]["outcome"], "saved_with_raw");
    assert_eq!(summary["shots"][0]["secured_ms"], 30);
}

#[test]
fn the_details_name_the_frame_choice_gap_and_keep_a_reserved_place_for_render_diagnostics() {
    let items = healthy_report().detail_items();

    assert_eq!(items[0]["frame_choice_note"], FRAME_CHOICE_NOTE);
    assert!(items[0]["render_still_detail"].is_null());
    assert!(
        items[0]
            .as_object()
            .expect("объект")
            .contains_key("render_still_detail")
    );
    // Затем шаги, серия и 12 снимков с null в поле выбора кадра.
    assert_eq!(items.len(), 1 + 3 + 1 + 12);
    let shot = items.last().expect("снимок");
    assert!(shot["frame_choice"].is_null() && shot.as_object().expect("объект").contains_key("frame_choice"));
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
        13,
        "адреса не потеряны"
    );
}

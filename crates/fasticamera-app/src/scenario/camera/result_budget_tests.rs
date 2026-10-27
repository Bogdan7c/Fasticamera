//! Бюджет строки результата: укорачивание по границе символа, сжатие нарушений, списки.

use super::*;

#[test]
fn truncation_respects_utf8_boundaries_and_the_byte_limit() {
    let cyrillic = "ж".repeat(100); // 200 байт
    for limit in [0, 1, 2, 3, 4, 5, 50, 51, 199] {
        let shortened = truncate_text(&cyrillic, limit);
        assert!(
            shortened.len() <= limit.max(ELLIPSIS.len()),
            "{limit}: {} байт",
            shortened.len()
        );
        assert!(shortened.ends_with(ELLIPSIS), "{limit}: обрезка помечена");
    }
    assert_eq!(truncate_text(&cyrillic, 200), cyrillic, "влезает — без изменений");
    assert_eq!(truncate_text("коротко", 100), "коротко");
}

#[test]
fn many_violations_keep_the_first_ones_and_count_the_rest() {
    let violations: Vec<String> = (0..50)
        .map(|index| format!("окно {index}: нарушение {}", "ы".repeat(60)))
        .collect();
    let verdict = judge_within_budget(serde_json::json!({"ok": 1}), violations.clone());
    let ScenarioOutcome::FailedWithReport { error, report } = verdict.outcome else {
        panic!("провал с отчётом");
    };
    assert!(error.len() <= MAX_ERROR_BYTES, "{} байт", error.len());
    assert!(error.starts_with("окно 0:"), "первые нарушения целиком: {error}");
    assert!(error.contains("…ещё "), "{error}");
    assert_eq!(report["ok"], 1);
    assert_eq!(
        verdict.violation_details.len(),
        50,
        "полный список — в detail_violations"
    );
}

#[test]
fn few_short_violations_lose_nothing_and_need_no_details() {
    let verdict = judge_within_budget(Value::Null, vec!["a".to_owned(), "b".to_owned()]);
    let ScenarioOutcome::FailedWithReport { error, .. } = verdict.outcome else {
        panic!("провал с отчётом");
    };
    assert_eq!(error, "a; b", "как прежде у `judged`");
    assert!(verdict.violation_details.is_empty());
}

#[test]
fn no_violations_pass_and_summary_strings_are_bounded() {
    let summary = serde_json::json!({ "nested": [{ "reason": worst_case::long_reason("x") }] });
    let verdict = judge_within_budget(summary, Vec::new());
    let ScenarioOutcome::Passed { report } = verdict.outcome else {
        panic!("прошёл");
    };
    let reason = report["nested"][0]["reason"].as_str().expect("строка");
    assert!(reason.len() <= MAX_SUMMARY_TEXT_BYTES, "{} байт", reason.len());
}

#[test]
fn bounded_list_reports_the_count_and_the_first_items() {
    let items: Vec<String> = (0..20).map(|index| format!("регулировка {index}")).collect();
    let list = bounded_list(&items);
    assert_eq!(list["count"], 20);
    assert_eq!(list["first"].as_array().map(Vec::len), Some(MAX_LIST_ITEMS_IN_SUMMARY));
}

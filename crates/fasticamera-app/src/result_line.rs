//! Строка результата smoke-сценария: `FASTICAMERA_RESULT {json}` одной записью logcat.
//!
//! Это контракт с `scripts/device-smoke.sh`: строка не режется на куски, поэтому её длина
//! ограничена одной записью logcat. Слишком длинный отчёт заменяется отчётом об ошибке —
//! тоже валидной строкой, чтобы скрипт получил ясный провал, а не обрывок JSON.

use crate::logging::record::LOGCAT_MAX_TEXT_BYTES;
use crate::logging::{LogPriority, emit_unfiltered};
use crate::scenario::{ScenarioOutcome, ScenarioReport};

/// Префикс строки результата в logcat.
pub const RESULT_LINE_PREFIX: &str = "FASTICAMERA_RESULT ";

/// Собирает строку результата и JSON отчёта (его же Kotlin показывает на экране).
pub fn format_result_line(report: &ScenarioReport) -> (String, String) {
    let json = serialize_within_limit(report);
    (format!("{RESULT_LINE_PREFIX}{json}"), json)
}

/// Пишет строку результата в logcat мимо фильтра и возвращает JSON отчёта.
pub fn emit_result_line(report: &ScenarioReport) -> String {
    let (line, json) = format_result_line(report);
    emit_unfiltered(LogPriority::Info, line);
    json
}

fn serialize_within_limit(report: &ScenarioReport) -> String {
    let max_json_bytes = LOGCAT_MAX_TEXT_BYTES - RESULT_LINE_PREFIX.len();
    let replacement = match serde_json::to_string(report) {
        Ok(json) if json.len() <= max_json_bytes => return json,
        Ok(json) => format!(
            "отчёт {} байт не помещается в строку logcat ({max_json_bytes} байт)",
            json.len()
        ),
        Err(error) => format!("отчёт не сериализован: {error}"),
    };
    let failure = ScenarioReport {
        scenario: report.scenario.clone(),
        outcome: ScenarioOutcome::Failed { error: replacement },
    };
    // Отчёт из двух коротких строк сериализуется всегда; запасной JSON — на невозможный сбой,
    // чтобы скрипт всё равно получил разбираемую строку.
    serde_json::to_string(&failure)
        .unwrap_or_else(|_| r#"{"scenario":"?","status":"failed","error":"result_line_serialization"}"#.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn passed(report: serde_json::Value) -> ScenarioReport {
        ScenarioReport {
            scenario: "hello".to_owned(),
            outcome: ScenarioOutcome::Passed { report },
        }
    }

    #[test]
    fn line_has_prefix_and_round_trips_json() {
        let (line, json) = format_result_line(&passed(serde_json::json!({"abi": "arm64-v8a"})));
        let payload = line.strip_prefix(RESULT_LINE_PREFIX).expect("префикс");
        assert_eq!(payload, json);
        let parsed: serde_json::Value = serde_json::from_str(payload).expect("json");
        assert_eq!(parsed["status"], "passed");
        assert_eq!(parsed["report"]["abi"], "arm64-v8a");
    }

    #[test]
    fn oversized_report_becomes_failure_line_within_limit() {
        let huge = "x".repeat(LOGCAT_MAX_TEXT_BYTES);
        let (line, json) = format_result_line(&passed(serde_json::json!({ "blob": huge })));
        assert!(line.len() <= LOGCAT_MAX_TEXT_BYTES, "длина {}", line.len());
        let parsed: serde_json::Value = serde_json::from_str(&json).expect("json");
        assert_eq!(parsed["scenario"], "hello");
        assert_eq!(parsed["status"], "failed");
        assert!(parsed["error"].as_str().expect("error").contains("не помещается"));
    }
}

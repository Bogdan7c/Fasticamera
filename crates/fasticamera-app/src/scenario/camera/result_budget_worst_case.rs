//! Худшие случаи провальных прогонов для тестов бюджета строки результата: длинные причины на
//! кириллице во всех текстовых полях и проверка напечатанной строки целиком.

use serde_json::Value;

use super::super::engine_watch::RunFailure;
use super::super::raw_stream_report::DegradedNote;
use super::super::stream_report::ConfiguredEntry;
use super::super::viewfinder_session::SessionSummary;
use crate::logging::record::LOGCAT_MAX_TEXT_BYTES;
use crate::result_line::{RESULT_LINE_PREFIX, format_result_line};
use crate::scenario::{ScenarioOutcome, ScenarioReport};

/// Длинная причина с кириллицей (худший случай свободного текста): ~2,5 КБ.
pub fn long_reason(label: &str) -> String {
    format!("{label}: {}", "отказ драйвера при записи прохода ".repeat(40))
}

/// Отказ прогона с длинным текстом (он попадает и в сводку, и в нарушения).
pub fn long_failure() -> RunFailure {
    RunFailure::EngineFatal {
        cause: long_reason("движок"),
    }
}

/// Сводка сессии со всеми необязательными полями и длинными строками.
pub fn worst_session() -> SessionSummary {
    SessionSummary {
        configured: Some(ConfiguredEntry {
            camera: long_reason("камера"),
            raw_format: "raw10",
            buffer_width: 4_096,
            buffer_height: 3_072,
            frame_rate: 60,
            lever: "hal_default",
            lever_detail: camera_contract::stream::FrameRateMechanism::HalDefault,
        }),
        degraded: Some(DegradedNote {
            requested_fps: 60,
            fallback_fps: 30,
            failure: long_reason("откат частоты"),
        }),
        time_to_streaming_ms: Some(900),
        time_to_live_ms: Some(1_100),
        state_at_end: "streaming",
        events: Vec::new(),
    }
}

/// Печатает строку результата, как её напечатал бы сценарий, и проверяет: влезает в запись
/// logcat, не заменена отчётом «не помещается», это провал со сводкой. Возвращает JSON строки.
pub fn assert_failure_line_fits(scenario: &str, outcome: ScenarioOutcome) -> Value {
    let report = ScenarioReport {
        scenario: scenario.to_owned(),
        outcome,
    };
    let (line, json) = format_result_line(&report);
    assert!(
        line.len() <= LOGCAT_MAX_TEXT_BYTES,
        "{scenario}: строка {} байт",
        line.len()
    );
    assert!(line.starts_with(RESULT_LINE_PREFIX));
    let parsed: Value = serde_json::from_str(&json).expect("JSON строки результата");
    assert_eq!(parsed["status"], "failed", "{scenario}: {parsed}");
    assert!(
        parsed.get("report").is_some_and(Value::is_object),
        "{scenario}: отчёт не заменён сообщением о размере: {parsed}"
    );
    parsed
}

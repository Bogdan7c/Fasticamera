//! Сводка результата стенда насыщения: компактная таблица строки результата и подробности по работам.
//!
//! Строка `FASTICAMERA_RESULT` — одна запись logcat (~4000 байт), поэтому в ней одна КОРОТКАЯ строка на работу
//! (`имя|к/с|мс/кадр|вердикт`), а не одна длинная строка таблицы (длинные строки сводки укорачиваются до 160 байт).
//! Причины провалов, проходы, шкала, тепло и состав сцены уходят подробностями (`FASTICAMERA_DETAIL`) по работе.

use serde::Serialize;
use serde_json::{Value, json};

use super::inputs::{LookSummary, SensorSource};
use super::report::{RenderSilence, RunSummary, WorkloadRow, thermal_word};
use crate::gpu_services::ValidationState;

/// Что стало с камерой на время замера.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CameraSilence {
    /// Сценарий закрыл камеру и дождался `Closed` (она вернётся по концу сценария).
    ClosedByScenario,
    /// Движка камеры нет: стримить было нечему.
    EngineUnavailable,
}

/// Общие факты прогона для сводки.
#[derive(Debug, Clone)]
pub struct SummaryHeader {
    pub validation: ValidationState,
    pub sensor: SensorSource,
    pub look: LookSummary,
    pub camera: CameraSilence,
    /// Запрошенные работы в порядке списка: скрипт ПК сверяет по ним таблицу (ни одна не пропала).
    pub requested: Vec<&'static str>,
    pub warmup_ms: u64,
    pub measure_ms: u64,
    pub frames_in_flight: usize,
    /// Допуск продуктовой работы GPU потока рендера за замер, кадров (из конфига): по нему скрипт ПК перепроверяет вердикт.
    pub render_tolerance_frames: u64,
}

/// Число с тремя значащими цифрами: `123`, `45.6`, `7.89`, `0.123`.
pub fn format_sig3(value: f64) -> String {
    if !value.is_finite() || value == 0.0 {
        return format!("{value}");
    }
    let magnitude = value.abs().log10().floor() as i32;
    let decimals = (2 - magnitude).clamp(0, 12) as usize;
    format!("{value:.decimals$}")
}

/// Строка компактной таблицы: `имя|к/с|мс/кадр|вердикт`.
pub fn table_row(row: &WorkloadRow) -> String {
    let (fps, cost) = match row.measurement() {
        Some(measurement) => (
            measurement
                .frames_per_second
                .map_or_else(|| "-".to_owned(), format_sig3),
            measurement.frame_cost_ms.map_or_else(|| "-".to_owned(), format_sig3),
        ),
        None => ("-".to_owned(), "-".to_owned()),
    };
    format!("{}|{}|{}|{}", row.name, fps, cost, row.verdict_word())
}

/// Сводка для строки результата (укладывается в бюджет по построению: строка на работу короткая).
pub fn build_summary(header: &SummaryHeader, run: &RunSummary, silence: &RenderSilence) -> Value {
    json!({
        "table": run.rows.iter().map(table_row).collect::<Vec<_>>(),
        "requested": header.requested,
        "workloads": run.rows.len(),
        "measured": run.rows.iter().filter(|row| row.is_measured()).count(),
        "cancelled": run.cancelled,
        "validation": header.validation,
        "sensor": header.sensor,
        "look": header.look,
        "camera": header.camera,
        "warmup_ms": header.warmup_ms,
        "measure_ms": header.measure_ms,
        "frames_in_flight": header.frames_in_flight,
        "render_tolerance_frames": header.render_tolerance_frames,
        "render": silence,
    })
}

/// Полная строка одной работы для `FASTICAMERA_DETAIL`.
pub fn detail_item(row: &WorkloadRow) -> Value {
    let mut item =
        serde_json::to_value(row).unwrap_or_else(|error| json!({ "serialization_error": error.to_string() }));
    if let Value::Object(map) = &mut item
        && let (Some(start), Some(end)) = (&row.thermal_start, &row.thermal_end)
    {
        map.insert(
            "thermal_words".to_owned(),
            json!({ "start": thermal_word(start), "end": thermal_word(end) }),
        );
    }
    item
}

#[cfg(test)]
#[path = "summary_tests.rs"]
mod tests;

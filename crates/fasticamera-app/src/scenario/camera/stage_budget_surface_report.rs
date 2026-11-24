//! Часть отчёта `viewfinder-stage-budget` про место замера (сессия 16a): что было на экране, какой прибор считался,
//! база «камера без нашей работы GPU и UI» и компактные заметки по строкам (тепло, частота показа).
//!
//! Отдельно от `viewfinder_stage_budget_report`, чтобы тот не вырос: там таблица строк, здесь всё, что относится к
//! месту замера и условиям окон.

use render_loop::viewfinder::ViewfinderDiagnostics;
use serde::Serialize;
use serde_json::Value;

use super::stage_budget_window::WindowCondition;
use super::viewfinder_metrics::{GpuTimeMs, round2};
use super::viewfinder_stage_budget_report::{StageBudgetRow, WHOLE_PRODUCT_ROW};
use crate::power_measurement::{StageSetting, ThermalSnapshot};
use crate::stage_budget_surface_config::StageBudgetSurface;

/// Прибор, каким его видел рендер при входе на место замера (только экран калибровки).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ScopesReport {
    /// Действующий запрос экрана: вид прибора, оси, размер картинки (как сериализует `ScopeRequest`).
    pub request: Option<Value>,
    /// Возможности устройства по округлению и денормалям f32 (пункт отчёта 14j: раньше в отчёт не попадало).
    pub float32_controls: Value,
    /// Сколько проходов приборов записано на момент входа.
    pub passes_recorded: u64,
    /// Время GPU прохода приборов на момент входа.
    pub gpu_time: Option<GpuTimeMs>,
}

impl ScopesReport {
    /// Снимок по диагностике видоискателя.
    pub fn of(viewfinder: &ViewfinderDiagnostics) -> Self {
        let controls = viewfinder.scopes.float32_controls;
        Self {
            request: viewfinder.scopes.requested.as_ref().map(|request| {
                serde_json::to_value(request)
                    .unwrap_or_else(|error| serde_json::json!({ "serialization_error": error.to_string() }))
            }),
            float32_controls: serde_json::json!({
                "rounding_mode_rte": controls.rounding_mode_rte,
                "denorm_preserve": controls.denorm_preserve,
                "denorm_flush_to_zero": controls.denorm_flush_to_zero,
            }),
            passes_recorded: viewfinder.scopes.counters.passes_recorded,
            gpu_time: GpuTimeMs::of_optional(viewfinder.scopes.gpu_time.as_ref()),
        }
    }
}

/// Место замера в отчёте.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SurfaceReport {
    pub surface: StageBudgetSurface,
    /// Прибор при входе на экран калибровки; на других местах `None`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scopes: Option<ScopesReport>,
    /// Не удалось выйти на место замера (экран не появился, раздел не открылся, приборы не заработали).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entry_failure: Option<String>,
    /// Не удалось вернуть экран в PHOTO по окончании.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_failure: Option<String>,
}

impl SurfaceReport {
    /// Место без наблюдений (тестовые наблюдения).
    #[cfg(test)]
    pub fn plain(surface: StageBudgetSurface) -> Self {
        Self {
            surface,
            scopes: None,
            entry_failure: None,
            exit_failure: None,
        }
    }

    /// Нарушения места замера: вход и выход обязаны удаться.
    pub fn violations(&self) -> Vec<String> {
        let mut violations = Vec::new();
        if let Some(reason) = &self.entry_failure {
            violations.push(format!("место замера {}: {reason}", self.surface.name()));
        }
        if let Some(reason) = &self.exit_failure {
            violations.push(format!(
                "экран не вернулся в Photo по окончании ({}): {reason}",
                self.surface.name()
            ));
        }
        violations
    }

    /// Сжатая запись прибора для строки результата: вид и размер запроса, возможности f32, число проходов.
    pub fn scopes_summary(&self) -> Option<Value> {
        let scopes = self.scopes.as_ref()?;
        Some(serde_json::json!({
            "request": scopes.request,
            "float32_controls": scopes.float32_controls,
            "passes_at_entry": scopes.passes_recorded,
        }))
    }
}

/// База «камера без нашей работы GPU и UI»: B-окна строки `whole_product` (мозаика, приборы и UI подавлены).
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct CameraWithoutProductGpuRow {
    pub frame_total_p50_ms: Option<f64>,
    pub frame_total_p95_ms: Option<f64>,
    pub mean_discharge_ma: Option<f64>,
}

impl CameraWithoutProductGpuRow {
    pub fn from_rows(rows: &[StageBudgetRow]) -> Option<Self> {
        let whole = rows.iter().find(|row| row.row == WHOLE_PRODUCT_ROW)?;
        Some(Self {
            frame_total_p50_ms: whole.gpu_off.frame_total_p50_ms,
            frame_total_p95_ms: whole.gpu_off.frame_total_p95_ms,
            mean_discharge_ma: whole.power.mean_off_ma(),
        })
    }

    pub fn table_line(&self) -> String {
        let gpu = match (self.frame_total_p50_ms, self.frame_total_p95_ms) {
            (Some(p50), Some(p95)) => format!("{p50:.2} / {p95:.2}"),
            _ => "нет замеров".to_owned(),
        };
        let current = self
            .mean_discharge_ma
            .map_or_else(|| "—".to_owned(), |ma| format!("{ma:.1} (абс.)"));
        format!("camera_without_product_gpu | {gpu} | {current} | baseline")
    }
}

/// Имя строки с частотой экрана: только у неё в заметки идёт частота показа.
const DISPLAY_ROW: &str = "display_120hz";

/// Тепло одним коротким словом: `статус запас` (`moderate 0.81`; `?` — не прочиталось).
fn thermal_text(snapshot: &ThermalSnapshot) -> String {
    let status = snapshot
        .status
        .and_then(|status| serde_json::to_value(status).ok())
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_else(|| "?".to_owned());
    match snapshot.headroom {
        Some(headroom) => format!("{status} {headroom:.2}"),
        None => status,
    }
}

/// Средняя частота показа окон положения `stage` строки, Гц.
fn mean_refresh_hz(conditions: &[WindowCondition], stage: StageSetting) -> Option<f64> {
    let values: Vec<f64> = conditions
        .iter()
        .filter(|condition| condition.stage == stage)
        .filter_map(|condition| condition.display.map(|display| display.mean_hz))
        .collect();
    (!values.is_empty()).then(|| round2(values.iter().sum::<f64>() / values.len() as f64))
}

/// Компактные заметки по строкам для строки результата: тепло с первого окна по последнее и, у строки с частотой,
/// частота показа в окнах A и B. Строки без условий окон (голый видоискатель до 16a) заметок не дают.
pub fn row_notes(rows: &[StageBudgetRow]) -> Vec<String> {
    rows.iter()
        .filter_map(|row| {
            let first = row.conditions.first()?;
            let last = row.conditions.last()?;
            let mut note = format!(
                "{}: {} -> {}",
                row.row,
                thermal_text(&first.thermal_start),
                thermal_text(&last.thermal_end)
            );
            if row.row == DISPLAY_ROW
                && let (Some(on), Some(off)) = (
                    mean_refresh_hz(&row.conditions, StageSetting::On),
                    mean_refresh_hz(&row.conditions, StageSetting::Off),
                )
            {
                note.push_str(&format!("; hz A {on:.0} B {off:.0}"));
            }
            Some(note)
        })
        .collect()
}

#[cfg(test)]
#[path = "stage_budget_surface_report_tests.rs"]
mod tests;

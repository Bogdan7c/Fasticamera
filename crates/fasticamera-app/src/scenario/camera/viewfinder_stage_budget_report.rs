//! Отчёт сценария `viewfinder-stage-budget`: таблица «стадия → мс GPU → мА» (решение 45).
//!
//! Строка таблицы — один A/B-замер [`AbPowerExperiment`](crate::power_measurement::AbPowerExperiment)
//! по схеме ABBA: A — «со стадией», B — «без неё». Время GPU каждого окна — снимок скользящих окон
//! потока рендера в конце окна (последние `viewfinder.gpu_time_window_frames` кадров, по умолчанию
//! 600 ≈ 10 с при 60 к/с — укладывается в измеряемую часть окна после разогрева). По окнам одной
//! настройки берётся медиана p50 и медиана p95; цена стадии в мс — разность «A − B».
//!
//! Строка «камера без ISP» — абсолютные числа B-окон строки «весь видоискатель» (отладочная мозаика
//! 05 вместо ISP): отдельного замера ей не нужно.
//!
//! Вердикт ([`evaluate_stage_budget`]) судит годность замера: ток обязан быть измерен (на внешнем
//! питании и без источника батареи — честный отказ), регулировка обязана действовать в каждом окне,
//! время GPU обязано быть. Сами числа — артефакт для `results/07.md`, не правило.

use serde::Serialize;
use serde_json::Value;

use super::detail::to_value;
use super::engine_watch::RunFailure;
use super::result_budget::bounded_list;
use super::viewfinder_metrics::{GpuTimeMs, median, round2};
use super::viewfinder_session::SessionSummary;
use crate::power_measurement::{AbPowerReport, Contribution, StageSetting};

/// Время GPU одного окна (снимок в конце измеряемой части).
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct GpuWindowSample {
    pub index: usize,
    pub stage: StageSetting,
    pub isp_dispatch: Option<GpuTimeMs>,
    pub frame_total: Option<GpuTimeMs>,
    /// Рисовальщик и стадии в статусе рендера совпали с нужными в момент снимка.
    pub setting_in_effect: bool,
}

/// Медианы времени GPU по окнам одной настройки, мс.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize)]
pub struct SettingGpu {
    pub windows: usize,
    pub isp_dispatch_p50_ms: Option<f64>,
    pub isp_dispatch_p95_ms: Option<f64>,
    pub frame_total_p50_ms: Option<f64>,
    pub frame_total_p95_ms: Option<f64>,
}

impl SettingGpu {
    /// Медианы по окнам настройки `stage`.
    pub fn of(samples: &[GpuWindowSample], stage: StageSetting) -> Self {
        let own: Vec<&GpuWindowSample> = samples.iter().filter(|sample| sample.stage == stage).collect();
        let pick = |value: fn(&GpuWindowSample) -> Option<f64>| {
            let values: Vec<f64> = own.iter().filter_map(|sample| value(sample)).collect();
            median(&values).map(round2)
        };
        Self {
            windows: own.len(),
            isp_dispatch_p50_ms: pick(|sample| sample.isp_dispatch.map(|time| time.p50_ms)),
            isp_dispatch_p95_ms: pick(|sample| sample.isp_dispatch.map(|time| time.p95_ms)),
            frame_total_p50_ms: pick(|sample| sample.frame_total.map(|time| time.p50_ms)),
            frame_total_p95_ms: pick(|sample| sample.frame_total.map(|time| time.p95_ms)),
        }
    }
}

/// Ток строки.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "source", rename_all = "snake_case")]
pub enum RowPower {
    /// A/B-замер прошёл до конца (вывод внутри: измерен / не различим / отказ).
    Ab { report: AbPowerReport },
    /// Опрос батареи не запустился.
    SamplingUnavailable { reason: String },
}

impl RowPower {
    fn contribution(&self) -> Option<&Contribution> {
        match self {
            RowPower::Ab { report } => Some(&report.contribution),
            RowPower::SamplingUnavailable { .. } => None,
        }
    }

    /// Средний ток окон B (стадия выключена), мА — для строки «камера без ISP».
    fn mean_off_ma(&self) -> Option<f64> {
        let RowPower::Ab { report } = self else {
            return None;
        };
        let off: Vec<f64> = report
            .windows
            .iter()
            .filter(|window| window.stage == StageSetting::Off)
            .map(|window| window.measurement.mean_discharge_ma)
            .collect();
        (!off.is_empty()).then(|| round2(off.iter().sum::<f64>() / off.len() as f64))
    }
}

/// Строка таблицы.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StageBudgetRow {
    /// Имя строки (`levels`, …, `whole_viewfinder`).
    pub row: &'static str,
    pub gpu_on: SettingGpu,
    pub gpu_off: SettingGpu,
    /// Цена в мс GPU «A − B»: для стадии — по проходу ISP, для всего видоискателя — по кадру целиком.
    pub delta_gpu_p50_ms: Option<f64>,
    pub delta_gpu_p95_ms: Option<f64>,
    pub power: RowPower,
    pub windows: Vec<GpuWindowSample>,
}

fn difference(on: Option<f64>, off: Option<f64>) -> Option<f64> {
    Some(round2(on? - off?))
}

impl StageBudgetRow {
    /// Собирает строку из окон GPU и тока.
    pub fn assemble(row: &'static str, windows: Vec<GpuWindowSample>, power: RowPower) -> Self {
        let gpu_on = SettingGpu::of(&windows, StageSetting::On);
        let gpu_off = SettingGpu::of(&windows, StageSetting::Off);
        // У отладочной мозаики прохода ISP нет: «весь видоискатель» сравнивается по кадру целиком.
        let whole = row == WHOLE_VIEWFINDER_ROW;
        let (delta_p50, delta_p95) = if whole {
            (
                difference(gpu_on.frame_total_p50_ms, gpu_off.frame_total_p50_ms),
                difference(gpu_on.frame_total_p95_ms, gpu_off.frame_total_p95_ms),
            )
        } else {
            (
                difference(gpu_on.isp_dispatch_p50_ms, gpu_off.isp_dispatch_p50_ms),
                difference(gpu_on.isp_dispatch_p95_ms, gpu_off.isp_dispatch_p95_ms),
            )
        };
        Self {
            row,
            gpu_on,
            gpu_off,
            delta_gpu_p50_ms: delta_p50,
            delta_gpu_p95_ms: delta_p95,
            power,
            windows,
        }
    }

    /// Строка таблицы для отчёта: `стадия | мс GPU (медиана / p95) | Δ мА | вывод`.
    pub fn table_line(&self) -> String {
        let gpu = match (self.delta_gpu_p50_ms, self.delta_gpu_p95_ms) {
            (Some(p50), Some(p95)) => format!("{p50:.2} / {p95:.2}"),
            _ => "нет замеров".to_owned(),
        };
        let (delta_ma, verdict) = match self.power.contribution() {
            Some(Contribution::Measured {
                mean_delta_ma,
                spread_ma,
            }) => (format!("{mean_delta_ma:.1} ± {spread_ma:.1}"), "measured".to_owned()),
            Some(Contribution::Indistinguishable { spread_ma }) => (
                format!("не различим (разброс {spread_ma:.1})"),
                "indistinguishable".to_owned(),
            ),
            Some(Contribution::Rejected { reason }) => {
                ("—".to_owned(), format!("rejected: {}", rejection_text(reason)))
            }
            None => ("—".to_owned(), "power_unavailable".to_owned()),
        };
        format!("{} | {gpu} | {delta_ma} | {verdict}", self.row)
    }
}

/// Причина отказа A/B коротко (для строки таблицы).
fn rejection_text(reason: &crate::power_measurement::AbRejection) -> String {
    serde_json::to_value(reason)
        .ok()
        .and_then(|value| {
            value
                .pointer("/rejection/kind")
                .or_else(|| value.get("kind"))
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .unwrap_or_else(|| "unknown".to_owned())
}

/// Имя строки «весь видоискатель».
pub const WHOLE_VIEWFINDER_ROW: &str = "whole_viewfinder";

/// Строка «камера без ISP» — B-окна строки «весь видоискатель».
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct CameraWithoutIspRow {
    pub frame_total_p50_ms: Option<f64>,
    pub frame_total_p95_ms: Option<f64>,
    pub mean_discharge_ma: Option<f64>,
}

impl CameraWithoutIspRow {
    pub fn from_rows(rows: &[StageBudgetRow]) -> Option<Self> {
        let whole = rows.iter().find(|row| row.row == WHOLE_VIEWFINDER_ROW)?;
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
        format!("camera_without_isp | {gpu} | {current} | baseline")
    }
}

/// Наблюдения прогона.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StageBudgetObservation {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure: Option<RunFailure>,
    /// Почему ток не мерили (внешнее питание, нет источника) — сценарий прерван честно.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub power_refusal: Option<String>,
    pub session: SessionSummary,
    pub requested_rows: Vec<&'static str>,
    pub rows: Vec<StageBudgetRow>,
    pub camera_without_isp: Option<CameraWithoutIspRow>,
    /// Окно времени GPU потока рендера, кадров (справка: оно должно укладываться в измеряемую часть).
    pub gpu_window_frames: usize,
    pub undelivered: Vec<String>,
}

/// Годен ли замер.
pub fn evaluate_stage_budget(observation: &StageBudgetObservation) -> Vec<String> {
    if let Some(failure) = &observation.failure {
        return vec![failure.describe()];
    }
    let mut violations: Vec<String> = observation
        .undelivered
        .iter()
        .map(|reason| format!("регулировка рендера не доставлена: {reason}"))
        .collect();
    if let Some(refusal) = &observation.power_refusal {
        violations.push(format!("ток не измерен: {refusal}"));
    }
    if observation.rows.len() < observation.requested_rows.len() && observation.power_refusal.is_none() {
        violations.push(format!(
            "измерено строк {} из {}",
            observation.rows.len(),
            observation.requested_rows.len()
        ));
    }
    for row in &observation.rows {
        let stray = row.windows.iter().filter(|window| !window.setting_in_effect).count();
        if stray > 0 {
            violations.push(format!(
                "строка {}: в {stray} окнах настройка видоискателя не действовала",
                row.row
            ));
        }
        if row.delta_gpu_p50_ms.is_none() {
            violations.push(format!("строка {}: нет времени GPU для одной из настроек", row.row));
        }
        if let RowPower::SamplingUnavailable { reason } = &row.power {
            violations.push(format!("строка {}: опрос батареи не запущен: {reason}", row.row));
        }
        if let Some(Contribution::Rejected { reason }) = row.power.contribution() {
            violations.push(format!(
                "строка {}: A/B тока отказал: {}",
                row.row,
                rejection_text(reason)
            ));
        }
    }
    violations
}

impl StageBudgetObservation {
    /// Сводка: таблица строками (её и читает владелец) и итоги.
    pub fn summary(&self) -> Value {
        let mut table: Vec<String> = self.rows.iter().map(StageBudgetRow::table_line).collect();
        if let Some(baseline) = &self.camera_without_isp {
            table.push(baseline.table_line());
        }
        serde_json::json!({
            "failure": self.failure,
            "power_refusal": self.power_refusal,
            "configured": self.session.configured,
            "requested_rows": self.requested_rows,
            "table_header": "stage | ms GPU (median / p95) | Δ mA | verdict",
            "table": table,
            "undelivered": bounded_list(&self.undelivered),
        })
    }

    /// Подробности: строка таблицы целиком (окна, пары тока) — по элементу на строку.
    pub fn detail_items(&self) -> Vec<Value> {
        // Строка дробится на три элемента: целиком она не влезает в строку `detail_*` (3400 байт).
        let mut items: Vec<Value> = Vec::new();
        for row in &self.rows {
            let (contribution, pairs, power_windows, samples) = match &row.power {
                RowPower::Ab { report } => (
                    to_value(&report.contribution),
                    to_value(&report.pairs),
                    to_value(&report.windows),
                    serde_json::json!({
                        "before_start": report.samples_before_start, "late": report.late_samples,
                    }),
                ),
                RowPower::SamplingUnavailable { reason } => (
                    serde_json::json!({ "sampling_unavailable": reason }),
                    Value::Null,
                    Value::Null,
                    Value::Null,
                ),
            };
            items.push(serde_json::json!({
                "row": row.row,
                "gpu_on": row.gpu_on,
                "gpu_off": row.gpu_off,
                "delta_gpu_p50_ms": row.delta_gpu_p50_ms,
                "delta_gpu_p95_ms": row.delta_gpu_p95_ms,
                "contribution": contribution,
                "pairs": pairs,
            }));
            items.push(serde_json::json!({ "row": row.row, "gpu_windows": to_value(&row.windows) }));
            items.push(serde_json::json!({ "row": row.row, "power_windows": power_windows, "samples": samples }));
        }
        items.push(serde_json::json!({
            "camera_without_isp": self.camera_without_isp,
            "gpu_window_frames": self.gpu_window_frames,
        }));
        items.push(serde_json::json!({ "session": to_value(&self.session) }));
        items
    }
}

#[cfg(test)]
#[path = "viewfinder_stage_budget_report_tests.rs"]
mod tests;

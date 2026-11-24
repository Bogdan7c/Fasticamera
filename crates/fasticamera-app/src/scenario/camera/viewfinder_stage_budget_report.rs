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
//!
//! С 16a строка — пара состояний работы, а не только «стадия ISP» (`stage_budget_states`): колонка «мс GPU»
//! считается метрикой строки ([`CostMetric`]), окна несут условия (тепло на границах, частота показа, приращения
//! счётчиков, отказ окна — `stage_budget_window`), а результат называет место замера (`surface`) и, на экране
//! калибровки, действующий запрос прибора. Окно, чьё состояние не подтвердилось, отвергается эксперимент-вердиктом
//! `state_not_applied` / `display_refresh_not_reached` — строка тогда не `measured`.

use serde::Serialize;
use serde_json::Value;

use super::detail::to_value;
use super::engine_watch::RunFailure;
use super::result_budget::bounded_list;
use super::stage_budget_states::{CostMetric, RowStates};
use super::stage_budget_surface_report::{CameraWithoutProductGpuRow, SurfaceReport};
use super::stage_budget_window::WindowCondition;
use super::viewfinder_metrics::{GpuTimeMs, median, round2};
use super::viewfinder_session::SessionSummary;
use crate::power_measurement::{AbPowerReport, Contribution, StageSetting};
use crate::viewfinder_scenario_config::StageBudgetRowSetting;

/// Время GPU одного окна (снимок в конце измеряемой части).
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct GpuWindowSample {
    pub index: usize,
    pub stage: StageSetting,
    pub isp_dispatch: Option<GpuTimeMs>,
    pub frame_total: Option<GpuTimeMs>,
    /// Рисовальщик и стадии в статусе рендера совпали с нужными в момент снимка.
    pub setting_in_effect: bool,
    /// Проход UI поверх видоискателя (16a). `None` — UI не рисовался (экрана нет или рисование подавлено).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ui_overlay: Option<GpuTimeMs>,
    /// Проход цветовых приборов (16a). `None` — приборы не считались (закрыты или подавлены).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scopes: Option<GpuTimeMs>,
}

/// Медианы времени GPU по окнам одной настройки, мс.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize)]
pub struct SettingGpu {
    pub windows: usize,
    pub isp_dispatch_p50_ms: Option<f64>,
    pub isp_dispatch_p95_ms: Option<f64>,
    pub frame_total_p50_ms: Option<f64>,
    pub frame_total_p95_ms: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ui_overlay_p50_ms: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ui_overlay_p95_ms: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scopes_p50_ms: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scopes_p95_ms: Option<f64>,
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
            ui_overlay_p50_ms: pick(|sample| sample.ui_overlay.map(|time| time.p50_ms)),
            ui_overlay_p95_ms: pick(|sample| sample.ui_overlay.map(|time| time.p95_ms)),
            scopes_p50_ms: pick(|sample| sample.scopes.map(|time| time.p50_ms)),
            scopes_p95_ms: pick(|sample| sample.scopes.map(|time| time.p95_ms)),
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
    pub(super) fn mean_off_ma(&self) -> Option<f64> {
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
    /// Условия окон (16a): тепло на границах, частота показа, приращения счётчиков, отказ окна.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub conditions: Vec<WindowCondition>,
}

fn difference(on: Option<f64>, off: Option<f64>) -> Option<f64> {
    Some(round2(on? - off?))
}

impl StageBudgetRow {
    /// Собирает строку из окон GPU и тока. Чем считается цена в мс — по имени строки (`CostMetric`).
    pub fn assemble(row: &'static str, windows: Vec<GpuWindowSample>, power: RowPower) -> Self {
        let gpu_on = SettingGpu::of(&windows, StageSetting::On);
        let gpu_off = SettingGpu::of(&windows, StageSetting::Off);
        let cost = cost_metric_of(row);
        // Подавленная работа (UI, приборы) в окнах B не выполнялась: её время там — ноль, а не «нет замеров».
        let off = |value: Option<f64>| {
            if cost.absent_when_off_means_zero() {
                absent_off_as_zero(&gpu_off, value)
            } else {
                value
            }
        };
        let (delta_p50, delta_p95) = match cost {
            // У отладочной мозаики прохода ISP нет: строки с заменой рисовальщика сравниваются по кадру целиком.
            CostMetric::FrameTotal => (
                difference(gpu_on.frame_total_p50_ms, off(gpu_off.frame_total_p50_ms)),
                difference(gpu_on.frame_total_p95_ms, off(gpu_off.frame_total_p95_ms)),
            ),
            CostMetric::IspDispatch => (
                difference(gpu_on.isp_dispatch_p50_ms, off(gpu_off.isp_dispatch_p50_ms)),
                difference(gpu_on.isp_dispatch_p95_ms, off(gpu_off.isp_dispatch_p95_ms)),
            ),
            CostMetric::UiOverlay => (
                difference(gpu_on.ui_overlay_p50_ms, off(gpu_off.ui_overlay_p50_ms)),
                difference(gpu_on.ui_overlay_p95_ms, off(gpu_off.ui_overlay_p95_ms)),
            ),
            CostMetric::ColorScopes => (
                difference(gpu_on.scopes_p50_ms, off(gpu_off.scopes_p50_ms)),
                difference(gpu_on.scopes_p95_ms, off(gpu_off.scopes_p95_ms)),
            ),
        };
        Self {
            row,
            gpu_on,
            gpu_off,
            delta_gpu_p50_ms: delta_p50,
            delta_gpu_p95_ms: delta_p95,
            power,
            windows,
            conditions: Vec::new(),
        }
    }

    /// Строка с условиями окон (тепло, частота, счётчики, отказы).
    #[must_use]
    pub fn with_conditions(mut self, conditions: Vec<WindowCondition>) -> Self {
        self.conditions = conditions;
        self
    }

    /// Строка таблицы для отчёта: `стадия | мс GPU (медиана / p95) | Δ мА | вывод`.
    pub fn table_line(&self) -> String {
        // У строки, отвергнутой по состоянию окна, цифр мс нет: часть окон шла не в том состоянии, и «0.00 / 0.00»
        // читалось бы как «работа стоит ноль».
        let refused_by_scenario = matches!(
            self.power.contribution(),
            Some(Contribution::Rejected { reason }) if reason.is_declared_by_scenario()
        );
        let gpu = match (self.delta_gpu_p50_ms, self.delta_gpu_p95_ms) {
            _ if refused_by_scenario => "—".to_owned(),
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

/// Отказ окна, объявленный сценарием: номер окна и что не подтвердилось (из условий окна строки).
fn declared_refusal_text(row: &StageBudgetRow, reason: &crate::power_measurement::AbRejection) -> String {
    let window = match reason {
        crate::power_measurement::AbRejection::StateNotApplied { index, .. }
        | crate::power_measurement::AbRejection::DisplayRefreshNotReached { index, .. } => *index,
        _ => return rejection_text(reason),
    };
    match row
        .conditions
        .iter()
        .find(|condition| condition.index == window)
        .and_then(|condition| condition.refusal.as_ref())
    {
        Some(refusal) => format!("окно {window} отвергнуто: {}", refusal.describe()),
        None => format!("окно {window} отвергнуто: {}", rejection_text(reason)),
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
/// Имя строки «вся наша работа GPU и UI» (16a).
pub const WHOLE_PRODUCT_ROW: &str = "whole_product";

/// Метрика цены строки по имени; имя не из списка (подставные строки тестов) считается проходом ISP, как раньше.
fn cost_metric_of(row: &str) -> CostMetric {
    StageBudgetRowSetting::from_name(row).map_or(CostMetric::IspDispatch, |setting| RowStates::of(setting).cost)
}

/// У работы, подавленной в окнах B, там времени нет по определению: она не выполнялась, значит стоила 0 мс. Окон B
/// при этом должно быть хотя бы одно — без них «ноль» был бы выдумкой.
fn absent_off_as_zero(off: &SettingGpu, measured: Option<f64>) -> Option<f64> {
    measured.or((off.windows > 0).then_some(0.0))
}

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
    /// База «камера без нашей работы GPU и UI» — B-окна строки `whole_product` (16a).
    pub camera_without_product_gpu: Option<CameraWithoutProductGpuRow>,
    /// Место замера и то, что на нём наблюдали (запрос прибора, сбои входа и выхода).
    pub surface: SurfaceReport,
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
    violations.extend(observation.surface.violations());
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
            violations.push(if reason.is_declared_by_scenario() {
                // Окно отвергнуто по состоянию (16a): к причине добавляем, ЧТО именно не подтвердилось.
                format!("строка {}: {}", row.row, declared_refusal_text(row, reason))
            } else {
                format!("строка {}: A/B тока отказал: {}", row.row, rejection_text(reason))
            });
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
        if let Some(baseline) = &self.camera_without_product_gpu {
            table.push(baseline.table_line());
        }
        let mut summary = serde_json::json!({
            "failure": self.failure,
            "power_refusal": self.power_refusal,
            "configured": self.session.configured,
            "surface": self.surface.surface.name(),
            "requested_rows": self.requested_rows,
            "table_header": "stage | ms GPU (median / p95) | Δ mA | verdict",
            "table": table,
            "undelivered": bounded_list(&self.undelivered),
        });
        if let (Some(map), Some(scopes)) = (summary.as_object_mut(), self.surface.scopes_summary()) {
            map.insert("scopes".to_owned(), scopes);
        }
        let notes = super::stage_budget_surface_report::row_notes(&self.rows);
        if let (Some(map), false) = (summary.as_object_mut(), notes.is_empty()) {
            map.insert("row_notes".to_owned(), serde_json::json!(notes));
        }
        summary
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
            if row.conditions.is_empty() {
                items.push(serde_json::json!({ "row": row.row, "gpu_windows": to_value(&row.windows) }));
            } else {
                // 16a: у окна ещё тепло, частота, счётчики и отказ — по элементу на окно, иначе строка не влезет.
                for window in &row.windows {
                    let condition = row.conditions.iter().find(|condition| condition.index == window.index);
                    items.push(serde_json::json!({
                        "row": row.row, "gpu_window": to_value(window), "condition": condition.map(to_value),
                    }));
                }
                let unmatched: Vec<&WindowCondition> = row
                    .conditions
                    .iter()
                    .filter(|condition| !row.windows.iter().any(|window| window.index == condition.index))
                    .collect();
                if !unmatched.is_empty() {
                    items.push(serde_json::json!({ "row": row.row, "conditions_without_gpu": to_value(&unmatched) }));
                }
            }
            items.push(serde_json::json!({ "row": row.row, "power_windows": power_windows, "samples": samples }));
        }
        let mut baselines = serde_json::json!({
            "camera_without_isp": self.camera_without_isp,
            "gpu_window_frames": self.gpu_window_frames,
        });
        if let (Some(map), Some(baseline)) = (baselines.as_object_mut(), &self.camera_without_product_gpu) {
            map.insert("camera_without_product_gpu".to_owned(), to_value(baseline));
        }
        items.push(baselines);
        items.push(to_value(&self.surface));
        items.push(serde_json::json!({ "session": to_value(&self.session) }));
        items
    }
}

#[cfg(test)]
#[path = "viewfinder_stage_budget_report_tests.rs"]
mod tests;

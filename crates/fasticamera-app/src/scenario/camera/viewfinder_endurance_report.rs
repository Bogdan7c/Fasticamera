//! Отчёт сценария `viewfinder-endurance` (сессия 07): 10 минут продуктового видоискателя.
//!
//! Критерий выхода 07 — «без троттлинга или с найденной и записанной причиной». Поэтому троттлинг
//! здесь не провал, а находка: [`find_throttling`] перечисляет признаки по порогам конфига
//! (`viewfinder.endurance`), и они попадают в отчёт. Провал ([`evaluate_endurance`]) — только когда
//! прогон негоден: отказ камеры/рендера, прогон не дошёл до конца, CPU-копии, сбои ISP.

use serde::Serialize;
use serde_json::Value;
use shell_contract::thermal::ThermalStatus;

use super::detail::to_value;
use super::engine_watch::RunFailure;
use super::viewfinder_metrics::{WindowDelta, ratio, round2};
use super::viewfinder_session::SessionSummary;
use crate::power_measurement::EnduranceReport;
use crate::viewfinder_scenario_config::ViewfinderEnduranceConfig;

/// Сколько признаков троттлинга показывать в строке результата (остальные — в подробностях).
pub const MAX_THROTTLING_SIGNS_IN_SUMMARY: usize = 6;

/// Признак троттлинга.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(tag = "sign", rename_all = "snake_case")]
pub enum ThrottlingSign {
    /// Тепловой статус достиг порога (минута 0 — начало прогона).
    ThermalStatus { minute: u32, status: ThermalStatus },
    /// Частота сенсора в минуте ниже порога.
    SensorFpsDrop { minute: u32, fps: f64 },
    /// Время прохода ISP выросло от первой минуты к последней сверх допуска.
    IspTimeGrowth { first_ms: f64, last_ms: f64, growth: f64 },
}

/// Признаки троттлинга по минутам отчёта.
pub fn find_throttling(config: &ViewfinderEnduranceConfig, report: &EnduranceReport) -> Vec<ThrottlingSign> {
    let threshold = config.throttling_thermal_status.to_status();
    let mut signs = Vec::new();
    if let Some(status) = report.start.status.filter(|status| *status >= threshold) {
        signs.push(ThrottlingSign::ThermalStatus { minute: 0, status });
    }
    for minute in &report.minutes {
        if let Some(status) = minute.observation.thermal.status.filter(|status| *status >= threshold) {
            signs.push(ThrottlingSign::ThermalStatus {
                minute: minute.minute,
                status,
            });
        }
        if let Some(fps) = minute.observation.sensor_fps.filter(|fps| *fps < config.min_sensor_fps) {
            signs.push(ThrottlingSign::SensorFpsDrop {
                minute: minute.minute,
                fps: round2(fps),
            });
        }
    }
    let isp_times: Vec<f64> = report
        .minutes
        .iter()
        .filter_map(|minute| minute.observation.isp_gpu_time.map(|time| time.median_ms))
        .collect();
    if let (Some(first), Some(last)) = (isp_times.first(), isp_times.last())
        && let Some(growth) = ratio(*last, *first)
        && growth > config.max_isp_time_growth
    {
        signs.push(ThrottlingSign::IspTimeGrowth {
            first_ms: round2(*first),
            last_ms: round2(*last),
            growth: round2(growth),
        });
    }
    signs
}

/// Наблюдения прогона.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ViewfinderEnduranceObservation {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure: Option<RunFailure>,
    pub session: SessionSummary,
    pub expected_minutes: u32,
    /// Почему ток не мерился (нет источника батареи); `None` — опрос шёл (минуты на внешнем
    /// питании всё равно отвергаются внутри отчёта).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub power_unavailable: Option<String>,
    pub report: EnduranceReport,
    pub window: WindowDelta,
    pub throttling: Vec<ThrottlingSign>,
}

/// Годен ли прогон.
pub fn evaluate_endurance(observation: &ViewfinderEnduranceObservation) -> Vec<String> {
    if let Some(failure) = &observation.failure {
        return vec![failure.describe()];
    }
    let mut violations = Vec::new();
    let minutes = u32::try_from(observation.report.minutes.len()).unwrap_or(u32::MAX);
    if minutes < observation.expected_minutes {
        violations.push(format!("записано минут {minutes} из {}", observation.expected_minutes));
    }
    let isp = &observation.window.isp;
    let copies = isp.cpu_copies + observation.window.stream_cpu_copies;
    if copies != 0 {
        violations.push(format!("CPU-копий кадров: {copies}, ожидалось 0"));
    }
    for (name, value) in [
        ("isp_record_failures", isp.isp_record_failures),
        ("compose_failures", isp.compose_failures),
        ("params_failed", isp.params_failed),
    ] {
        if value != 0 {
            violations.push(format!("{name} = {value}, ожидалось 0"));
        }
    }
    if isp.frames_processed == 0 {
        violations.push("ISP не обработал ни одного кадра".to_owned());
    }
    violations
}

impl ViewfinderEnduranceObservation {
    /// Сводка: вывод о троттлинге, итоги тока и тепла, последняя минута (все минуты — в деталях).
    pub fn summary(&self, budget_bytes: usize) -> Value {
        serde_json::json!({
            "failure": self.failure,
            "configured": self.session.configured,
            "state_at_end": self.session.state_at_end,
            "expected_minutes": self.expected_minutes,
            "power_unavailable": self.power_unavailable,
            // Признаков до двух на минуту: в сводку — первые (их достаточно для вывода «найден» и
            // минуты начала), все — в `detail_endurance`.
            "throttling": {
                "found": !self.throttling.is_empty(),
                "sign_count": self.throttling.len(),
                "signs": self.throttling.iter().take(MAX_THROTTLING_SIGNS_IN_SUMMARY).collect::<Vec<_>>(),
            },
            "frames_processed": self.window.isp.frames_processed,
            "cpu_copies": self.window.isp.cpu_copies + self.window.stream_cpu_copies,
            "endurance": self.report.summary_within(budget_bytes),
        })
    }

    pub fn detail_items(&self) -> Vec<Value> {
        let mut items = self.report.detail_items();
        items.extend(
            self.throttling
                .iter()
                .map(|sign| serde_json::json!({ "throttling": sign })),
        );
        items.push(serde_json::json!({ "window": to_value(&self.window) }));
        items.push(serde_json::json!({ "session": to_value(&self.session) }));
        items
    }
}

#[cfg(test)]
#[path = "viewfinder_endurance_report_tests.rs"]
mod tests;

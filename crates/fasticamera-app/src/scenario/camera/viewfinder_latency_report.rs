//! Отчёт и вывод сценария `viewfinder-latency`: два профиля настроек рендера в чередующихся окнах.
//!
//! Схема окон — ABBA, как у A/B тока (почему — `power_measurement::ab_experiment`): A — база
//! ([`ProfileSlot::Baseline`], первый профиль конфига), B — проверяемый ([`ProfileSlot::Tuned`],
//! второй). Дрейф (нагрев, частоты GPU) в ABBA сокращается в среднем и честно попадает в разброс пар.
//!
//! Сравнение профилей и вердикт — `viewfinder_latency_verdict`. Сценарий ПАДАЕТ только когда замер
//! негоден ([`evaluate_latency`]): отказ камеры/рендера, регулировка не дошла или не применилась,
//! мало замеров в окне, ADPF включён и не создался. Сам вывод «лучше/хуже» — данные для решения владельца.

use serde::Serialize;
use serde_json::Value;

use super::detail::to_value;
use super::engine_watch::RunFailure;
use super::raw_stream_samples::LatencyReport;
use super::result_budget::bounded_list;
use super::viewfinder_latency_verdict::ProfileComparison;
use super::viewfinder_metrics::{GpuTimeMs, PresentTargetDelta};
use super::viewfinder_pacing_report::PacingWindowReport;
use super::viewfinder_session::SessionSummary;
use super::viewfinder_tuning_report::AdpfWindowReport;
use crate::viewfinder_latency_profile::LatencyProfileConfig;
use crate::viewfinder_scenario_config::ViewfinderLatencyConfig;

/// Роль профиля в сравнении (по месту в списке `latency.profiles`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileSlot {
    /// A: база, первый профиль.
    Baseline,
    /// B: проверяемый, второй профиль.
    Tuned,
}

impl ProfileSlot {
    /// Профиль окна `index` по схеме ABBA: пара `index / 2`, чётная пара — A, B; нечётная — B, A.
    pub fn of_window(index: usize) -> Self {
        let pair = index / 2;
        let first_in_pair = index.is_multiple_of(2);
        let a_first = pair.is_multiple_of(2);
        if first_in_pair == a_first {
            Self::Baseline
        } else {
            Self::Tuned
        }
    }

    /// Место профиля в списке конфига.
    pub fn index(self) -> usize {
        match self {
            Self::Baseline => 0,
            Self::Tuned => 1,
        }
    }
}

/// Закрытое окно.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LatencyWindow {
    pub index: usize,
    pub pair: usize,
    pub profile: ProfileSlot,
    /// Длина измеряемой части окна, мс.
    pub measured_ms: u64,
    /// Кадров ISP в секунду.
    pub processed_fps: Option<f64>,
    /// Кадров сенсора в секунду (счётчик потока RAW).
    pub sensor_fps: Option<f64>,
    /// Кадров, вытесненных более новыми, за окно и в секунду.
    pub superseded: u64,
    pub superseded_per_s: Option<f64>,
    /// Подача кадров видоискателя: потери, повторы, доля кадров, показанных ровно один раз.
    pub pacing: PacingWindowReport,
    pub latency: LatencyReport,
    pub present_target: PresentTargetDelta,
    /// ADPF окна: режим и сессия на конец, приращения отчётов.
    pub adpf: AdpfWindowReport,
    /// Время кадра на GPU на конец окна (окно статистики сброшено командой начала окна).
    pub frame_gpu_time: Option<GpuTimeMs>,
    /// Поток рендера к концу окна работал именно с настройками профиля (линия кадра, поздний
    /// захват, сход на ближнюю линию и режим ADPF), а поток кадров — с его политикой подачи.
    pub selection_matched: bool,
}

impl LatencyWindow {
    /// Доля кадров, показанных позже задуманного vsync; `None` — кадров с известным опозданием нет.
    pub fn miss_ratio(&self) -> Option<f64> {
        self.present_target.vsync_shift.miss_ratio()
    }
}

/// Наблюдения прогона.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LatencyObservation {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure: Option<RunFailure>,
    pub session: SessionSummary,
    /// Профили сравнения (база, проверяемый) — как их поставил сценарий.
    pub profiles: Vec<LatencyProfileConfig>,
    pub expected_windows: usize,
    pub windows: Vec<LatencyWindow>,
    /// Регулировки, которые не дошли до потока рендера (имя и причина).
    pub undelivered: Vec<String>,
    pub comparison: ProfileComparison,
}

impl LatencyObservation {
    /// Имя профиля роли `slot` для сообщений.
    fn profile_name(&self, slot: ProfileSlot) -> &str {
        self.profiles
            .get(slot.index())
            .map_or("?", |profile| profile.name.as_str())
    }
}

/// Годен ли замер: список нарушений (пустой — прошёл; вывод о профилях — в отчёте).
pub fn evaluate_latency(config: &ViewfinderLatencyConfig, observation: &LatencyObservation) -> Vec<String> {
    if let Some(failure) = &observation.failure {
        return vec![failure.describe()];
    }
    let mut violations: Vec<String> = observation
        .undelivered
        .iter()
        .map(|reason| format!("регулировка рендера не доставлена: {reason}"))
        .collect();
    if observation.windows.len() < observation.expected_windows {
        violations.push(format!(
            "закрыто окон {} из {}",
            observation.windows.len(),
            observation.expected_windows
        ));
    }
    for window in &observation.windows {
        let name = observation.profile_name(window.profile);
        if window.latency.samples < config.min_samples_per_window {
            violations.push(format!(
                "окно {} ({name}): замеров задержки {} меньше {}",
                window.index, window.latency.samples, config.min_samples_per_window
            ));
        }
        if !window.selection_matched {
            violations.push(format!(
                "окно {} ({name}): поток рендера или поток кадров работал не с настройками профиля (линия кадра, захват, сход на ближнюю линию, ADPF или подача кадров)",
                window.index
            ));
        }
        if window.adpf.failed_while_enabled() {
            violations.push(format!(
                "окно {} ({name}): ADPF включён, но сессия не создана (failed)",
                window.index
            ));
        }
    }
    violations
}

impl LatencyObservation {
    /// Сводка для строки результата: сравнение целиком, окна — в `detail_latency`.
    pub fn summary(&self) -> Value {
        serde_json::json!({
            "failure": self.failure,
            "configured": self.session.configured,
            "state_at_end": self.session.state_at_end,
            "profiles": self.profiles,
            "windows": self.windows.len(),
            "expected_windows": self.expected_windows,
            // Регулировок по одной на окно: число окон растёт с `latency.pairs`.
            "undelivered": bounded_list(&self.undelivered),
            "comparison": self.comparison,
        })
    }

    pub fn detail_items(&self) -> Vec<Value> {
        let mut items: Vec<Value> = self.windows.iter().map(to_value).collect();
        items.push(serde_json::json!({ "session": to_value(&self.session) }));
        items
    }
}

#[cfg(test)]
#[path = "viewfinder_latency_report_tests.rs"]
mod tests;

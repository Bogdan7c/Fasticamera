//! Отчётные части «настройки рендера под видоискатель» (P13 сессии 07): выбор линии кадра с оценкой
//! стоимости кадра и состояние ADPF. Общие для сценариев `viewfinder` (состояние на конец замера) и
//! `viewfinder-latency` (приращения за окно профиля).
//!
//! Чистые функции над снимками потока рендера, без платформы: проверяются на ПК.

use std::time::Duration;

use render_loop::config::AdpfMode;
use render_loop::thread::{AdpfDiagnostics, AdpfSessionState, GpuCostSource, PresentTargetDiagnostics};
use serde::Serialize;

use super::viewfinder_metrics::{GpuTimeMs, millis, round2};
use crate::viewfinder_control::PresentTargetSetting;

/// Самая длинная причина последней ошибки ADPF в отчёте, символов (строка результата ограничена).
const MAX_ADPF_ERROR_CHARS: usize = 120;

/// Имя состояния сессии ADPF для отчёта и скриптов.
pub fn adpf_session_name(state: &AdpfSessionState) -> &'static str {
    match state {
        AdpfSessionState::Disabled => "disabled",
        AdpfSessionState::Pending => "pending",
        AdpfSessionState::Active => "active",
        AdpfSessionState::Unsupported => "unsupported",
        AdpfSessionState::Failed => "failed",
    }
}

/// Состояние ADPF на момент снимка (счётчики накопительные за жизнь потока рендера).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AdpfReport {
    pub mode: AdpfMode,
    /// `disabled` / `pending` / `active` / `unsupported` / `failed`.
    pub session: &'static str,
    pub reports_sent: u64,
    pub report_failures: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

impl AdpfReport {
    pub fn of(adpf: &AdpfDiagnostics) -> Self {
        Self {
            mode: adpf.mode,
            session: adpf_session_name(&adpf.session),
            reports_sent: adpf.reports_sent,
            report_failures: adpf.report_failures,
            last_error: adpf
                .last_error
                .as_ref()
                .map(|error| error.chars().take(MAX_ADPF_ERROR_CHARS).collect()),
        }
    }

    /// Нарушение smoke: ADPF включён, а сессию создать не удалось (`Unsupported` — не нарушение:
    /// платформа может подсказок не поддерживать, это просто отчитывается).
    pub fn violation(&self) -> Option<String> {
        (self.mode == AdpfMode::Enabled && self.session == "failed").then(|| {
            format!(
                "ADPF включён, но сессия не создана (failed): {}",
                self.last_error.as_deref().unwrap_or("причина не записана")
            )
        })
    }
}

/// Приращения учёта ADPF за окно.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct AdpfWindowDelta {
    pub reports_sent: u64,
    pub reports_with_gpu: u64,
    pub report_failures: u64,
}

impl AdpfWindowDelta {
    pub fn between(before: &AdpfDiagnostics, after: &AdpfDiagnostics) -> Self {
        Self {
            reports_sent: after.reports_sent.saturating_sub(before.reports_sent),
            reports_with_gpu: after.reports_with_gpu.saturating_sub(before.reports_with_gpu),
            report_failures: after.report_failures.saturating_sub(before.report_failures),
        }
    }
}

/// ADPF окна: режим и состояние сессии на конец окна плюс приращения за окно.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AdpfWindowReport {
    pub mode: AdpfMode,
    pub session: &'static str,
    #[serde(flatten)]
    pub delta: AdpfWindowDelta,
}

impl AdpfWindowReport {
    /// Нарушение: ADPF включён, сессия не создалась.
    pub fn failed_while_enabled(&self) -> bool {
        self.mode == AdpfMode::Enabled && self.session == "failed"
    }
}

/// Имя источника GPU-части оценки стоимости кадра.
fn gpu_source_name(source: GpuCostSource) -> &'static str {
    match source {
        GpuCostSource::Measured => "measured",
        GpuCostSource::ConfiguredFallback => "configured_fallback",
    }
}

fn millis_hundredths(duration: Duration) -> f64 {
    round2(millis(duration))
}

/// Действующий выбор линии кадра, оценка стоимости кадра и время кадра на GPU.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PresentTargetReport {
    #[serde(flatten)]
    pub selection: PresentTargetSetting,
    /// Откуда взята GPU-часть оценки: `measured` или `configured_fallback`.
    pub gpu_source: &'static str,
    pub estimated_cpu_ms: f64,
    pub estimated_gpu_ms: f64,
    /// Время кадра на GPU по окну потока рендера (`None` — бэкенд не меряет или замеров нет).
    pub frame_gpu_time: Option<GpuTimeMs>,
}

impl PresentTargetReport {
    pub fn of(present_target: &PresentTargetDiagnostics) -> Self {
        Self {
            selection: PresentTargetSetting {
                policy: present_target.selection.policy,
                late_latch: present_target.selection.late_latch,
                depth_reduction: present_target.selection.depth_reduction,
            },
            gpu_source: gpu_source_name(present_target.estimated_cost_parts.gpu_source),
            estimated_cpu_ms: millis_hundredths(present_target.estimated_cost_parts.cpu),
            estimated_gpu_ms: millis_hundredths(present_target.estimated_cost_parts.gpu),
            frame_gpu_time: GpuTimeMs::of_optional(present_target.frame_gpu_time.as_ref()),
        }
    }
}

#[cfg(test)]
#[path = "viewfinder_tuning_report_tests.rs"]
mod tests;

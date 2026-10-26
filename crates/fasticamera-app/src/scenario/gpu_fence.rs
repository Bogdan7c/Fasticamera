//! Сценарий `gpu-fence`: GPU-работа B, ждущая `sync_fd` работы A, не начинается раньше конца A.
//!
//! Проверка идёт на рабочем потоке с отдельным `GpuContext` (`fence_order_check`): A — долгий compute,
//! после него экспортируется `sync_fd`, B ждёт импортированный fence и читает итог A; метки времени GPU
//! показывают порядок. Вторая фаза проверяет путь «fence уже сигнален».
//!
//! Правила прохода: B увидела итог A, B стартовала после конца A, экспортированный fence в момент
//! экспорта был ещё не сигнален (это настоящий незавершённый `sync_fd`), фаза «уже сигнален» отработала
//! без ожидающего семафора и дала верный итог, ошибок validation нет.

use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;

use super::background_job::{BackgroundJob, JobPoll};
use super::observing::{ObservedEvent, ObservingScenario, ScenarioContext, ScenarioStep, failed, stage_fields};
use super::render_metrics::Verdict;
use crate::gpu_services::{FenceCheckRequest, FenceCheckResult, FenceCheckSummary, GpuDiagnostics, ValidationState};

#[derive(Debug, Serialize)]
struct FenceReport<'summary> {
    busy_iterations: u32,
    #[serde(flatten)]
    summary: Option<&'summary FenceCheckSummary>,
    error: Option<&'summary str>,
}

/// Судит результат проверки.
pub fn evaluate(busy_iterations: u32, result: &FenceCheckResult) -> Verdict {
    let mut violations = Vec::new();
    let report = match result {
        FenceCheckResult::Completed(summary) => {
            if summary.work_a_value != summary.expected_value {
                violations.push(format!(
                    "работа A дала {:#010X}, ожидалось {:#010X}",
                    summary.work_a_value, summary.expected_value
                ));
            }
            if summary.work_b_value != summary.expected_value {
                violations.push(format!(
                    "работа B увидела {:#010X}, а не итог A {:#010X}: порядок не соблюдён",
                    summary.work_b_value, summary.expected_value
                ));
            }
            if !summary.b_started_after_a_finished {
                violations.push(format!(
                    "B стартовала до конца A (зазор {} нс)",
                    summary.gap_between_a_end_and_b_begin_ns
                ));
            }
            match summary.release_fence_pending_at_export {
                Some(true) => {}
                Some(false) => {
                    violations.push("экспортированный fence был уже сигнален: ожидание не проверено".to_owned())
                }
                None => violations.push("экспорт fence в sync_fd не поддерживается: порядок не проверить".to_owned()),
            }
            if summary.already_signaled_wait_semaphore_created {
                violations.push("фаза «уже сигнален»: создан лишний ожидающий семафор".to_owned());
            }
            if !summary.already_signaled_work_completed {
                violations.push("фаза «уже сигнален»: работа не завершилась".to_owned());
            }
            if summary.already_signaled_result_value != summary.expected_value {
                violations.push(format!(
                    "фаза «уже сигнален»: итог {:#010X}, ожидалось {:#010X}",
                    summary.already_signaled_result_value, summary.expected_value
                ));
            }
            if summary.validation == ValidationState::Enabled && summary.validation_errors != 0 {
                violations.push(format!("ошибки validation: {}", summary.validation_errors));
            }
            FenceReport {
                busy_iterations,
                summary: Some(summary),
                error: None,
            }
        }
        FenceCheckResult::Failed { reason } => {
            violations.push(format!("диагностика не выполнена: {reason}"));
            FenceReport {
                busy_iterations,
                summary: None,
                error: Some(reason),
            }
        }
    };
    Verdict::new(report, violations)
}

#[derive(Default)]
pub struct GpuFenceScenario {
    job: Option<BackgroundJob<FenceCheckResult>>,
    busy_iterations: u32,
}

impl GpuFenceScenario {
    pub fn new() -> Self {
        Self::default()
    }
}

impl ObservingScenario for GpuFenceScenario {
    fn name(&self) -> &'static str {
        "gpu-fence"
    }

    fn start(&mut self, context: &mut ScenarioContext<'_>) -> ScenarioStep {
        self.busy_iterations = context.config.fence_busy_iterations;
        let request = FenceCheckRequest {
            busy_iterations: self.busy_iterations,
        };
        let diagnostics: Arc<dyn GpuDiagnostics> = Arc::clone(&context.gpu.diagnostics);
        let timeout = Duration::from_millis(context.config.gpu_job_timeout_ms);
        match BackgroundJob::spawn("fasticamera-fence", timeout, move || diagnostics.fence_order(request)) {
            Ok(job) => {
                self.job = Some(job);
                context.emit_stage(
                    "started",
                    stage_fields([("busy_iterations", self.busy_iterations.into())]),
                );
                ScenarioStep::Continue
            }
            Err(reason) => ScenarioStep::Finished(failed(reason)),
        }
    }

    fn on_event(&mut self, _event: ObservedEvent, _context: &mut ScenarioContext<'_>) -> ScenarioStep {
        ScenarioStep::Continue
    }

    fn on_tick(&mut self, _context: &mut ScenarioContext<'_>) -> ScenarioStep {
        let Some(job) = &self.job else {
            return ScenarioStep::Continue;
        };
        match job.poll() {
            JobPoll::Pending => ScenarioStep::Continue,
            JobPoll::Done(result) => ScenarioStep::Finished(evaluate(self.busy_iterations, &result).into_outcome()),
            JobPoll::TimedOut { after } => ScenarioStep::Finished(failed(format!(
                "работа GPU не завершилась за {} мс (busy_iterations = {})",
                after.as_millis(),
                self.busy_iterations
            ))),
            JobPoll::Lost(reason) => ScenarioStep::Finished(failed(reason)),
        }
    }
}

#[cfg(test)]
#[path = "gpu_fence_tests.rs"]
mod tests;

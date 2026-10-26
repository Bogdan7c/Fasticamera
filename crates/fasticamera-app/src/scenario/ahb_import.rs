//! Сценарий `ahb-import`: RAW `AHardwareBuffer` попадает в Vulkan без копий.
//!
//! Проверка идёт на рабочем потоке (создание `GpuContext` и аллокация буфера долгие, а маршрутизатор
//! обязан читать события оболочки), маршрутизатор опрашивает результат на тиках. Для RAW16 и RAW10
//! (размеры — `scenarios.ahb_width/ahb_height`, 4096×3072 по умолчанию) запускается `ahb_roundtrip`:
//! буфер заполняется известным узором, импортируется, GPU считает контрольные суммы, их сверяют
//! с CPU, повторный импорт того же буфера обязан быть попаданием в кэш.
//!
//! Правила:
//! * RAW16: сумма совпала, второй импорт — `Hit`, объекты Vulkan восстановлены, ошибок validation 0
//!   (известные отступления из конфига, решение 35, допустимы и показываются в отчёте отдельно);
//! * RAW10: то же; если телефон не выделяет такой буфер — это `unsupported` (виден в отчёте, но
//!   не провал); несовпадение сумм — провал.

use serde::Serialize;

use super::background_job::{BackgroundJob, JobPoll};
use super::observing::{ObservedEvent, ObservingScenario, ScenarioContext, ScenarioStep, failed, stage_fields};
use super::render_metrics::Verdict;
use crate::gpu_services::{
    AhbCheckRequest, AhbCheckResult, AhbCheckSummary, AhbFormat, GpuDiagnostics, ValidationState,
};

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

/// Как закончилась проверка одного формата.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FormatOutcome {
    /// Суммы совпали, кэш и учёт объектов в порядке.
    Matched,
    /// Суммы, кэш и объекты в порядке, но validation слой нашёл ошибки сверх известных отступлений:
    /// данные дошли верно, значит, ломается использование Vulkan, а не импорт.
    MatchedWithValidationErrors,
    /// Прогон завершился, но не сошлись суммы, кэш или учёт объектов.
    Mismatch,
    /// Телефон не выделяет буфер такого формата.
    Unsupported,
    /// Диагностика сломалась (нет Vulkan, ошибка импорта…).
    Error,
}

/// Отчёт по одному формату.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FormatReport {
    pub format: AhbFormat,
    pub outcome: FormatOutcome,
    /// Подробности: однострочный отчёт `gpu-vulkan`, причина отказа аллокации или текст ошибки.
    pub details: String,
    pub summary: Option<AhbCheckSummary>,
}

/// Судит результат одного формата. `unsupported_is_failure` — RAW16 обязан работать всегда.
pub fn judge_format(
    format: AhbFormat,
    result: &AhbCheckResult,
    unsupported_is_failure: bool,
    violations: &mut Vec<String>,
) -> FormatReport {
    match result {
        AhbCheckResult::Completed(summary) => {
            let mut mismatches = Vec::new();
            if !summary.matched {
                mismatches.push("контрольные суммы GPU и CPU не совпали".to_owned());
            }
            if !summary.second_import_hit {
                mismatches.push("повторный импорт не попал в кэш".to_owned());
            }
            if !summary.live_objects_restored {
                mismatches.push("объекты Vulkan не вернулись к исходному числу".to_owned());
            }
            let has_validation_errors =
                summary.validation == ValidationState::Enabled && summary.validation_errors != 0;
            // Расхождение данных важнее ошибок validation: сначала проверяем, дошли ли данные верно.
            let outcome = if !mismatches.is_empty() {
                FormatOutcome::Mismatch
            } else if has_validation_errors {
                FormatOutcome::MatchedWithValidationErrors
            } else {
                FormatOutcome::Matched
            };
            let mut problems = mismatches;
            if has_validation_errors {
                problems.push(format!("ошибки validation: {}", summary.validation_errors));
            }
            violations.extend(problems.into_iter().map(|problem| format!("{format:?}: {problem}")));
            FormatReport {
                format,
                outcome,
                details: summary.details.clone(),
                summary: Some(summary.clone()),
            }
        }
        AhbCheckResult::AllocationUnsupported { status, requested } => {
            if unsupported_is_failure {
                violations.push(format!("{format:?}: телефон не выделил буфер (код {status})"));
            }
            FormatReport {
                format,
                outcome: FormatOutcome::Unsupported,
                details: format!("AHardwareBuffer_allocate вернул {status} для {requested}"),
                summary: None,
            }
        }
        AhbCheckResult::Failed { reason } => {
            violations.push(format!("{format:?}: диагностика не выполнена: {reason}"));
            FormatReport {
                format,
                outcome: FormatOutcome::Error,
                details: reason.clone(),
                summary: None,
            }
        }
    }
}

#[derive(Debug, Serialize)]
struct AhbReport {
    width: u32,
    height: u32,
    raw16: FormatReport,
    raw10: FormatReport,
}

/// Судит пару результатов.
pub fn evaluate(width: u32, height: u32, raw16: &AhbCheckResult, raw10: &AhbCheckResult) -> Verdict {
    let mut violations = Vec::new();
    let report = AhbReport {
        width,
        height,
        raw16: judge_format(AhbFormat::Raw16, raw16, true, &mut violations),
        raw10: judge_format(AhbFormat::Raw10, raw10, false, &mut violations),
    };
    Verdict::new(report, violations)
}

/// Результат фоновой работы: обе проверки.
type PairResult = (AhbCheckResult, AhbCheckResult);

#[derive(Default)]
pub struct AhbImportScenario {
    job: Option<BackgroundJob<PairResult>>,
    size: (u32, u32),
    /// Сколько проверок формата уже завершил рабочий поток (0..=2): при зависании по этому числу
    /// видно, на каком формате застряло, — частичные сведения для сообщения о таймауте.
    finished_formats: Arc<AtomicUsize>,
}

impl AhbImportScenario {
    pub fn new() -> Self {
        Self::default()
    }

    /// Фоновая работа: RAW16, затем RAW10 — одной пачкой на одном GPU-контексте
    /// (почему одной — см. [`GpuDiagnostics::ahb_roundtrips`]).
    fn run_checks(
        diagnostics: &Arc<dyn GpuDiagnostics>,
        width: u32,
        height: u32,
        finished_formats: &AtomicUsize,
    ) -> PairResult {
        let requests = [AhbFormat::Raw16, AhbFormat::Raw10].map(|format| AhbCheckRequest { format, width, height });
        let mut on_checked = |request: &AhbCheckRequest| {
            finished_formats.fetch_add(1, Ordering::SeqCst);
            tracing::info!(target: "fasticamera::scenario", format = ?request.format, "ahb-import: проверка формата завершена");
        };
        let mut results = diagnostics.ahb_roundtrips(&requests, &mut on_checked).into_iter();
        // Реализация обязана вернуть ответ на каждый запрос; недостающий — честная ошибка, не догадка.
        let mut next = || {
            results.next().unwrap_or(AhbCheckResult::Failed {
                reason: "диагностика не вернула результат проверки".to_owned(),
            })
        };
        let raw16 = next();
        let raw10 = next();
        (raw16, raw10)
    }

    /// Сообщение о зависшей работе: срок и то, что успело завершиться.
    fn timeout_message(&self, after: Duration) -> String {
        let finished = self.finished_formats.load(Ordering::SeqCst);
        let progress = match finished {
            0 => "ни один формат не завершён (завис RAW16)",
            1 => "RAW16 завершён, завис RAW10",
            _ => "оба формата завершены, но результат не передан",
        };
        format!("работа GPU не завершилась за {} мс: {progress}", after.as_millis())
    }
}

impl ObservingScenario for AhbImportScenario {
    fn name(&self) -> &'static str {
        "ahb-import"
    }

    fn start(&mut self, context: &mut ScenarioContext<'_>) -> ScenarioStep {
        let (width, height) = (context.config.ahb_width, context.config.ahb_height);
        self.size = (width, height);
        let diagnostics = Arc::clone(&context.gpu.diagnostics);
        let finished_formats = Arc::clone(&self.finished_formats);
        let timeout = Duration::from_millis(context.config.gpu_job_timeout_ms);
        match BackgroundJob::spawn("fasticamera-ahb", timeout, move || {
            Self::run_checks(&diagnostics, width, height, &finished_formats)
        }) {
            Ok(job) => {
                self.job = Some(job);
                context.emit_stage(
                    "started",
                    stage_fields([("width", width.into()), ("height", height.into())]),
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
            JobPoll::Done((raw16, raw10)) => {
                ScenarioStep::Finished(evaluate(self.size.0, self.size.1, &raw16, &raw10).into_outcome())
            }
            JobPoll::TimedOut { after } => ScenarioStep::Finished(failed(self.timeout_message(after))),
            JobPoll::Lost(reason) => ScenarioStep::Finished(failed(reason)),
        }
    }
}

#[cfg(test)]
#[path = "ahb_import_tests.rs"]
mod tests;

//! Сценарий `permission`: системный диалог камеры, два отказа подряд и запрос после них.
//!
//! Скрипт перед запуском сбрасывает разрешение нашего пакета, а на каждой стадии
//! `dialog_expected` жмёт «Don't allow». Ожидаемые ответы Android 11+:
//! 1-й отказ — `denied`, 2-й — `denied_permanently`, 3-й запрос диалога не показывает —
//! снова `denied_permanently`.

use serde::Serialize;
use shell_contract::commands::{CommandOutcome, PlatformCommand};
use shell_contract::event::ShellEventKind;
use shell_contract::permission::CameraPermission;

use super::observing::{ObservedEvent, ObservingScenario, ScenarioContext, ScenarioStep, failed, passed, stage_fields};

/// Сколько раз спрашиваем: два диалога и один запрос после постоянного отказа.
const REQUEST_ATTEMPTS: usize = 3;
/// С какой попытки система уже не показывает диалог (после двух отказов).
const FIRST_ATTEMPT_WITHOUT_DIALOG: usize = 3;

#[derive(Debug, Serialize)]
struct PermissionReport {
    answers: Vec<CameraPermission>,
}

#[derive(Debug, Default)]
pub struct PermissionScenario {
    answers: Vec<CameraPermission>,
}

impl PermissionScenario {
    pub fn new() -> Self {
        Self::default()
    }

    /// Пишет стадию очередной попытки и просит разрешение.
    fn request_next(&mut self, context: &mut ScenarioContext<'_>) -> ScenarioStep {
        let attempt = self.answers.len() + 1;
        let stage = if attempt >= FIRST_ATTEMPT_WITHOUT_DIALOG {
            "no_dialog_expected"
        } else {
            "dialog_expected"
        };
        context.emit_stage(stage, stage_fields([("attempt", attempt.into())]));
        match context.services.commands.request_camera_permission() {
            CommandOutcome::Queued => ScenarioStep::Continue,
            other => ScenarioStep::Finished(failed(format!(
                "запрос разрешения (попытка {attempt}) не принят оболочкой: {other:?}"
            ))),
        }
    }
}

impl ObservingScenario for PermissionScenario {
    fn name(&self) -> &'static str {
        "permission"
    }

    fn start(&mut self, context: &mut ScenarioContext<'_>) -> ScenarioStep {
        self.request_next(context)
    }

    fn on_event(&mut self, event: ObservedEvent, context: &mut ScenarioContext<'_>) -> ScenarioStep {
        if event.kind == ShellEventKind::PlatformCommandFailed
            && event.failed_command == Some(PlatformCommand::RequestCameraPermission)
        {
            return ScenarioStep::Finished(failed("платформа не смогла показать запрос разрешения"));
        }
        let Some(answer) = event.camera_permission else {
            return ScenarioStep::Continue;
        };
        self.answers.push(answer);
        if self.answers.len() >= REQUEST_ATTEMPTS {
            return ScenarioStep::Finished(passed(PermissionReport {
                answers: self.answers.clone(),
            }));
        }
        self.request_next(context)
    }

    fn on_tick(&mut self, _context: &mut ScenarioContext<'_>) -> ScenarioStep {
        ScenarioStep::Continue
    }
}

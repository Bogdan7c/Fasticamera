//! Журнал подсказок владельцу: что показано и дошло ли до платформы (общий для `3a` и `ae-trace`).
//!
//! Подсказка, которая не дошла до экрана, — не тишина: владелец не знает, когда закрывать объектив или
//! щёлкать лампой, и прогон получится пустым. Поэтому каждый показ записывается вместе с исходом команды,
//! а отказы (всё, кроме `Queued`) попадают в вердикт понятным русским текстом.

use shell_contract::commands::CommandOutcome;
use shell_contract::operator_prompt::{OperatorPrompt, OperatorPromptService};

/// Журнал показов подсказок.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct PromptLog {
    /// Сколько подсказок платформа приняла в очередь.
    pub queued: u32,
    /// Подсказки, которые не приняты: когда были нужны, что показывали и чем кончилось.
    pub failures: Vec<String>,
}

impl PromptLog {
    /// Показывает подсказку через сервис и записывает исход. `moment` — когда она нужна (для читаемого нарушения).
    pub fn show(
        &mut self,
        service: &dyn OperatorPromptService,
        moment: &str,
        prompt: &OperatorPrompt,
    ) -> CommandOutcome {
        let outcome = service.show(prompt);
        self.record(moment, prompt, outcome);
        outcome
    }

    /// Записывает исход одного показа.
    pub fn record(&mut self, moment: &str, prompt: &OperatorPrompt, outcome: CommandOutcome) {
        match outcome {
            CommandOutcome::Queued => self.queued += 1,
            CommandOutcome::NotAttached | CommandOutcome::Backpressure => {
                self.failures
                    .push(format!("{moment} ({}): {outcome:?}", prompt.headline()));
            }
        }
    }
}

/// Сколько целых секунд показывать в отсчёте при остатке `remaining_ms`: округление вверх, чтобы «1» висело
/// до самого момента действия, а не «0» за долю секунды до него.
pub fn countdown_seconds_left(remaining_ms: u64, step_ms: u64) -> u64 {
    remaining_ms.div_ceil(step_ms.max(1))
}

#[cfg(test)]
#[path = "operator_prompt_log_tests.rs"]
mod tests;

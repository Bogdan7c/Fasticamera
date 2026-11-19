//! Fake-сервис подсказок владельцу для тестов: записывает всё показанное и снятое по порядку.

use std::sync::Mutex;

use shell_contract::commands::CommandOutcome;
use shell_contract::operator_prompt::{OperatorPrompt, OperatorPromptKind, OperatorPromptService};

/// Что сделали с экраном подсказок.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromptEvent {
    Shown(OperatorPrompt),
    Cleared,
}

/// Запись подсказок с настраиваемым исходом (по умолчанию — «поставлено в очередь»).
#[derive(Default)]
pub struct FakePrompts {
    events: Mutex<Vec<PromptEvent>>,
    outcome: Mutex<Option<CommandOutcome>>,
}

impl FakePrompts {
    /// Все события по порядку.
    pub fn events(&self) -> Vec<PromptEvent> {
        self.events.lock().expect("замок").clone()
    }

    /// Только показанные подсказки по порядку.
    pub fn shown(&self) -> Vec<OperatorPrompt> {
        self.events()
            .into_iter()
            .filter_map(|event| match event {
                PromptEvent::Shown(prompt) => Some(prompt),
                PromptEvent::Cleared => None,
            })
            .collect()
    }

    /// Виды показанных подсказок по порядку.
    pub fn shown_kinds(&self) -> Vec<OperatorPromptKind> {
        self.shown().iter().map(OperatorPrompt::kind).collect()
    }

    /// Сколько раз подсказку снимали.
    pub fn clear_count(&self) -> usize {
        self.events()
            .iter()
            .filter(|event| matches!(event, PromptEvent::Cleared))
            .count()
    }

    /// Ответ платформы на все следующие команды (`None` — снова «в очереди»).
    pub fn set_outcome(&self, outcome: Option<CommandOutcome>) {
        *self.outcome.lock().expect("замок") = outcome;
    }

    fn answer(&self) -> CommandOutcome {
        (*self.outcome.lock().expect("замок")).unwrap_or(CommandOutcome::Queued)
    }
}

impl OperatorPromptService for FakePrompts {
    fn show(&self, prompt: &OperatorPrompt) -> CommandOutcome {
        self.events
            .lock()
            .expect("замок")
            .push(PromptEvent::Shown(prompt.clone()));
        self.answer()
    }

    fn clear(&self) -> CommandOutcome {
        self.events.lock().expect("замок").push(PromptEvent::Cleared);
        self.answer()
    }
}

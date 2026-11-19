//! Журнал подсказок: принятые считаются, отказы попадают в текст с исходом платформы.

use std::sync::Mutex;

use shell_contract::operator_prompt::OperatorPromptKind;

use super::*;

/// Сервис-запись: помнит показанное и отвечает заданным исходом.
struct RecordingPrompts {
    shown: Mutex<Vec<OperatorPrompt>>,
    outcome: CommandOutcome,
}

impl OperatorPromptService for RecordingPrompts {
    fn show(&self, prompt: &OperatorPrompt) -> CommandOutcome {
        self.shown.lock().expect("замок").push(prompt.clone());
        self.outcome
    }

    fn clear(&self) -> CommandOutcome {
        self.outcome
    }
}

fn prompt() -> OperatorPrompt {
    OperatorPrompt::new(OperatorPromptKind::Act, "ЩЁЛКНИ ЛАМПОЙ", "ступенька 1")
}

#[test]
fn queued_prompt_is_counted_and_reaches_the_service() {
    let service = RecordingPrompts {
        shown: Mutex::new(Vec::new()),
        outcome: CommandOutcome::Queued,
    };
    let mut log = PromptLog::default();
    let outcome = log.show(&service, "момент", &prompt());
    assert_eq!(outcome, CommandOutcome::Queued);
    assert_eq!(log.queued, 1);
    assert!(log.failures.is_empty());
    assert_eq!(service.shown.lock().expect("замок").as_slice(), &[prompt()]);
}

#[test]
fn refused_prompts_are_logged_with_moment_text_and_outcome() {
    for outcome in [CommandOutcome::NotAttached, CommandOutcome::Backpressure] {
        let service = RecordingPrompts {
            shown: Mutex::new(Vec::new()),
            outcome,
        };
        let mut log = PromptLog::default();
        assert_eq!(log.show(&service, "подсказка 1", &prompt()), outcome);
        assert_eq!(log.queued, 0);
        assert_eq!(log.failures, vec![format!("подсказка 1 (ЩЁЛКНИ ЛАМПОЙ): {outcome:?}")]);
    }
}

#[test]
fn countdown_rounds_up_so_the_last_second_shows_one() {
    assert_eq!(countdown_seconds_left(3_000, 1_000), 3);
    assert_eq!(countdown_seconds_left(2_001, 1_000), 3);
    assert_eq!(countdown_seconds_left(1_000, 1_000), 1);
    assert_eq!(countdown_seconds_left(1, 1_000), 1);
    assert_eq!(countdown_seconds_left(0, 1_000), 0);
}

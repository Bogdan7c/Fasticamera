//! Тесты исполнителя просьб о тексте: каждый исход брокера даёт своё обратное действие и свой счётчик;
//! приёмник ответов кладёт ответ диалога в канал входа экрана, а при переполнении ответ считается потерянным.

use std::sync::{Arc, Mutex};

use shell_contract::commands::CommandOutcome;
use shell_contract::text_input::{TextInputOutcome, TextInputService, TextRequest};

use super::*;
use crate::camera_screen_input::{ScreenInput, screen_input_channel};

/// Подделка платформы: помнит показанные запросы, отвечает заданным итогом постановки.
struct FakeTextShell {
    shown: Mutex<Vec<TextRequest>>,
    outcome: Mutex<CommandOutcome>,
}

impl FakeTextShell {
    fn new(outcome: CommandOutcome) -> Arc<Self> {
        Arc::new(Self {
            shown: Mutex::default(),
            outcome: Mutex::new(outcome),
        })
    }

    fn shown(&self) -> Vec<TextRequest> {
        self.shown.lock().expect("замок").clone()
    }
}

impl TextInputService for FakeTextShell {
    fn request_text(&self, request: &TextRequest) -> CommandOutcome {
        let outcome = *self.outcome.lock().expect("замок");
        if outcome == CommandOutcome::Queued {
            self.shown.lock().expect("замок").push(request.clone());
        }
        outcome
    }
}

fn effects_over(shell: &Arc<FakeTextShell>) -> TextEffects {
    TextEffects::new(Some(TextInputBroker::new(
        Arc::clone(shell) as Arc<dyn TextInputService>
    )))
}

#[test]
fn a_shown_dialog_returns_its_number_and_the_platform_gets_the_name_limit() {
    let shell = FakeTextShell::new(CommandOutcome::Queued);
    let mut effects = effects_over(&shell);

    let feedback = effects.request(TextPurpose::RenamePreset, "Sunset".to_owned());

    let shown = shell.shown();
    assert_eq!(shown.len(), 1);
    assert_eq!(feedback, ScreenAction::TextRequestIssued { id: shown[0].id });
    assert_eq!(shown[0].purpose, TextPurpose::RenamePreset);
    assert_eq!(shown[0].initial_text, "Sunset");
    assert_eq!(shown[0].max_chars as usize, PRESET_NAME_MAX_CHARS);
    assert_eq!(effects.counters().shown, 1);
}

#[test]
fn a_second_request_supersedes_the_first_and_is_counted_separately() {
    let shell = FakeTextShell::new(CommandOutcome::Queued);
    let mut effects = effects_over(&shell);
    effects.request(TextPurpose::NewPresetName, String::new());

    let feedback = effects.request(TextPurpose::NewPresetName, String::new());

    let shown = shell.shown();
    assert_eq!(feedback, ScreenAction::TextRequestIssued { id: shown[1].id });
    assert_ne!(shown[0].id, shown[1].id, "номера запросов разные");
    let counters = effects.counters();
    assert_eq!((counters.shown, counters.shown_replacing), (1, 1));
}

#[test]
fn refusals_of_the_platform_and_a_missing_broker_tell_the_screen_nothing_was_sent() {
    for (outcome, check) in [
        (CommandOutcome::NotAttached, (1, 0, 0)),
        (CommandOutcome::Backpressure, (0, 1, 0)),
    ] {
        let shell = FakeTextShell::new(outcome);
        let mut effects = effects_over(&shell);
        assert_eq!(
            effects.request(TextPurpose::NewPresetName, String::new()),
            ScreenAction::TextRequestNotSent
        );
        let counters = effects.counters();
        assert_eq!(
            (counters.not_attached, counters.backpressure, counters.shown),
            check,
            "{outcome:?}"
        );
        assert!(shell.shown().is_empty());
    }

    let mut without_broker = TextEffects::default();
    assert_eq!(
        without_broker.request(TextPurpose::NewPresetName, String::new()),
        ScreenAction::TextRequestNotSent
    );
    assert_eq!(without_broker.counters().without_broker, 1);
}

#[test]
fn the_sink_puts_the_dialog_answer_into_the_screen_input_and_counts_an_overflow() {
    let (sender, mut receiver) = screen_input_channel(1);
    let sink = TextResultToScreen::new(sender.injector());
    let answer = |id| TextInputResult {
        id: TextRequestId(id),
        outcome: TextInputOutcome::Entered("Sunset".to_owned()),
    };

    sink.deliver(answer(7));
    assert_eq!(
        receiver.try_next(),
        Some(ScreenInput::Action(ScreenAction::TextInputFinished(answer(7))))
    );

    // Очередь ёмкости 1: второй ответ без чтения не влезает и считается потерянным, а не пропадает молча.
    sink.deliver(answer(8));
    sink.deliver(answer(9));
    assert_eq!(sender.stats().overflowed_actions, 1);
}

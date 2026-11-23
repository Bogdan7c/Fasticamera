//! Маршрутизатор доставляет ответ диалога ввода текста брокеру, а тот — приёмнику (решение 197).

use shell_contract::commands::{CommandOutcome, PlatformCommand, PlatformCommandFailure};
use shell_contract::text_input::{
    TextInputOutcome, TextInputResult, TextInputService, TextPurpose, TextRequest, TextRequestId,
};

use super::*;
use crate::text_input_broker::{TextInputBroker, TextRequestOutcome};

/// Платформа, которая принимает все запросы.
struct AcceptingTextShell;

impl TextInputService for AcceptingTextShell {
    fn request_text(&self, _request: &TextRequest) -> CommandOutcome {
        CommandOutcome::Queued
    }
}

/// Маршрутизатор с брокером и приёмником, собирающим итоги.
fn harness_with_text_input() -> (Harness, TextInputBroker, Arc<Mutex<Vec<TextInputResult>>>) {
    let mut harness = harness(ScenarioConfig::default());
    let broker = TextInputBroker::new(Arc::new(AcceptingTextShell));
    let received = Arc::new(Mutex::new(Vec::new()));
    let received_for_sink = Arc::clone(&received);
    broker.set_sink(Arc::new(move |result: TextInputResult| {
        received_for_sink.lock().expect("замок").push(result);
    }));
    harness.router.set_text_input(broker.clone());
    (harness, broker, received)
}

#[test]
fn text_entered_in_the_dialog_reaches_the_sink_through_the_router() {
    let (mut harness, broker, received) = harness_with_text_input();
    let request = broker.new_request(TextPurpose::NewPresetName, "", 20);
    let id = request.id;
    assert_eq!(broker.request(request), TextRequestOutcome::Shown);

    harness.deliver(ShellEvent::TextInput(TextInputResult {
        id,
        outcome: TextInputOutcome::Entered("  Warm film ".to_owned()),
    }));

    assert_eq!(
        *received.lock().expect("замок"),
        vec![TextInputResult {
            id,
            outcome: TextInputOutcome::Entered("Warm film".to_owned())
        }]
    );
}

#[test]
fn stale_dialog_answer_is_not_forwarded_by_the_router() {
    let (mut harness, broker, received) = harness_with_text_input();
    let request = broker.new_request(TextPurpose::NewPresetName, "", 20);
    broker.request(request);

    harness.deliver(ShellEvent::TextInput(TextInputResult {
        id: TextRequestId(9_999),
        outcome: TextInputOutcome::Cancelled,
    }));

    assert!(received.lock().expect("замок").is_empty());
    assert_eq!(broker.stats().stale_results, 1);
}

#[test]
fn platform_failure_to_show_the_dialog_ends_the_request_as_unavailable() {
    let (mut harness, broker, received) = harness_with_text_input();
    let request = broker.new_request(TextPurpose::RenamePreset, "A", 20);
    let id = request.id;
    broker.request(request);

    harness.deliver(ShellEvent::PlatformCommandFailed(PlatformCommandFailure {
        command: PlatformCommand::RequestTextInput,
        reason: "нет окна".to_owned(),
    }));

    assert_eq!(
        *received.lock().expect("замок"),
        vec![TextInputResult {
            id,
            outcome: TextInputOutcome::Unavailable
        }]
    );
}

#[test]
fn other_platform_failures_do_not_touch_the_open_text_request() {
    let (mut harness, broker, received) = harness_with_text_input();
    let request = broker.new_request(TextPurpose::NewPresetName, "", 20);
    let id = request.id;
    broker.request(request);

    harness.deliver(ShellEvent::PlatformCommandFailed(PlatformCommandFailure {
        command: PlatformCommand::OpenSystemGallery,
        reason: "нет галереи".to_owned(),
    }));

    assert!(received.lock().expect("замок").is_empty());
    assert_eq!(broker.open_request_id(), Some(id));
}

#[test]
fn text_answer_without_a_broker_is_handled_without_panic() {
    let mut harness = harness(ScenarioConfig::default());
    harness.deliver(ShellEvent::TextInput(TextInputResult {
        id: TextRequestId(1),
        outcome: TextInputOutcome::Cancelled,
    }));
}

use std::sync::Mutex;

use super::*;

/// Подделка платформы: помнит запросы и отвечает заданным итогом постановки.
struct FakeTextShell {
    requests: Mutex<Vec<TextRequest>>,
    outcome: Mutex<CommandOutcome>,
}

impl FakeTextShell {
    fn new(outcome: CommandOutcome) -> Arc<Self> {
        Arc::new(Self {
            requests: Mutex::new(Vec::new()),
            outcome: Mutex::new(outcome),
        })
    }

    fn set_outcome(&self, outcome: CommandOutcome) {
        *self.outcome.lock().expect("замок") = outcome;
    }

    fn shown(&self) -> Vec<TextRequest> {
        self.requests.lock().expect("замок").clone()
    }
}

impl TextInputService for FakeTextShell {
    fn request_text(&self, request: &TextRequest) -> CommandOutcome {
        let outcome = *self.outcome.lock().expect("замок");
        if outcome == CommandOutcome::Queued {
            self.requests.lock().expect("замок").push(request.clone());
        }
        outcome
    }
}

/// Приёмник, который складывает итоги в список.
#[derive(Clone, Default)]
struct CollectingSink {
    results: Arc<Mutex<Vec<TextInputResult>>>,
}

impl CollectingSink {
    fn results(&self) -> Vec<TextInputResult> {
        self.results.lock().expect("замок").clone()
    }
}

impl TextResultSink for CollectingSink {
    fn deliver(&self, result: TextInputResult) {
        self.results.lock().expect("замок").push(result);
    }
}

fn broker_with_sink() -> (TextInputBroker, Arc<FakeTextShell>, CollectingSink) {
    let shell = FakeTextShell::new(CommandOutcome::Queued);
    let broker = TextInputBroker::new(shell.clone());
    let sink = CollectingSink::default();
    broker.set_sink(Arc::new(sink.clone()));
    (broker, shell, sink)
}

fn entered(id: TextRequestId, text: &str) -> TextInputResult {
    TextInputResult {
        id,
        outcome: TextInputOutcome::Entered(text.to_owned()),
    }
}

#[test]
fn request_ids_grow_monotonically() {
    let (broker, _shell, _sink) = broker_with_sink();
    let first = broker.new_request(TextPurpose::NewPresetName, "", 30);
    let second = broker.new_request(TextPurpose::RenamePreset, "Warm", 30);
    assert!(second.id > first.id);
    assert_eq!(second.initial_text, "Warm");
    assert_eq!(second.purpose, TextPurpose::RenamePreset);
}

#[test]
fn accepted_request_reaches_the_platform_and_becomes_open() {
    let (broker, shell, _sink) = broker_with_sink();
    let request = broker.new_request(TextPurpose::NewPresetName, "Seed", 20);
    assert_eq!(broker.request(request.clone()), TextRequestOutcome::Shown);
    assert_eq!(shell.shown(), vec![request.clone()]);
    assert_eq!(broker.open_request_id(), Some(request.id));
}

#[test]
fn entered_text_is_normalized_and_delivered_to_the_sink_once() {
    let (broker, _shell, sink) = broker_with_sink();
    let request = broker.new_request(TextPurpose::NewPresetName, "", 5);
    let id = request.id;
    broker.request(request);

    let delivery = broker.deliver_platform_result(entered(id, "  Warm film\n"));

    assert_eq!(delivery, ResultDelivery::Forwarded);
    assert_eq!(sink.results(), vec![entered(id, "Warm")]);
    assert_eq!(broker.open_request_id(), None, "запрос закрыт после ответа");
}

#[test]
fn cancelled_and_unavailable_pass_through_unchanged() {
    for outcome in [TextInputOutcome::Cancelled, TextInputOutcome::Unavailable] {
        let (broker, _shell, sink) = broker_with_sink();
        let request = broker.new_request(TextPurpose::RenamePreset, "A", 10);
        let id = request.id;
        broker.request(request);
        broker.deliver_platform_result(TextInputResult {
            id,
            outcome: outcome.clone(),
        });
        assert_eq!(sink.results(), vec![TextInputResult { id, outcome }]);
    }
}

#[test]
fn empty_entry_is_reported_as_cancelled_and_counted() {
    let (broker, _shell, sink) = broker_with_sink();
    let request = broker.new_request(TextPurpose::NewPresetName, "", 10);
    let id = request.id;
    broker.request(request);

    broker.deliver_platform_result(entered(id, " \n "));

    assert_eq!(
        sink.results(),
        vec![TextInputResult {
            id,
            outcome: TextInputOutcome::Cancelled
        }]
    );
    assert_eq!(broker.stats().empty_entries, 1);
}

#[test]
fn new_request_supersedes_the_open_one_and_old_answer_becomes_stale() {
    let (broker, shell, sink) = broker_with_sink();
    let old = broker.new_request(TextPurpose::NewPresetName, "", 10);
    let new = broker.new_request(TextPurpose::RenamePreset, "X", 10);
    let (old_id, new_id) = (old.id, new.id);
    broker.request(old);

    let outcome = broker.request(new);

    assert_eq!(outcome, TextRequestOutcome::ShownReplacing { previous: old_id });
    assert_eq!(shell.shown().len(), 2, "платформе ушли оба запроса");
    assert_eq!(
        sink.results(),
        vec![TextInputResult {
            id: old_id,
            outcome: TextInputOutcome::Superseded
        }]
    );
    assert_eq!(broker.open_request_id(), Some(new_id));

    // Запоздавший ответ старого диалога: не подменяет собой ответ нового.
    assert_eq!(
        broker.deliver_platform_result(entered(old_id, "late")),
        ResultDelivery::Stale
    );
    assert_eq!(sink.results().len(), 1, "приёмник не получил устаревший ответ");
    assert_eq!(broker.stats().stale_results, 1);
    assert_eq!(
        broker.open_request_id(),
        Some(new_id),
        "новый запрос по-прежнему открыт"
    );

    broker.deliver_platform_result(entered(new_id, "fresh"));
    assert_eq!(sink.results()[1], entered(new_id, "fresh"));
}

#[test]
fn refused_command_keeps_the_previous_request_open() {
    let (broker, shell, sink) = broker_with_sink();
    let open = broker.new_request(TextPurpose::NewPresetName, "", 10);
    let open_id = open.id;
    broker.request(open);

    for refusal in [CommandOutcome::NotAttached, CommandOutcome::Backpressure] {
        shell.set_outcome(refusal);
        let rejected = broker.new_request(TextPurpose::RenamePreset, "", 10);
        let expected = match refusal {
            CommandOutcome::NotAttached => TextRequestOutcome::NotAttached,
            _ => TextRequestOutcome::Backpressure,
        };
        assert_eq!(broker.request(rejected), expected);
        assert_eq!(
            broker.open_request_id(),
            Some(open_id),
            "отказ не вытесняет открытый запрос"
        );
    }
    assert!(sink.results().is_empty(), "отказ не порождает итогов");
}

#[test]
fn refused_command_without_open_request_leaves_nothing_open() {
    let shell = FakeTextShell::new(CommandOutcome::NotAttached);
    let broker = TextInputBroker::new(shell);
    let request = broker.new_request(TextPurpose::NewPresetName, "", 10);
    assert_eq!(broker.request(request), TextRequestOutcome::NotAttached);
    assert_eq!(broker.open_request_id(), None);
}

#[test]
fn answer_without_any_open_request_is_stale() {
    let (broker, _shell, sink) = broker_with_sink();
    assert_eq!(
        broker.deliver_platform_result(entered(TextRequestId(99), "ghost")),
        ResultDelivery::Stale
    );
    assert!(sink.results().is_empty());
}

#[test]
fn answer_is_delivered_only_once() {
    let (broker, _shell, sink) = broker_with_sink();
    let request = broker.new_request(TextPurpose::NewPresetName, "", 10);
    let id = request.id;
    broker.request(request);
    assert_eq!(
        broker.deliver_platform_result(entered(id, "A")),
        ResultDelivery::Forwarded
    );
    assert_eq!(broker.deliver_platform_result(entered(id, "A")), ResultDelivery::Stale);
    assert_eq!(sink.results().len(), 1);
}

#[test]
fn platform_refusal_ends_the_open_request_as_unavailable() {
    let (broker, _shell, sink) = broker_with_sink();
    let request = broker.new_request(TextPurpose::NewPresetName, "", 10);
    let id = request.id;
    broker.request(request);

    assert_eq!(broker.platform_refused(), ResultDelivery::Forwarded);

    assert_eq!(
        sink.results(),
        vec![TextInputResult {
            id,
            outcome: TextInputOutcome::Unavailable
        }]
    );
    assert_eq!(broker.open_request_id(), None);
}

#[test]
fn platform_refusal_without_open_request_is_a_noop() {
    let (broker, _shell, sink) = broker_with_sink();
    assert_eq!(broker.platform_refused(), ResultDelivery::Stale);
    assert!(sink.results().is_empty());
}

#[test]
fn answer_without_a_sink_closes_the_request_and_is_counted() {
    let shell = FakeTextShell::new(CommandOutcome::Queued);
    let broker = TextInputBroker::new(shell);
    let request = broker.new_request(TextPurpose::NewPresetName, "", 10);
    let id = request.id;
    broker.request(request);

    assert_eq!(broker.deliver_platform_result(entered(id, "A")), ResultDelivery::NoSink);

    assert_eq!(broker.stats().results_without_sink, 1);
    assert_eq!(broker.open_request_id(), None);
}

#[test]
fn closure_works_as_a_sink() {
    let shell = FakeTextShell::new(CommandOutcome::Queued);
    let broker = TextInputBroker::new(shell);
    let received = Arc::new(Mutex::new(Vec::new()));
    let received_for_sink = Arc::clone(&received);
    broker.set_sink(Arc::new(move |result: TextInputResult| {
        received_for_sink.lock().expect("замок").push(result);
    }));
    let request = broker.new_request(TextPurpose::NewPresetName, "", 10);
    let id = request.id;
    broker.request(request);
    broker.deliver_platform_result(entered(id, "Z"));
    assert_eq!(*received.lock().expect("замок"), vec![entered(id, "Z")]);
}

#[test]
fn sink_may_call_the_broker_again_without_deadlock() {
    // Приёмник вызывается без замка: из него можно сразу поставить следующий запрос (например, повторить ввод).
    let shell = FakeTextShell::new(CommandOutcome::Queued);
    let broker = TextInputBroker::new(shell.clone());
    let broker_for_sink = broker.clone();
    broker.set_sink(Arc::new(move |_result: TextInputResult| {
        let next = broker_for_sink.new_request(TextPurpose::NewPresetName, "", 10);
        broker_for_sink.request(next);
    }));
    let first = broker.new_request(TextPurpose::NewPresetName, "", 10);
    let first_id = first.id;
    broker.request(first);
    broker.deliver_platform_result(entered(first_id, "A"));
    assert_eq!(shell.shown().len(), 2);
    assert_ne!(broker.open_request_id(), Some(first_id));
}

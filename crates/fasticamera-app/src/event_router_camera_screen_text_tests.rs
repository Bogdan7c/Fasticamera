//! Ответ диалога ввода текста доходит до ЭКРАНА камеры, который сейчас стоит (14b, решение 197): настоящий
//! маршрутизатор, настоящий брокер, настоящий сервис экрана; платформа диалога подставная.
//!
//! Доказательство — счётчики канала входа ТЕКУЩЕГО экрана: ответ платформы (`ShellEvent::TextInput`) → брокер →
//! приёмник сервиса экрана → канал входа → источник UI забрал. После переустановки экрана канал новый, поэтому
//! ответ обязан лечь в счётчики нового канала, а не потеряться в старом.

use shell_contract::commands::CommandOutcome;
use shell_contract::text_input::{TextInputOutcome, TextInputResult, TextInputService, TextPurpose, TextRequest};

use super::*;
use crate::text_input_broker::{TextInputBroker, TextRequestOutcome};

/// Платформа диалога: принимает все запросы.
struct AcceptingDialogs;

impl TextInputService for AcceptingDialogs {
    fn request_text(&self, _request: &TextRequest) -> CommandOutcome {
        CommandOutcome::Queued
    }
}

/// Маршрутизатор с экраном, которому подключён брокер ДО установки (так делает `with_text_input` при запуске).
fn harness_with_text_broker() -> (Harness, TextInputBroker, RenderClock) {
    let mut harness = harness_for_rig(crate::camera_test_support::FakeCameraRig::with_config(
        CameraConfig::default(),
    ));
    harness.router.camera_screen = CameraScreenService::new(instant_capture_config());
    let broker = TextInputBroker::new(Arc::new(AcceptingDialogs));
    harness.router.set_text_input(broker.clone());
    harness.router.install_camera_screen();
    start_streaming(&mut harness);
    let mut clock = RenderClock::new();
    clock.frames(&harness, 2);
    (harness, broker, clock)
}

/// Платформа отвечает на запрос `purpose` текстом `text` событием оболочки.
fn answer_dialog(harness: &mut Harness, broker: &TextInputBroker, text: &str) {
    let request = broker.new_request(TextPurpose::NewPresetName, "", 40);
    let id = request.id;
    assert_eq!(broker.request(request), TextRequestOutcome::Shown);
    harness.deliver(ShellEvent::TextInput(TextInputResult {
        id,
        outcome: TextInputOutcome::Entered(text.to_owned()),
    }));
}

#[test]
fn a_dialog_answer_travels_through_the_router_and_broker_into_the_installed_screen_input() {
    let (mut harness, broker, mut clock) = harness_with_text_broker();
    let before = harness.router.camera_screen().report().expect("отчёт").input;

    answer_dialog(&mut harness, &broker, "Sunset");
    clock.frame(&harness);

    let after = harness.router.camera_screen().report().expect("отчёт").input;
    assert_eq!(
        after.queued_actions,
        before.queued_actions + 1,
        "ответ лёг в канал входа экрана"
    );
    assert_eq!(after.received, before.received + 1, "источник UI забрал его на кадре");
    assert_eq!(after.overflowed_actions, 0);
}

#[test]
fn after_a_reinstall_the_answer_goes_to_the_fresh_screen_not_the_old_one() {
    let (mut harness, broker, mut clock) = harness_with_text_broker();
    let finish = start_probe(&mut harness, CameraScreenPolicy::SuspendForScenario, false);
    finish_probe(&mut harness, &finish);
    assert_eq!(
        harness.router.camera_screen().installs(),
        2,
        "экран переустановлен свежим"
    );
    clock.frame(&harness);
    let fresh_before = harness
        .router
        .camera_screen()
        .report()
        .expect("отчёт нового экрана")
        .input;
    assert_eq!(fresh_before.queued_actions, 0, "канал нового экрана ещё пуст");

    answer_dialog(&mut harness, &broker, "Night");
    clock.frame(&harness);

    let fresh_after = harness.router.camera_screen().report().expect("отчёт").input;
    assert_eq!(fresh_after.queued_actions, 1, "ответ лёг в канал НОВОГО экрана");
    assert_eq!(fresh_after.received, 1);
}

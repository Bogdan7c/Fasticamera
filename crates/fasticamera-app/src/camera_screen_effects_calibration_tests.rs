//! Эффекты раздела калибровки в исполнителе (14b): «чистяк», просьба спросить имя и приведение показа вида при
//! установке нового экрана. Порт рендера и платформа диалога подставные, результат — то, что получили рендер и экран.

use look_contract::{CleanView, LookIntent, LookSettings};
use shell_contract::commands::CommandOutcome;
use shell_contract::text_input::{TextInputService, TextPurpose, TextRequest};

use super::*;
use crate::look_service::LookHandle;
use crate::look_test_support::{StubPersistence, contrast_look, handle_with};
use crate::viewfinder_control::RenderAdjustment;

/// Платформа диалога: принимает запросы и помнит их.
#[derive(Default)]
struct FakeDialogs {
    shown: Mutex<Vec<TextRequest>>,
}

impl TextInputService for FakeDialogs {
    fn request_text(&self, request: &TextRequest) -> CommandOutcome {
        self.shown.lock().expect("замок").push(request.clone());
        CommandOutcome::Queued
    }
}

fn executor_with_library() -> (EffectExecutor, Rig, LookHandle) {
    let handle = handle_with(StubPersistence::accepting());
    let (executor, rig) = build_executor(Behavior {
        look: Some(handle.clone()),
        ..Behavior::default()
    });
    (executor, rig, handle)
}

/// Значения, которые рендер получил командами `SetLook`, по порядку.
fn shown_looks(rig: &Rig) -> Vec<LookSettings> {
    rig.render
        .recorded()
        .into_iter()
        .filter_map(|adjustment| match adjustment {
            RenderAdjustment::Viewfinder(ViewfinderCommand::SetLook(look)) => Some((*look).clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn the_clean_view_effect_shows_neutral_while_held_and_the_edited_look_after_release_without_touching_the_library() {
    let (mut executor, rig, library) = executor_with_library();
    executor.execute(vec![Effect::Look(LookIntent::SetLook(contrast_look(0.5)))]);

    executor.execute(vec![Effect::CleanView(CleanView::ShowBase)]);
    assert_eq!(
        shown_looks(&rig).last(),
        Some(&LookSettings::NEUTRAL),
        "зажат: рендер на нейтрали"
    );
    assert_eq!(
        library.active_settings(),
        contrast_look(0.5),
        "библиотека правок не потеряла"
    );

    executor.execute(vec![Effect::CleanView(CleanView::ShowEdited)]);
    assert_eq!(
        shown_looks(&rig).last(),
        Some(&contrast_look(0.5)),
        "отпущен: правки вернулись"
    );
    assert_eq!(library.active_settings(), contrast_look(0.5));
}

#[test]
fn a_text_request_effect_asks_the_platform_and_answers_the_screen_with_the_dialog_number() {
    let (mut executor, _rig, _library) = executor_with_library();
    let dialogs = Arc::new(FakeDialogs::default());
    executor = executor.with_text_input(TextInputBroker::new(Arc::clone(&dialogs) as Arc<dyn TextInputService>));

    let feedback = executor.execute(vec![Effect::RequestText {
        purpose: TextPurpose::NewPresetName,
        initial_text: String::new(),
    }]);

    let shown = dialogs.shown.lock().expect("замок").clone();
    assert_eq!(shown.len(), 1, "платформа получила запрос");
    assert_eq!(feedback, vec![ScreenAction::TextRequestIssued { id: shown[0].id }]);
    assert_eq!(executor.counters().text.shown, 1);
}

#[test]
fn a_text_request_without_a_broker_tells_the_screen_it_was_not_sent() {
    let (mut executor, _rig, _library) = executor_with_library();

    let feedback = executor.execute(vec![Effect::RequestText {
        purpose: TextPurpose::RenamePreset,
        initial_text: "Sunset".to_owned(),
    }]);

    assert_eq!(feedback, vec![ScreenAction::TextRequestNotSent]);
    assert_eq!(executor.counters().text.without_broker, 1);
}

#[test]
fn a_new_screen_releases_the_clean_view_the_previous_screen_left_held() {
    let handle = handle_with(StubPersistence::accepting());
    handle
        .apply(LookIntent::SetLook(contrast_look(0.5)))
        .expect("правка принята");
    // Прежний экран: «чистяк» зажат, рендер на нейтрали.
    let (mut previous, previous_rig) = build_executor(Behavior {
        look: Some(handle.clone()),
        ..Behavior::default()
    });
    previous.execute(vec![Effect::CleanView(CleanView::ShowBase)]);
    assert_eq!(shown_looks(&previous_rig).last(), Some(&LookSettings::NEUTRAL));

    // Новый экран пришёл на тот же рендер: его исполнитель не знает про зажатый «чистяк» прежнего.
    let (mut fresh, fresh_rig) = build_executor(Behavior {
        look: Some(handle.clone()),
        ..Behavior::default()
    });
    fresh.on_screen_installed();

    assert_eq!(
        shown_looks(&fresh_rig),
        vec![contrast_look(0.5)],
        "рендеру безусловно ушёл действующий вид библиотеки"
    );
    // После этого «чистяк» нового экрана работает как обычно: нейтраль и обратно.
    fresh.execute(vec![Effect::CleanView(CleanView::ShowBase)]);
    assert_eq!(shown_looks(&fresh_rig).last(), Some(&LookSettings::NEUTRAL));
}

#[test]
fn a_screen_install_without_a_library_sends_the_render_nothing() {
    let (mut executor, rig) = build_executor(Behavior::default());

    executor.on_screen_installed();

    assert!(rig.render.recorded().is_empty());
}

//! Тесты сервиса вида: намерение → что изменилось → слать ли вид рендеру и просить ли запись файла.

use look_contract::{BuiltinPreset, LookIntent, PresetError, PresetId, PresetName};
use look_presets::{LibraryChange, PresetStoreConfig, SaveRequestOutcome};

use super::*;
use crate::look_test_support::{StubPersistence, contrast_look, handle_with};

fn name(raw: &str) -> PresetName {
    PresetName::new(raw).expect("имя")
}

#[test]
fn a_new_look_goes_to_the_render_and_to_the_file_with_the_same_values() {
    let stub = StubPersistence::accepting();
    let handle = handle_with(stub.clone());

    let applied = handle
        .apply(LookIntent::SetLook(contrast_look(0.5)))
        .expect("применено");

    assert_eq!(applied.change, LibraryChange::VisibleLookAndStoredState);
    assert_eq!(applied.look_for_render.as_deref(), Some(&contrast_look(0.5)));
    assert_eq!(applied.save_request, Some(SaveRequestOutcome::Queued));
    // В файл уходит снимок библиотеки именно с этими правками поверх активного пресета.
    let requested = stub.requested();
    assert_eq!(requested.len(), 1);
    assert_eq!(requested[0].edits, Some(contrast_look(0.5)));
    assert_eq!(handle.active_settings(), contrast_look(0.5));
}

#[test]
fn renaming_changes_only_the_stored_state_so_the_render_gets_nothing() {
    let stub = StubPersistence::accepting();
    let handle = handle_with(stub.clone());
    handle.apply(LookIntent::SetLook(contrast_look(0.5))).expect("правки");
    handle
        .apply(LookIntent::SaveAsNewPreset(name("Mine")))
        .expect("сохранён как пресет");
    let user = match handle.active_look().preset {
        PresetId::User(id) => id,
        other => panic!("активным должен стать новый пресет, а не {other:?}"),
    };
    let requests_before = stub.requested().len();

    let applied = handle
        .apply(LookIntent::RenamePreset {
            preset: user,
            name: name("Renamed"),
        })
        .expect("переименован");

    assert_eq!(applied.change, LibraryChange::StoredStateOnly);
    assert!(applied.look_for_render.is_none(), "картинка не изменилась");
    assert_eq!(stub.requested().len(), requests_before + 1, "а файл переписать надо");
}

#[test]
fn a_repeat_that_changes_nothing_sends_nothing_and_asks_for_no_write() {
    let stub = StubPersistence::accepting();
    let handle = handle_with(stub.clone());

    let applied = handle
        .apply(LookIntent::SelectPreset(handle.active_look().preset))
        .expect("выбор текущего");

    assert_eq!(applied.change, LibraryChange::Unchanged);
    assert!(applied.look_for_render.is_none());
    assert_eq!(applied.save_request, None);
    assert!(stub.requested().is_empty());
}

#[test]
fn a_rejected_intent_returns_the_distinct_error_and_leaves_state_and_file_alone() {
    let stub = StubPersistence::accepting();
    let handle = handle_with(stub.clone());
    handle.apply(LookIntent::SetLook(contrast_look(0.5))).expect("правки");
    let before = handle.active_look();
    let requests_before = stub.requested().len();

    let builtin_name = PresetStoreConfig::default().builtin_names.honest;
    let taken = handle.apply(LookIntent::SaveAsNewPreset(name(&builtin_name)));
    let read_only = handle.apply(LookIntent::OverwritePreset(look_contract::UserPresetId(999)));

    assert_eq!(taken.unwrap_err(), PresetError::NameTaken);
    assert!(matches!(read_only.unwrap_err(), PresetError::NotFound(_)));
    assert_eq!(handle.active_look(), before, "отказ не меняет вид");
    assert_eq!(stub.requested().len(), requests_before, "и не просит запись");
}

#[test]
fn without_a_writer_the_request_is_reported_closed_and_the_look_still_applies() {
    let config = PresetStoreConfig::default();
    let handle = LookHandle::in_memory(&config).expect("конфиг по умолчанию верен");

    let applied = handle
        .apply(LookIntent::SetLook(contrast_look(0.25)))
        .expect("применено");

    assert_eq!(applied.save_request, Some(SaveRequestOutcome::Closed));
    assert_eq!(handle.active_settings(), contrast_look(0.25), "вид живёт в памяти");
    assert_eq!(handle.startup(), LookStartup::InMemoryOnly);
}

#[test]
fn a_fresh_library_starts_on_the_configured_startup_preset() {
    let config = PresetStoreConfig::default();
    let handle = LookHandle::in_memory(&config).expect("конфиг по умолчанию верен");

    assert_eq!(handle.active_look().preset, PresetId::Builtin(BuiltinPreset::AsEye));
}

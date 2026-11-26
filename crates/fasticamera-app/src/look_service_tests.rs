//! Тесты сервиса вида: намерение → что изменилось → слать ли вид рендеру и просить ли запись файла.

use look_contract::{BuiltinPreset, LookIntent, LookRecipe, PresetError, PresetId, PresetName};
use look_presets::{LibraryChange, PresetStoreConfig, SaveRequestOutcome};

use super::*;
use crate::look_test_support::{StubPersistence, contrast_look, default_environment, handle_with};

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
    assert_eq!(
        requested[0].edits,
        Some(LookRecipe {
            base: handle.active_recipe().base,
            settings: contrast_look(0.5),
        })
    );
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
    let handle = LookHandle::in_memory(&config, &default_environment()).expect("конфиг по умолчанию верен");

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
    let handle = LookHandle::in_memory(&config, &default_environment()).expect("конфиг по умолчанию верен");

    assert_eq!(handle.active_look().preset, PresetId::Builtin(BuiltinPreset::AsEye));
}

#[test]
fn the_revision_moves_only_when_the_stored_state_changes() {
    let handle = handle_with(StubPersistence::accepting());
    let start = handle.revision();

    handle
        .apply(LookIntent::SetLook(contrast_look(0.5)))
        .expect("применено");
    let after_edit = handle.revision();
    assert_eq!(after_edit, start + 1, "правка двигает номер");

    handle
        .apply(LookIntent::SetLook(contrast_look(0.5)))
        .expect("применено");
    assert_eq!(handle.revision(), after_edit, "то же значение: ничего не изменилось");

    let rejected = handle.apply(LookIntent::RenamePreset {
        preset: look_contract::UserPresetId(99),
        name: name("Ghost"),
    });
    assert!(rejected.is_err());
    assert_eq!(handle.revision(), after_edit, "отказ номер не двигает");

    handle
        .apply(LookIntent::SaveAsNewPreset(name("Mine")))
        .expect("применено");
    assert_eq!(
        handle.revision(),
        after_edit + 1,
        "новый пресет (список изменился, вид тот же)"
    );
}

#[test]
fn only_the_bad_startup_outcomes_have_a_problem_for_the_screen() {
    assert_eq!(
        LookStartup::Quarantined.problem(),
        Some(look_contract::LookProblemKind::PresetsReset)
    );
    for refused in [LookStartup::QuarantineFailed, LookStartup::Unreadable] {
        assert_eq!(
            refused.problem(),
            Some(look_contract::LookProblemKind::SavesRefused),
            "{refused:?}"
        );
    }
    for normal in [
        LookStartup::InMemoryOnly,
        LookStartup::NoFileYet,
        LookStartup::Loaded,
        LookStartup::Migrated { from_version: 1 },
    ] {
        assert_eq!(normal.problem(), None, "{normal:?}");
    }
}

#[test]
fn the_startup_problem_is_held_until_a_screen_confirms_it_was_shown_through_the_shared_handle() {
    let library = look_presets::PresetLibrary::new(
        PresetStoreConfig::default()
            .builtin_catalog(&default_environment())
            .expect("каталог"),
    );
    let handle = LookHandle::new(LookService::new(
        library,
        Box::new(crate::look_service::NoLookPersistence),
        LookStartup::Quarantined,
    ));
    let other_clone = handle.clone();
    assert_eq!(
        handle.pending_startup_problem(),
        Some(look_contract::LookProblemKind::PresetsReset)
    );
    assert_eq!(
        other_clone.pending_startup_problem(),
        Some(look_contract::LookProblemKind::PresetsReset),
        "пока показ не подтверждён, проблема держится (экран мог быть пересоздан до входа в раздел)"
    );
    handle.acknowledge_startup_problem();
    assert_eq!(
        other_clone.pending_startup_problem(),
        None,
        "после подтверждения показа проблему не получает никто"
    );
}

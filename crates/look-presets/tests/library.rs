//! Тесты библиотеки пресетов: намерения, ошибки, учёт изменений.

#![allow(clippy::expect_used, clippy::unwrap_used)]

mod common;

use common::{contrast, name, new_library, with_sky_band};
use look_contract::{BuiltinPreset, EditState, LookIntent, LookSettings, PresetError, PresetId, UserPresetId};
use look_presets::{LibraryChange, LibrarySnapshot, PresetKind, PresetStoreConfig, SnapshotError, UserPreset};

const HONEST: PresetId = PresetId::Builtin(BuiltinPreset::Honest);
const AS_EYE: PresetId = PresetId::Builtin(BuiltinPreset::AsEye);

fn user(id: u64) -> PresetId {
    PresetId::User(UserPresetId(id))
}

#[test]
fn fresh_library_starts_on_as_eye_with_config_values() {
    let library = new_library();
    let active = library.active_look();
    assert_eq!(active.preset, AS_EYE);
    assert_eq!(active.edits, EditState::Clean);
    assert_eq!(active.settings.contrast.get(), 0.2);
    assert_eq!(active.settings.saturation.get(), 0.15);
    assert_eq!(active.settings.vibrance.get(), 0.15);
}

#[test]
fn preset_list_has_builtins_first_then_user_in_creation_order() {
    let mut library = new_library();
    library.save_as_new_preset(name("Zeta")).unwrap();
    library.save_as_new_preset(name("Alpha")).unwrap();
    let listed: Vec<(PresetId, String, bool)> = library
        .presets()
        .into_iter()
        .map(|p| (p.id, p.name.as_str().to_owned(), p.is_builtin()))
        .collect();
    assert_eq!(
        listed,
        vec![
            (HONEST, "Honest".into(), true),
            (AS_EYE, "Natural".into(), true),
            (user(1), "Zeta".into(), false),
            (user(2), "Alpha".into(), false),
        ]
    );
    assert_eq!(library.presets()[2].kind, PresetKind::User);
}

#[test]
fn honest_is_always_neutral_regardless_of_config() {
    let mut library = new_library();
    library.select_preset(HONEST).unwrap();
    assert!(library.active_look().settings.is_neutral());
}

#[test]
fn set_look_edits_on_top_and_matching_values_return_to_clean() {
    let mut library = new_library();
    let preset_values = library.active_look().settings;

    let change = library.set_look(contrast(0.5)).unwrap();
    assert_eq!(change, LibraryChange::VisibleLookAndStoredState);
    let edited = library.active_look();
    assert_eq!(edited.edits, EditState::EditedOnTop);
    assert_eq!(edited.settings.contrast.get(), 0.5);
    assert_eq!(edited.preset, AS_EYE, "пресет под правками не меняется");

    // Те же правки второй раз — ничего не изменилось.
    assert_eq!(library.set_look(contrast(0.5)).unwrap(), LibraryChange::Unchanged);

    // Вернули значения пресета руками: снова Clean.
    let change = library.set_look(preset_values.clone()).unwrap();
    assert!(change.look_changed());
    assert_eq!(library.active_look().edits, EditState::Clean);
    assert_eq!(library.active_look().settings, preset_values);
}

#[test]
fn select_preset_resets_edits_and_reports_unknown_user_preset() {
    let mut library = new_library();
    library.set_look(contrast(-0.4)).unwrap();
    let change = library.select_preset(HONEST).unwrap();
    assert!(change.look_changed() && change.needs_save());
    let active = library.active_look();
    assert_eq!((active.preset, active.edits), (HONEST, EditState::Clean));

    let before = library.snapshot();
    assert_eq!(library.select_preset(user(99)), Err(PresetError::NotFound(user(99))));
    assert_eq!(library.snapshot(), before, "ошибка не меняет состояние");
}

#[test]
fn selecting_active_preset_with_edits_discards_them() {
    let mut library = new_library();
    library.set_look(contrast(0.9)).unwrap();
    let change = library.select_preset(AS_EYE).unwrap();
    assert!(change.look_changed());
    assert_eq!(library.active_look().edits, EditState::Clean);
}

#[test]
fn save_as_new_captures_current_look_and_becomes_active_clean() {
    let mut library = new_library();
    library.set_look(with_sky_band(0.3)).unwrap();
    let change = library.save_as_new_preset(name("  Sky pop ")).unwrap();
    // Видимый вид не изменился (значения те же), но состояние для файла — да.
    assert_eq!(change, LibraryChange::StoredStateOnly);
    let active = library.active_look();
    assert_eq!(active.preset, user(1));
    assert_eq!(active.edits, EditState::Clean);
    assert_eq!(active.settings, with_sky_band(0.3));
    assert_eq!(library.presets()[2].name.as_str(), "Sky pop");
}

#[test]
fn user_ids_are_monotonic_and_never_reused_after_delete() {
    let mut library = new_library();
    library.save_as_new_preset(name("One")).unwrap();
    library.save_as_new_preset(name("Two")).unwrap();
    library.delete_preset(user(2)).unwrap();
    library.save_as_new_preset(name("Three")).unwrap();
    assert_eq!(library.active_look().preset, user(3));
    assert_eq!(library.snapshot().next_id, 4);
}

#[test]
fn name_taken_by_builtin_or_other_user_any_case_and_state_untouched() {
    let mut library = new_library();
    library.save_as_new_preset(name("Sunset")).unwrap();
    library.set_look(contrast(0.7)).unwrap();
    let before = library.snapshot();
    for taken in ["honest", "NATURAL", " Natural ", "sunset", "SUNSET"] {
        assert_eq!(
            library.save_as_new_preset(name(taken)),
            Err(PresetError::NameTaken),
            "{taken}"
        );
    }
    assert_eq!(library.snapshot(), before);
}

#[test]
fn rename_changes_only_stored_state_and_allows_case_change_of_own_name() {
    let mut library = new_library();
    library.save_as_new_preset(name("sunset")).unwrap();
    let change = library.rename_preset(user(1), name("Sunset")).unwrap();
    assert_eq!(change, LibraryChange::StoredStateOnly);
    assert_eq!(library.presets()[2].name.as_str(), "Sunset");
    // Тот же регистр, тот же вид — ничего не изменилось.
    assert_eq!(
        library.rename_preset(user(1), name("Sunset")).unwrap(),
        LibraryChange::Unchanged
    );
}

#[test]
fn rename_errors_are_distinct_and_leave_state_alone() {
    let mut library = new_library();
    library.save_as_new_preset(name("A")).unwrap();
    library.save_as_new_preset(name("B")).unwrap();
    let before = library.snapshot();
    assert_eq!(library.rename_preset(user(1), name("b")), Err(PresetError::NameTaken));
    assert_eq!(
        library.rename_preset(user(1), name("honest")),
        Err(PresetError::NameTaken)
    );
    assert_eq!(
        library.rename_preset(user(42), name("X")),
        Err(PresetError::NotFound(user(42)))
    );
    assert_eq!(
        library.rename_preset(HONEST, name("X")),
        Err(PresetError::BuiltinIsReadOnly(BuiltinPreset::Honest))
    );
    assert_eq!(library.snapshot(), before);
}

#[test]
fn overwrite_stores_current_look_in_preset_and_activates_it() {
    let mut library = new_library();
    library.save_as_new_preset(name("Mine")).unwrap(); // с значениями «Как глаз»
    library.set_look(contrast(-0.6)).unwrap();
    let change = library.overwrite_preset(user(1)).unwrap();
    assert_eq!(change, LibraryChange::StoredStateOnly, "вид на экране не менялся");
    assert_eq!(library.active_look().edits, EditState::Clean);

    // Уйти и вернуться: пресет хранит перезаписанные значения.
    library.select_preset(HONEST).unwrap();
    library.select_preset(user(1)).unwrap();
    assert_eq!(library.active_look().settings.contrast.get(), -0.6);
}

#[test]
fn overwrite_of_inactive_preset_activates_it_with_current_look() {
    let mut library = new_library();
    library.save_as_new_preset(name("Mine")).unwrap();
    library.select_preset(HONEST).unwrap();
    library.set_look(contrast(0.33)).unwrap();
    let change = library.overwrite_preset(user(1)).unwrap();
    assert_eq!(change, LibraryChange::StoredStateOnly);
    let active = library.active_look();
    assert_eq!(active.preset, user(1));
    assert_eq!(active.settings.contrast.get(), 0.33);
}

#[test]
fn overwrite_errors_builtin_and_missing() {
    let mut library = new_library();
    assert_eq!(
        library.overwrite_preset(AS_EYE),
        Err(PresetError::BuiltinIsReadOnly(BuiltinPreset::AsEye))
    );
    assert_eq!(library.overwrite_preset(user(5)), Err(PresetError::NotFound(user(5))));
}

#[test]
fn deleting_active_preset_falls_back_to_as_eye_and_drops_edits() {
    let mut library = new_library();
    library.save_as_new_preset(name("Mine")).unwrap();
    library.set_look(contrast(-0.9)).unwrap();
    let change = library.delete_preset(user(1)).unwrap();
    assert_eq!(change, LibraryChange::VisibleLookAndStoredState);
    let active = library.active_look();
    assert_eq!((active.preset, active.edits), (AS_EYE, EditState::Clean));
    assert_eq!(active.settings.contrast.get(), 0.2);
    assert_eq!(library.presets().len(), 2);
}

#[test]
fn deleting_inactive_preset_keeps_look_and_edits() {
    let mut library = new_library();
    library.save_as_new_preset(name("Mine")).unwrap();
    library.select_preset(HONEST).unwrap();
    library.set_look(contrast(0.1)).unwrap();
    let change = library.delete_preset(user(1)).unwrap();
    assert_eq!(change, LibraryChange::StoredStateOnly);
    let active = library.active_look();
    assert_eq!((active.preset, active.edits), (HONEST, EditState::EditedOnTop));
    assert_eq!(active.settings.contrast.get(), 0.1);
}

#[test]
fn delete_errors_builtin_and_missing() {
    let mut library = new_library();
    assert_eq!(
        library.delete_preset(HONEST),
        Err(PresetError::BuiltinIsReadOnly(BuiltinPreset::Honest))
    );
    assert_eq!(library.delete_preset(user(1)), Err(PresetError::NotFound(user(1))));
}

#[test]
fn discard_edits_restores_preset_values_and_is_noop_when_clean() {
    let mut library = new_library();
    assert_eq!(library.discard_edits().unwrap(), LibraryChange::Unchanged);
    library.set_look(contrast(0.9)).unwrap();
    assert!(library.discard_edits().unwrap().look_changed());
    assert_eq!(library.active_look().settings.contrast.get(), 0.2);
}

#[test]
fn apply_routes_every_intent() {
    let mut library = new_library();
    library.apply(LookIntent::SetLook(contrast(0.4))).unwrap();
    library.apply(LookIntent::SaveAsNewPreset(name("P"))).unwrap();
    library
        .apply(LookIntent::RenamePreset {
            preset: UserPresetId(1),
            name: name("Q"),
        })
        .unwrap();
    library.apply(LookIntent::SetLook(contrast(-0.2))).unwrap();
    library.apply(LookIntent::OverwritePreset(UserPresetId(1))).unwrap();
    library.apply(LookIntent::SelectPreset(HONEST)).unwrap();
    assert!(library.active_look().settings.is_neutral());
    library.apply(LookIntent::SelectPreset(user(1))).unwrap();
    assert_eq!(library.active_look().settings.contrast.get(), -0.2);
    library.apply(LookIntent::SetLook(contrast(0.8))).unwrap();
    library.apply(LookIntent::DiscardEdits).unwrap();
    assert_eq!(library.active_look().settings.contrast.get(), -0.2);
    library.apply(LookIntent::DeletePreset(UserPresetId(1))).unwrap();
    assert_eq!(library.active_look().preset, AS_EYE);
    assert_eq!(library.presets().len(), 2);
}

#[test]
fn from_snapshot_roundtrip_and_rejects_broken_invariants() {
    let catalog = || PresetStoreConfig::default().builtin_catalog().unwrap();
    let mut library = new_library();
    library.save_as_new_preset(name("Mine")).unwrap();
    library.set_look(contrast(0.6)).unwrap();
    let snapshot = library.snapshot();
    let restored = look_presets::PresetLibrary::from_snapshot(catalog(), snapshot.clone()).unwrap();
    assert_eq!(restored.active_look(), library.active_look());

    let preset = |id: u64, label: &str| UserPreset {
        id: UserPresetId(id),
        name: name(label),
        settings: LookSettings::NEUTRAL,
    };
    let broken = |mutate: &dyn Fn(&mut LibrarySnapshot)| {
        let mut broken = snapshot.clone();
        mutate(&mut broken);
        look_presets::PresetLibrary::from_snapshot(catalog(), broken).err()
    };
    assert_eq!(
        broken(&|s| s.user_presets.push(preset(1, "Other"))),
        Some(SnapshotError::DuplicateId(UserPresetId(1)))
    );
    assert!(matches!(
        broken(&|s| s.user_presets.push(preset(2, "mine"))),
        Some(SnapshotError::DuplicateName(_))
    ));
    assert!(matches!(
        broken(&|s| s.user_presets.push(preset(2, "HONEST"))),
        Some(SnapshotError::NameClashesWithBuiltin(_))
    ));
    assert_eq!(
        broken(&|s| s.active = user(7)),
        Some(SnapshotError::ActivePresetMissing(user(7)))
    );
    assert!(matches!(
        broken(&|s| s.next_id = 1),
        Some(SnapshotError::NextIdNotAfterUsed {
            next_id: 1,
            max_used: 1
        })
    ));
}

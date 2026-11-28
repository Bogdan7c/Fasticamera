//! Миграция файла пресетов v1 → v4 (колёса цвета 14i, основа тона 14b, полоски 14t) на настоящих файлах схемы 1.
//!
//! Фикстуры в `tests/fixtures/` записаны кодом до 14i и заморожены (см. README рядом).

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

mod common;

use std::fs;
use std::path::{Path, PathBuf};

use common::{TempDir, contrast, environment, knob, name, with_sky_band};
use look_contract::{
    BuiltinPreset, ColorWheel, ColorWheels, EditState, LookRecipe, LookSettings, PresetId, ToneBaseModel, UserPresetId,
    WheelPush,
};
use look_presets::{
    CorruptionReason, LoadOutcome, OpenedPresets, PRE_MIGRATION_BACKUP_MARKER, PRODUCT_MIGRATIONS, PresetFileStore,
    PresetStoreConfig, QUARANTINE_FILE_MARKER,
};
use serde_json::Value;

const WITH_EDITS: &[u8] = include_bytes!("fixtures/schema_v1_with_edits.json");
const NO_EDITS: &[u8] = include_bytes!("fixtures/schema_v1_no_edits.json");

/// Рецепт на основе AgX: ею шаг 2 → 3 помечает все пресеты старых файлов.
fn agx_recipe(settings: LookSettings) -> LookRecipe {
    LookRecipe {
        base: ToneBaseModel::Agx,
        settings,
    }
}

fn open(dir: &Path) -> OpenedPresets {
    PresetFileStore::open(dir, &PresetStoreConfig::default(), &environment(), PRODUCT_MIGRATIONS)
        .expect("конфиг валиден")
}

fn presets_file(dir: &Path) -> PathBuf {
    dir.join("look-presets.json")
}

fn files_containing(dir: &Path, marker: &str) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.to_string_lossy().contains(marker))
        .collect();
    found.sort();
    found
}

/// Копирует фикстуру во временный каталог как основной файл пресетов.
fn dir_with_file(label: &str, content: &[u8]) -> TempDir {
    let dir = TempDir::new(label);
    fs::write(presets_file(dir.path()), content).unwrap();
    dir
}

#[test]
fn v1_file_with_edits_loads_with_neutral_wheels_everywhere() {
    let dir = dir_with_file("v1-edits", WITH_EDITS);
    let opened = open(dir.path());
    assert_eq!(opened.outcome, LoadOutcome::Loaded { migrated_from: Some(1) });

    let snapshot = opened.library.snapshot();
    assert_eq!(snapshot.next_id, 2);
    assert_eq!(snapshot.active, PresetId::User(UserPresetId(1)));
    assert_eq!(snapshot.user_presets.len(), 1);
    assert_eq!(snapshot.user_presets[0].name, name("Evening"));
    // Evening: контраст +0.3, теплота -0.25, полоса sky с оттенком +0.4; колёса нейтральны.
    let evening = LookSettings {
        warmth: knob(-0.25),
        ..LookSettings {
            bands: with_sky_band(0.4).bands,
            ..contrast(0.3)
        }
    };
    assert_eq!(snapshot.user_presets[0].recipe.settings, evening);
    assert!(snapshot.user_presets[0].recipe.settings.wheels.is_neutral());
    // Правки поверх: контраст -0.5, колёса нейтральны.
    assert_eq!(snapshot.edits, Some(agx_recipe(contrast(-0.5))));
    assert_eq!(snapshot.user_presets[0].recipe.base, ToneBaseModel::Agx);

    let active = opened.library.active_look();
    assert_eq!(active.edits, EditState::EditedOnTop);
    assert_eq!(active.recipe.settings, contrast(-0.5));
}

#[test]
fn v1_file_without_edits_keeps_clean_active_preset() {
    let dir = dir_with_file("v1-noedits", NO_EDITS);
    let opened = open(dir.path());
    assert_eq!(opened.outcome, LoadOutcome::Loaded { migrated_from: Some(1) });

    let snapshot = opened.library.snapshot();
    assert_eq!(snapshot.edits, None);
    assert_eq!(snapshot.user_presets[0].name, name("Punchy"));
    assert_eq!(snapshot.user_presets[0].recipe.settings, contrast(0.6));
    let active = opened.library.active_look();
    assert_eq!(active.preset, snapshot.active);
    assert_eq!(active.edits, EditState::Clean);
    assert_eq!(active.recipe.settings, contrast(0.6));
}

#[test]
fn first_save_backs_up_old_file_byte_exact_then_writes_v4_once() {
    for (label, fixture) in [("bk-edits", WITH_EDITS), ("bk-none", NO_EDITS)] {
        let dir = dir_with_file(label, fixture);
        let opened = open(dir.path());
        assert!(
            files_containing(dir.path(), PRE_MIGRATION_BACKUP_MARKER).is_empty(),
            "{label}: до записи копировать рано"
        );

        opened.store.save(&opened.library.snapshot()).unwrap();
        let backups = files_containing(dir.path(), PRE_MIGRATION_BACKUP_MARKER);
        assert_eq!(backups.len(), 1, "{label}: {backups:?}");
        assert!(backups[0].ends_with("look-presets.json.pre-v1"), "{label}");
        assert_eq!(fs::read(&backups[0]).unwrap(), fixture, "{label}: копия побайтно");

        let written: Value = serde_json::from_slice(&fs::read(presets_file(dir.path())).unwrap()).unwrap();
        assert_eq!(written["schema_version"], 4, "{label}");
        assert_eq!(written["user_presets"][0]["base"], "agx", "{label}");
        assert!(written["user_presets"][0]["settings"]["wheels"].is_object(), "{label}");
        assert!(
            written["user_presets"][0]["settings"]["primaries"].is_object(),
            "{label}"
        );
        assert_eq!(written["edits"].is_object(), fixture == WITH_EDITS, "{label}");
        if fixture == WITH_EDITS {
            assert_eq!(written["edits"]["base"], "agx");
            assert!(written["edits"]["settings"]["wheels"].is_object());
        }

        // Второе сохранение копию не плодит.
        opened.store.save(&opened.library.snapshot()).unwrap();
        assert_eq!(
            files_containing(dir.path(), PRE_MIGRATION_BACKUP_MARKER),
            backups,
            "{label}"
        );

        // Повторное открытие читает уже v4 без миграции и даёт то же состояние.
        let reopened = open(dir.path());
        assert_eq!(reopened.outcome, LoadOutcome::Loaded { migrated_from: None }, "{label}");
        assert_eq!(reopened.library.snapshot(), opened.library.snapshot(), "{label}");
    }
}

/// Все четыре колеса тронуты: толчки цвета и кольца яркости разные.
fn touched_wheels() -> ColorWheels {
    let wheel = |hue: f32, strength: f32, ring: f32| ColorWheel {
        push: WheelPush::from_hue_degrees(hue, strength).unwrap(),
        ring: knob(ring),
    };
    ColorWheels {
        lift: wheel(220.0, 0.35, -0.2),
        gamma: wheel(40.0, 0.5, 0.15),
        gain: wheel(75.0, 0.8, 0.4),
        offset: wheel(300.0, 0.1, -0.05),
    }
}

#[test]
fn non_neutral_wheels_and_knobs_roundtrip_through_a_saved_preset() {
    let dir = TempDir::new("wheels-roundtrip");
    let mut first = open(dir.path());
    let look = LookSettings {
        wheels: touched_wheels(),
        warmth: knob(0.3),
        ..with_sky_band(-0.4)
    };
    assert!(!look.wheels.is_neutral());
    first.library.set_look(look.clone()).unwrap();
    first.library.save_as_new_preset(name("Graded")).unwrap();
    // Правки поверх с другими колёсами тоже должны пережить перезапуск.
    let edited = LookSettings {
        wheels: ColorWheels {
            gain: ColorWheel::NEUTRAL,
            ..touched_wheels()
        },
        ..look.clone()
    };
    first.library.set_look(edited.clone()).unwrap();
    first.store.save(&first.library.snapshot()).unwrap();

    let second = open(dir.path());
    assert_eq!(second.outcome, LoadOutcome::Loaded { migrated_from: None });
    assert_eq!(second.library.snapshot(), first.library.snapshot());
    assert_eq!(second.library.snapshot().user_presets[0].recipe.settings, look);
    assert_eq!(second.library.active_look().recipe.settings, edited);
}

/// Файл версии 2 без `wheels`: serde молча не достраивает, файл уходит в карантин как неверная форма.
#[test]
fn v2_file_missing_wheels_is_quarantined_with_original_bytes() {
    let dir = TempDir::new("v2-nowheels");
    let content =
        String::from_utf8(NO_EDITS.to_vec())
            .unwrap()
            .replacen("\"schema_version\": 1", "\"schema_version\": 2", 1);
    assert!(!content.contains("wheels"));
    fs::write(presets_file(dir.path()), &content).unwrap();

    let opened = open(dir.path());
    let LoadOutcome::Quarantined { reason, moved_to } = opened.outcome else {
        panic!("ожидали карантин, получили {:?}", opened.outcome);
    };
    assert!(matches!(reason, CorruptionReason::InvalidShape(_)), "{reason:?}");
    assert_eq!(fs::read(&moved_to).unwrap(), content.as_bytes());
    assert_eq!(files_containing(dir.path(), QUARANTINE_FILE_MARKER), vec![moved_to]);
    assert!(!presets_file(dir.path()).exists());
    assert_eq!(
        opened.library.active_look().preset,
        PresetId::Builtin(BuiltinPreset::AsEye)
    );
}

#[test]
fn schema_five_is_quarantined_as_newer_and_bytes_are_kept() {
    let dir = TempDir::new("v4");
    let content = br#"{"schema_version":5,"anything":true}"#;
    fs::write(presets_file(dir.path()), content).unwrap();
    let opened = open(dir.path());
    let LoadOutcome::Quarantined { reason, moved_to } = opened.outcome else {
        panic!("ожидали карантин");
    };
    assert_eq!(reason, CorruptionReason::NewerSchema { found: 5, supported: 4 });
    assert_eq!(fs::read(moved_to).unwrap(), content);
}

#[test]
fn corrupt_v1_file_fails_migration_visibly_and_is_quarantined() {
    // `user_presets` не массив: шаг v1 → v2 падает различимой ошибкой, а не молча.
    let dir = TempDir::new("v1-broken");
    let content = br#"{"schema_version":1,"next_id":1,"active":{"builtin":"honest"},"edits":null,"user_presets":7}"#;
    fs::write(presets_file(dir.path()), content).unwrap();
    let opened = open(dir.path());
    let LoadOutcome::Quarantined { reason, moved_to } = opened.outcome else {
        panic!("ожидали карантин");
    };
    assert!(matches!(reason, CorruptionReason::MigrationFailed(_)), "{reason:?}");
    assert_eq!(fs::read(moved_to).unwrap(), content);
}

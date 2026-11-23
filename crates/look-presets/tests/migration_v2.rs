//! Миграция файла пресетов v2 → v3 (основа тона в пресете, 14b, решение 195) на настоящих файлах схемы 2.
//!
//! Фикстуры в `tests/fixtures/` записаны кодом до 14b и заморожены (см. README рядом).

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

mod common;

use std::fs;
use std::path::{Path, PathBuf};

use common::{TempDir, contrast, environment, knob, name, with_sky_band};
use look_contract::{
    ColorWheel, ColorWheels, EditState, LookRecipe, LookSettings, PresetId, ToneBaseModel, UserPresetId, WheelPush,
    WheelZone,
};
use look_presets::{
    LoadOutcome, OpenedPresets, PRE_MIGRATION_BACKUP_MARKER, PRODUCT_MIGRATIONS, PresetFileStore, PresetStoreConfig,
};
use serde_json::Value;

const WITH_EDITS: &[u8] = include_bytes!("fixtures/schema_v2_with_edits.json");
const NO_EDITS: &[u8] = include_bytes!("fixtures/schema_v2_no_edits.json");

fn open(dir: &Path) -> OpenedPresets {
    PresetFileStore::open(dir, &PresetStoreConfig::default(), &environment(), PRODUCT_MIGRATIONS)
        .expect("конфиг валиден")
}

fn presets_file(dir: &Path) -> PathBuf {
    dir.join("look-presets.json")
}

fn dir_with_file(label: &str, content: &[u8]) -> TempDir {
    let dir = TempDir::new(label);
    fs::write(presets_file(dir.path()), content).unwrap();
    dir
}

fn agx(settings: LookSettings) -> LookRecipe {
    LookRecipe {
        base: ToneBaseModel::Agx,
        settings,
    }
}

#[test]
fn v2_file_with_edits_loads_with_agx_base_everywhere_and_values_untouched() {
    let dir = dir_with_file("v2-edits", WITH_EDITS);
    let opened = open(dir.path());
    assert_eq!(opened.outcome, LoadOutcome::Loaded { migrated_from: Some(2) });

    let snapshot = opened.library.snapshot();
    assert_eq!(snapshot.active, PresetId::User(UserPresetId(1)));
    assert_eq!(snapshot.user_presets[0].name, name("Evening"));
    let evening = LookSettings {
        warmth: knob(-0.25),
        wheels: ColorWheels::NEUTRAL
            .with_wheel(
                WheelZone::Lift,
                ColorWheel {
                    push: WheelPush::new(0.3, -0.2).unwrap(),
                    ring: knob(0.0),
                },
            )
            .with_wheel(
                WheelZone::Gain,
                ColorWheel {
                    push: WheelPush::NEUTRAL,
                    ring: knob(0.15),
                },
            ),
        ..LookSettings {
            bands: with_sky_band(0.4).bands,
            ..contrast(0.3)
        }
    };
    assert_eq!(snapshot.user_presets[0].recipe, agx(evening));

    let edited = LookSettings {
        wheels: ColorWheels::NEUTRAL.with_wheel(
            WheelZone::Offset,
            ColorWheel {
                push: WheelPush::new(-0.1, 0.1).unwrap(),
                ring: knob(0.0),
            },
        ),
        ..contrast(-0.5)
    };
    assert_eq!(snapshot.edits, Some(agx(edited.clone())));
    let active = opened.library.active_look();
    assert_eq!(active.edits, EditState::EditedOnTop);
    assert_eq!(active.recipe, agx(edited));
    assert_eq!(active.preset_recipe, snapshot.user_presets[0].recipe);
}

#[test]
fn v2_file_without_edits_stays_clean() {
    let dir = dir_with_file("v2-noedits", NO_EDITS);
    let opened = open(dir.path());
    assert_eq!(opened.outcome, LoadOutcome::Loaded { migrated_from: Some(2) });
    let snapshot = opened.library.snapshot();
    assert_eq!(snapshot.edits, None);
    assert_eq!(snapshot.user_presets[0].recipe, agx(contrast(0.6)));
    let active = opened.library.active_look();
    assert_eq!(active.edits, EditState::Clean);
    assert_eq!(active.recipe, agx(contrast(0.6)));
}

#[test]
fn first_save_backs_up_v2_file_byte_exact_then_writes_v3_once() {
    for (label, fixture) in [("bk2-edits", WITH_EDITS), ("bk2-none", NO_EDITS)] {
        let dir = dir_with_file(label, fixture);
        let opened = open(dir.path());
        opened.store.save(&opened.library.snapshot()).unwrap();

        let backups: Vec<PathBuf> = fs::read_dir(dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.to_string_lossy().contains(PRE_MIGRATION_BACKUP_MARKER))
            .collect();
        assert_eq!(backups.len(), 1, "{label}: {backups:?}");
        assert!(backups[0].ends_with("look-presets.json.pre-v2"), "{label}");
        assert_eq!(fs::read(&backups[0]).unwrap(), fixture, "{label}: копия побайтно");

        let written: Value = serde_json::from_slice(&fs::read(presets_file(dir.path())).unwrap()).unwrap();
        assert_eq!(written["schema_version"], 3, "{label}");
        assert_eq!(written["user_presets"][0]["base"], "agx", "{label}");
        assert!(written["user_presets"][0]["settings"]["wheels"].is_object(), "{label}");

        let reopened = open(dir.path());
        assert_eq!(reopened.outcome, LoadOutcome::Loaded { migrated_from: None }, "{label}");
        assert_eq!(reopened.library.snapshot(), opened.library.snapshot(), "{label}");
    }
}

#[test]
fn recipe_with_non_default_base_roundtrips_through_the_file() {
    let dir = TempDir::new("v3-roundtrip");
    let mut first = open(dir.path());
    first.library.set_tone_base(ToneBaseModel::Filmic).unwrap();
    first.library.save_as_new_preset(name("Filmic mine")).unwrap();
    first.library.set_tone_base(ToneBaseModel::Aces).unwrap();
    first.store.save(&first.library.snapshot()).unwrap();

    let second = open(dir.path());
    assert_eq!(second.library.snapshot(), first.library.snapshot());
    assert_eq!(
        second.library.snapshot().user_presets[0].recipe.base,
        ToneBaseModel::Filmic
    );
    assert_eq!(second.library.active_look().recipe.base, ToneBaseModel::Aces);
    assert_eq!(second.library.active_look().preset_recipe.base, ToneBaseModel::Filmic);
}

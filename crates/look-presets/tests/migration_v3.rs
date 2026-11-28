//! Миграция файла пресетов v3 → v4 (полоски по каналам, 14t, решения 339–343) на настоящих файлах схемы 3.
//!
//! Фикстуры записаны прежним кодом (`tests/fixtures/README.md`) и заморожены. Схема 3 пережила 14l без подъёма
//! (решение 237: значения не пересчитываются), поэтому тест доказывает и это: все числа старого вида целы, а
//! новые полоски нейтральны.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

mod common;

use std::fs;
use std::path::{Path, PathBuf};

use common::{TempDir, contrast, environment, knob, name, with_sky_band};
use look_contract::{
    ChannelPrimaries, ColorWheel, ColorWheels, LookRecipe, LookSettings, PresetId, ToneBaseModel, UserPresetId,
    WheelPush, WheelZone,
};
use look_presets::{
    LoadOutcome, OpenedPresets, PRE_MIGRATION_BACKUP_MARKER, PRODUCT_MIGRATIONS, PresetFileStore, PresetStoreConfig,
};
use serde_json::Value;

const WITH_EDITS: &[u8] = include_bytes!("fixtures/schema_v3_with_edits.json");
const NO_EDITS: &[u8] = include_bytes!("fixtures/schema_v3_no_edits.json");
const PRESETS_FILE: &str = "look-presets.json";

fn open(dir: &Path) -> OpenedPresets {
    PresetFileStore::open(dir, &PresetStoreConfig::default(), &environment(), PRODUCT_MIGRATIONS)
        .expect("конфиг валиден")
}

fn presets_file(dir: &Path) -> PathBuf {
    dir.join(PRESETS_FILE)
}

fn dir_with_file(label: &str, content: &[u8]) -> TempDir {
    let dir = TempDir::new(label);
    fs::write(presets_file(dir.path()), content).unwrap();
    dir
}

/// Вид, сохранённый прежним кодом: ручки на самом краю (контраст и тени +1) — самое резкое, что там могло быть.
fn saved_evening() -> LookSettings {
    LookSettings {
        warmth: knob(-0.25),
        shadows: knob(1.0),
        highlights: knob(-0.6),
        bands: with_sky_band(0.4).bands,
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
        ..contrast(1.0)
    }
}

#[test]
fn the_old_schema_3_file_migrates_to_neutral_primaries_and_keeps_every_number() {
    let dir = dir_with_file("v3-edits", WITH_EDITS);
    let opened = open(dir.path());
    assert_eq!(opened.outcome, LoadOutcome::Loaded { migrated_from: Some(3) });

    let snapshot = opened.library.snapshot();
    assert_eq!(snapshot.active, PresetId::User(UserPresetId(1)));
    assert_eq!(snapshot.user_presets.len(), 1);
    assert_eq!(snapshot.user_presets[0].name, name("Evening"));
    let recipe = |settings| LookRecipe {
        base: ToneBaseModel::Aces,
        settings,
    };
    // `saved_evening()` собран через `..contrast(1.0)` и потому имеет нейтральные полоски: равенство доказывает,
    // что все прежние числа целы, а `primaries` появились нейтральными и в пресете, и в правках.
    assert_eq!(snapshot.user_presets[0].recipe, recipe(saved_evening()));
    assert_eq!(
        snapshot.edits,
        Some(recipe(LookSettings {
            contrast: knob(-0.5),
            ..saved_evening()
        }))
    );
    assert!(snapshot.user_presets[0].recipe.settings.primaries.is_neutral());
    assert_eq!(
        snapshot.edits.as_ref().unwrap().settings.primaries,
        ChannelPrimaries::NEUTRAL
    );
    // Чтение файл не трогает: копия и перезапись только при первом сохранении.
    assert_eq!(fs::read(presets_file(dir.path())).unwrap(), WITH_EDITS);
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
}

#[test]
fn first_save_backs_up_v3_file_byte_exact_then_writes_v4_once() {
    for (label, fixture) in [("bk3-edits", WITH_EDITS), ("bk3-none", NO_EDITS)] {
        let dir = dir_with_file(label, fixture);
        let opened = open(dir.path());
        assert_eq!(
            opened.outcome,
            LoadOutcome::Loaded { migrated_from: Some(3) },
            "{label}"
        );
        opened.store.save(&opened.library.snapshot()).unwrap();

        let backups: Vec<PathBuf> = fs::read_dir(dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.to_string_lossy().contains(PRE_MIGRATION_BACKUP_MARKER))
            .collect();
        assert_eq!(backups.len(), 1, "{label}: {backups:?}");
        assert!(backups[0].ends_with("look-presets.json.pre-v3"), "{label}");
        assert_eq!(fs::read(&backups[0]).unwrap(), fixture, "{label}: копия побайтно");

        let written: Value = serde_json::from_slice(&fs::read(presets_file(dir.path())).unwrap()).unwrap();
        assert_eq!(written["schema_version"], 4, "{label}");
        assert_eq!(
            written["user_presets"][0]["settings"]["primaries"]["gain"]["blue"], 0.0,
            "{label}"
        );
        assert_eq!(written["edits"].is_object(), fixture == WITH_EDITS, "{label}");
        if fixture == WITH_EDITS {
            assert_eq!(written["edits"]["settings"]["primaries"]["offset"]["red"], 0.0);
        }

        let reopened = open(dir.path());
        assert_eq!(reopened.outcome, LoadOutcome::Loaded { migrated_from: None }, "{label}");
        assert_eq!(reopened.library.snapshot(), opened.library.snapshot(), "{label}");
    }
}

#[test]
fn schema_3_file_without_edits_stays_clean() {
    let dir = dir_with_file("v3-no-edits", NO_EDITS);
    let opened = open(dir.path());
    assert_eq!(opened.outcome, LoadOutcome::Loaded { migrated_from: Some(3) });
    let snapshot = opened.library.snapshot();
    assert_eq!(snapshot.edits, None);
    assert_eq!(snapshot.user_presets[0].recipe.settings, contrast(0.6));
}

//! Файл пресетов схемы 3, записанный ДО новых краёв ручек (14l): читается как есть, значения не пересчитываются.
//!
//! В 14l изменилось, что значит край ручки ±1 (модель вида), а состав файла — нет, поэтому схема осталась 3
//! (решение 237: «старые пресеты не бережём», но файл не удаляется и запуск не ломает). Фикстуры — настоящие файлы
//! прежнего кода (`tests/fixtures/README.md`).

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

mod common;

use std::fs;
use std::path::Path;

use common::{TempDir, contrast, environment, knob, name, with_sky_band};
use look_contract::{
    ColorWheel, ColorWheels, LookRecipe, LookSettings, PresetId, ToneBaseModel, UserPresetId, WheelPush, WheelZone,
};
use look_presets::{LoadOutcome, OpenedPresets, PRODUCT_MIGRATIONS, PresetFileStore, PresetStoreConfig};

const WITH_EDITS: &[u8] = include_bytes!("fixtures/schema_v3_with_edits.json");
const NO_EDITS: &[u8] = include_bytes!("fixtures/schema_v3_no_edits.json");
const PRESETS_FILE: &str = "look-presets.json";

fn open(dir: &Path) -> OpenedPresets {
    PresetFileStore::open(dir, &PresetStoreConfig::default(), &environment(), PRODUCT_MIGRATIONS)
        .expect("конфиг валиден")
}

fn dir_with_file(label: &str, content: &[u8]) -> TempDir {
    let dir = TempDir::new(label);
    fs::write(dir.path().join(PRESETS_FILE), content).unwrap();
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
fn the_old_schema_3_file_loads_without_migration_and_keeps_every_number() {
    let dir = dir_with_file("v3-edits", WITH_EDITS);
    let opened = open(dir.path());
    assert_eq!(opened.outcome, LoadOutcome::Loaded { migrated_from: None });

    let snapshot = opened.library.snapshot();
    assert_eq!(snapshot.active, PresetId::User(UserPresetId(1)));
    assert_eq!(snapshot.user_presets.len(), 1);
    assert_eq!(snapshot.user_presets[0].name, name("Evening"));
    let recipe = |settings| LookRecipe {
        base: ToneBaseModel::Aces,
        settings,
    };
    assert_eq!(snapshot.user_presets[0].recipe, recipe(saved_evening()));
    assert_eq!(
        snapshot.edits,
        Some(recipe(LookSettings {
            contrast: knob(-0.5),
            ..saved_evening()
        }))
    );
    // Чтение файл не трогает: ни переименования в карантин, ни перезаписи, ни лишних файлов рядом.
    assert_eq!(fs::read(dir.path().join(PRESETS_FILE)).unwrap(), WITH_EDITS);
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
}

#[test]
fn a_saved_library_from_the_old_file_survives_a_restart_byte_for_byte() {
    let dir = dir_with_file("v3-no-edits", NO_EDITS);
    let opened = open(dir.path());
    assert_eq!(opened.outcome, LoadOutcome::Loaded { migrated_from: None });
    let snapshot = opened.library.snapshot();
    assert_eq!(snapshot.edits, None);
    assert_eq!(snapshot.user_presets[0].recipe.settings, contrast(0.6));

    opened.store.save(&snapshot).unwrap();
    assert_eq!(
        fs::read(dir.path().join(PRESETS_FILE)).unwrap(),
        NO_EDITS,
        "схема и числа прежние"
    );
    assert_eq!(open(dir.path()).library.snapshot(), snapshot);
}

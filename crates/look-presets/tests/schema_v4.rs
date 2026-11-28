//! Файл пресетов текущей схемы 4 (полоски по каналам, 14t): ненулевые полоски переживают перезапуск, а файл
//! без обязательного `primaries` уходит в карантин, а не получает нейтраль молча.
//!
//! Фикстуры — настоящие файлы, записанные кодом `look-presets` (`tests/fixtures/README.md`).

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

mod common;

use std::fs;
use std::path::Path;

use common::{TempDir, environment, knob, name};
use look_contract::{
    ChannelPrimaries, EditState, LookRecipe, PresetId, PrimaryChannel, ToneBaseModel, UserPresetId, WheelZone,
};
use look_presets::{
    CorruptionReason, LoadOutcome, OpenedPresets, PRODUCT_MIGRATIONS, PresetFileStore, PresetStoreConfig,
};
use serde_json::Value;

const WITH_EDITS: &[u8] = include_bytes!("fixtures/schema_v4_with_edits.json");
const NO_EDITS: &[u8] = include_bytes!("fixtures/schema_v4_no_edits.json");
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

fn bars(entries: &[(WheelZone, PrimaryChannel, f32)]) -> ChannelPrimaries {
    entries
        .iter()
        .fold(ChannelPrimaries::NEUTRAL, |primaries, &(zone, channel, value)| {
            primaries.with_value(zone, channel, knob(value))
        })
}

#[test]
fn schema_4_file_with_nonzero_bars_loads_without_migration_and_keeps_every_bar() {
    let dir = dir_with_file("v4-edits", WITH_EDITS);
    let opened = open(dir.path());
    assert_eq!(opened.outcome, LoadOutcome::Loaded { migrated_from: None });

    let snapshot = opened.library.snapshot();
    assert_eq!(snapshot.active, PresetId::User(UserPresetId(1)));
    assert_eq!(snapshot.user_presets[0].name, name("Evening"));
    let saved: &LookRecipe = &snapshot.user_presets[0].recipe;
    assert_eq!(saved.base, ToneBaseModel::Aces);
    assert_eq!(
        saved.settings.primaries,
        bars(&[
            (WheelZone::Lift, PrimaryChannel::Red, 0.3),
            (WheelZone::Gain, PrimaryChannel::Blue, -0.45),
        ])
    );
    let edited = snapshot.edits.as_ref().expect("правки есть");
    assert_eq!(
        edited.settings.primaries,
        bars(&[
            (WheelZone::Lift, PrimaryChannel::Red, -0.1),
            (WheelZone::Gain, PrimaryChannel::Blue, -0.45),
            (WheelZone::Offset, PrimaryChannel::Green, 0.2),
        ])
    );
    assert_eq!(opened.library.active_look().edits, EditState::EditedOnTop);
    // Чтение файл не трогает.
    assert_eq!(fs::read(dir.path().join(PRESETS_FILE)).unwrap(), WITH_EDITS);
}

#[test]
fn schema_4_files_survive_a_restart_byte_for_byte() {
    for (label, fixture) in [("v4-rt-edits", WITH_EDITS), ("v4-rt-none", NO_EDITS)] {
        let dir = dir_with_file(label, fixture);
        let opened = open(dir.path());
        let snapshot = opened.library.snapshot();
        opened.store.save(&snapshot).unwrap();
        assert_eq!(
            fs::read(dir.path().join(PRESETS_FILE)).unwrap(),
            fixture,
            "{label}: запись того, что прочли, даёт тот же файл"
        );
        assert_eq!(open(dir.path()).library.snapshot(), snapshot, "{label}");
    }
}

#[test]
fn schema_4_file_without_primaries_is_quarantined_not_silently_neutral() {
    for (label, fixture) in [("v4-nop-edits", WITH_EDITS), ("v4-nop-none", NO_EDITS)] {
        let mut document: Value = serde_json::from_slice(fixture).unwrap();
        // Вырезаем `primaries` из каждых settings: получается «файл схемы 4, который код не мог записать».
        for preset in document["user_presets"].as_array_mut().unwrap() {
            preset["settings"].as_object_mut().unwrap().remove("primaries");
        }
        let broken = serde_json::to_vec(&document).unwrap();
        let dir = dir_with_file(label, &broken);
        let opened = open(dir.path());
        let LoadOutcome::Quarantined { reason, moved_to } = opened.outcome else {
            panic!("{label}: ожидали карантин, получили {:?}", opened.outcome);
        };
        assert!(
            matches!(&reason, CorruptionReason::InvalidShape(text) if text.contains("primaries")),
            "{label}: {reason:?}"
        );
        assert_eq!(fs::read(moved_to).unwrap(), broken, "{label}: байты сохранены");
        assert!(!dir.path().join(PRESETS_FILE).exists(), "{label}");
    }
}

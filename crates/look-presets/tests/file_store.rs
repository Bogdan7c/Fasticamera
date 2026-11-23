//! Тесты файла пресетов: сохранение между запусками, карантин, атомарность, миграция.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

mod common;

use std::fs;
use std::path::{Path, PathBuf};

use common::{TempDir, contrast, environment, name, with_sky_band};
use look_contract::{BuiltinPreset, EditState, LookKnob, PresetError, PresetId, StorageFailure, UserPresetId};
use look_presets::{
    CorruptionReason, LoadOutcome, Migration, MigrationError, OpenedPresets, PRE_MIGRATION_BACKUP_MARKER,
    PRODUCT_MIGRATIONS, PresetFileStore, PresetStoreConfig, QUARANTINE_FILE_MARKER, SaveCommit, SnapshotError,
};
use serde_json::{Value, json};

fn open(dir: &Path) -> OpenedPresets {
    PresetFileStore::open(dir, &PresetStoreConfig::default(), &environment(), PRODUCT_MIGRATIONS)
        .expect("конфиг по умолчанию валиден")
}

fn presets_file(dir: &Path) -> PathBuf {
    dir.join("look-presets.json")
}

fn quarantined_files(dir: &Path) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.to_string_lossy().contains(QUARANTINE_FILE_MARKER))
        .collect();
    found.sort();
    found
}

fn assert_builtins_work(opened: &mut OpenedPresets) {
    let active = opened.library.active_look();
    assert_eq!(active.preset, PresetId::Builtin(BuiltinPreset::AsEye));
    assert_eq!(active.edits, EditState::Clean);
    assert_eq!(opened.library.presets().len(), 2);
    opened
        .library
        .select_preset(PresetId::Builtin(BuiltinPreset::Honest))
        .unwrap();
    assert!(opened.library.active_look().recipe.settings.is_neutral());
}

#[test]
fn missing_file_gives_clean_library() {
    let dir = TempDir::new("missing");
    let mut opened = open(dir.path());
    assert_eq!(opened.outcome, LoadOutcome::Missing);
    assert_builtins_work(&mut opened);
}

#[test]
fn saved_state_survives_restart_with_active_edits_and_values() {
    let dir = TempDir::new("restart");
    let mut first = open(dir.path());
    first.library.set_look(with_sky_band(0.4)).unwrap();
    first.library.save_as_new_preset(name("Sky pop")).unwrap();
    first.library.save_as_new_preset(name("Other")).unwrap();
    first.library.select_preset(PresetId::User(UserPresetId(1))).unwrap();
    first.library.set_look(contrast(-0.35)).unwrap(); // правки поверх «Sky pop»
    first.store.save(&first.library.snapshot()).unwrap();

    // «Перезапуск»: новая загрузка из того же файла.
    let second = open(dir.path());
    assert_eq!(second.outcome, LoadOutcome::Loaded { migrated_from: None });
    assert_eq!(second.library.active_look(), first.library.active_look());
    assert_eq!(second.library.snapshot(), first.library.snapshot());
    let active = second.library.active_look();
    assert_eq!(active.preset, PresetId::User(UserPresetId(1)));
    assert_eq!(active.edits, EditState::EditedOnTop);
    assert_eq!(active.recipe.settings.contrast.get(), -0.35);
    let listed: Vec<String> = second
        .library
        .presets()
        .iter()
        .map(|p| p.name.as_str().to_owned())
        .collect();
    assert_eq!(listed, ["Honest", "Natural", "Sky pop", "Other"]);
    // Счётчик id пережил перезапуск: следующий пресет не получает занятый номер.
    let mut second = second;
    second.library.save_as_new_preset(name("Third")).unwrap();
    assert_eq!(second.library.active_look().preset, PresetId::User(UserPresetId(3)));
}

#[test]
fn saved_file_has_documented_shape() {
    let dir = TempDir::new("shape");
    let mut opened = open(dir.path());
    opened.library.save_as_new_preset(name("Mine")).unwrap();
    opened.store.save(&opened.library.snapshot()).unwrap();
    let json: Value = serde_json::from_slice(&fs::read(presets_file(dir.path())).unwrap()).unwrap();
    assert_eq!(json["schema_version"], 3);
    assert_eq!(json["user_presets"][0]["base"], "agx");
    assert_eq!(json["next_id"], 2);
    assert_eq!(json["active"], json!({"user": 1}));
    assert_eq!(json["edits"], Value::Null);
    assert_eq!(json["user_presets"][0]["id"], 1);
    assert_eq!(json["user_presets"][0]["name"], "Mine");
}

#[test]
fn save_into_unreachable_directory_is_storage_error() {
    let dir = TempDir::new("nodir");
    let mut opened = open(dir.path());
    opened.library.save_as_new_preset(name("Mine")).unwrap();
    let broken = PresetFileStore::open(
        &dir.path().join("does").join("not").join("exist"),
        &PresetStoreConfig::default(),
        &environment(),
        PRODUCT_MIGRATIONS,
    )
    .unwrap();
    assert_eq!(broken.outcome, LoadOutcome::Missing);
    let result = broken.store.save(&opened.library.snapshot());
    assert!(
        matches!(result, Err(PresetError::Storage(StorageFailure::CreateTempFailed(_)))),
        "{result:?}"
    );
}

/// Записывает испорченное содержимое и проверяет весь сценарий карантина.
fn assert_quarantined(label: &str, content: &[u8], expect: impl Fn(&CorruptionReason) -> bool) {
    let dir = TempDir::new(label);
    fs::write(presets_file(dir.path()), content).unwrap();
    let mut opened = open(dir.path());

    let LoadOutcome::Quarantined { reason, moved_to } = opened.outcome.clone() else {
        panic!("{label}: ожидали карантин, получили {:?}", opened.outcome);
    };
    assert!(expect(&reason), "{label}: причина {reason:?}");
    assert_eq!(fs::read(&moved_to).unwrap(), content, "{label}: содержимое побайтно");
    assert!(!presets_file(dir.path()).exists(), "{label}: исходный файл перемещён");
    assert_eq!(quarantined_files(dir.path()), vec![moved_to]);
    assert_builtins_work(&mut opened);

    // Следующее сохранение создаёт новый файл, карантинный не трогает.
    opened.library.save_as_new_preset(name("After")).unwrap();
    opened.store.save(&opened.library.snapshot()).unwrap();
    assert_eq!(open(dir.path()).outcome, LoadOutcome::Loaded { migrated_from: None });
    assert_eq!(quarantined_files(dir.path()).len(), 1);
}

#[test]
fn not_json_goes_to_quarantine() {
    assert_quarantined("notjson", b"\x00\xffthis is { not json", |r| {
        matches!(r, CorruptionReason::NotJson(_))
    });
    assert_quarantined("empty", b"", |r| matches!(r, CorruptionReason::NotJson(_)));
}

#[test]
fn missing_or_non_numeric_schema_version_goes_to_quarantine() {
    assert_quarantined("nover", br#"{"next_id":1}"#, |r| {
        *r == CorruptionReason::MissingSchemaVersion
    });
    assert_quarantined("strver", br#"{"schema_version":"1"}"#, |r| {
        *r == CorruptionReason::MissingSchemaVersion
    });
}

#[test]
fn newer_schema_goes_to_quarantine_and_bytes_are_kept() {
    let content = br#"{"schema_version":4,"future":"field"}"#;
    assert_quarantined("newer", content, |r| {
        *r == CorruptionReason::NewerSchema { found: 4, supported: 3 }
    });
}

#[test]
fn wrong_shape_and_bad_values_go_to_quarantine() {
    let base = |extra: &str| {
        format!(
            r#"{{"schema_version":3,"next_id":1,"active":{{"builtin":"honest"}},"edits":null,"user_presets":[]{extra}}}"#
        )
    };
    assert_quarantined("extra", base(r#","surprise":1"#).as_bytes(), |r| {
        matches!(r, CorruptionReason::InvalidShape(_))
    });
    assert_quarantined("noactive", br#"{"schema_version":3,"next_id":1}"#, |r| {
        matches!(r, CorruptionReason::InvalidShape(_))
    });
    let out_of_range = br#"{"schema_version":3,"next_id":2,"active":{"user":1},"edits":null,
        "user_presets":[{"id":1,"name":"X","base":"agx","settings":{"contrast":9,"midtones":0,"shadows":0,
        "highlights":0,"saturation":0,"vibrance":0,"warmth":0,"tint":0,
        "wheels":{"lift":{"push":[0,0],"ring":0},"gamma":{"push":[0,0],"ring":0},"gain":{"push":[0,0],"ring":0},"offset":{"push":[0,0],"ring":0}}}}]}"#;
    assert_quarantined("range", out_of_range, |r| {
        matches!(r, CorruptionReason::InvalidShape(_))
    });
    let bad_name = br#"{"schema_version":3,"next_id":2,"active":{"user":1},"edits":null,
        "user_presets":[{"id":1,"name":"  ","base":"agx","settings":{"contrast":0,"midtones":0,"shadows":0,
        "highlights":0,"saturation":0,"vibrance":0,"warmth":0,"tint":0,
        "wheels":{"lift":{"push":[0,0],"ring":0},"gamma":{"push":[0,0],"ring":0},"gain":{"push":[0,0],"ring":0},"offset":{"push":[0,0],"ring":0}}}}]}"#;
    assert_quarantined("name", bad_name, |r| matches!(r, CorruptionReason::InvalidShape(_)));
}

#[test]
fn invariant_violation_in_valid_json_goes_to_quarantine() {
    let missing_active = br#"{"schema_version":3,"next_id":1,"active":{"user":5},"edits":null,"user_presets":[]}"#;
    assert_quarantined("ghost", missing_active, |r| {
        *r == CorruptionReason::InvalidContent(SnapshotError::ActivePresetMissing(PresetId::User(UserPresetId(5))))
    });
}

#[test]
fn repeated_corruption_in_same_millisecond_never_overwrites_earlier_quarantine() {
    let dir = TempDir::new("twice");
    let mut kept = Vec::new();
    for round in 0..3 {
        let content = format!("garbage {round}");
        fs::write(presets_file(dir.path()), &content).unwrap();
        let opened = open(dir.path());
        assert!(matches!(opened.outcome, LoadOutcome::Quarantined { .. }));
        kept.push(content);
    }
    let mut stored: Vec<String> = quarantined_files(dir.path())
        .iter()
        .map(|path| fs::read_to_string(path).unwrap())
        .collect();
    stored.sort();
    assert_eq!(stored, kept);
}

#[test]
fn stale_temp_file_from_interrupted_write_does_not_touch_main_file() {
    let dir = TempDir::new("stale");
    let mut opened = open(dir.path());
    opened.library.save_as_new_preset(name("Good")).unwrap();
    opened.store.save(&opened.library.snapshot()).unwrap();
    let good_bytes = fs::read(presets_file(dir.path())).unwrap();

    // Имитация обрыва: временный файл с обрезанным мусором лежит рядом.
    let temp = dir.path().join("look-presets.json.tmp");
    fs::write(&temp, b"{\"schema_version\":2,\"next_id\"").unwrap();

    let reopened = open(dir.path());
    assert_eq!(reopened.outcome, LoadOutcome::Loaded { migrated_from: None });
    assert_eq!(reopened.library.presets()[2].name.as_str(), "Good");
    assert_eq!(fs::read(presets_file(dir.path())).unwrap(), good_bytes);

    // Следующая запись перезаписывает мусорный временный файл и проходит.
    let mut reopened = reopened;
    reopened.library.save_as_new_preset(name("Next")).unwrap();
    reopened.store.save(&reopened.library.snapshot()).unwrap();
    assert!(!temp.exists(), "после rename временного файла нет");
    assert_eq!(open(dir.path()).library.presets().len(), 4);
}

#[test]
fn failure_before_rename_leaves_previous_file_byte_identical() {
    let dir = TempDir::new("beforerename");
    let mut opened = open(dir.path());
    opened.library.save_as_new_preset(name("Good")).unwrap();
    opened.store.save(&opened.library.snapshot()).unwrap();
    let good_bytes = fs::read(presets_file(dir.path())).unwrap();

    // На месте временного файла — каталог: запись падает ДО замены основного файла.
    fs::create_dir(dir.path().join("look-presets.json.tmp")).unwrap();
    opened.library.save_as_new_preset(name("Lost")).unwrap();
    let result = opened.store.save(&opened.library.snapshot());
    assert!(matches!(
        result,
        Err(PresetError::Storage(StorageFailure::CreateTempFailed(_)))
    ));
    assert_eq!(fs::read(presets_file(dir.path())).unwrap(), good_bytes);
}

#[test]
fn unreadable_file_refuses_saves_and_is_not_touched() {
    let dir = TempDir::new("unreadable");
    // Каталог на месте файла: прочитать нельзя, но это не «файла нет».
    fs::create_dir(presets_file(dir.path())).unwrap();
    fs::write(presets_file(dir.path()).join("inside"), b"keep").unwrap();
    let mut opened = open(dir.path());
    assert!(
        matches!(opened.outcome, LoadOutcome::Unreadable { .. }),
        "{:?}",
        opened.outcome
    );
    assert_builtins_work(&mut opened);
    let result = opened.store.save(&opened.library.snapshot());
    assert!(matches!(
        result,
        Err(PresetError::Storage(StorageFailure::SavesRefused(_)))
    ));
    assert_eq!(fs::read(presets_file(dir.path()).join("inside")).unwrap(), b"keep");
    assert!(quarantined_files(dir.path()).is_empty());
}

#[cfg(unix)]
#[test]
fn failed_quarantine_refuses_saves_and_keeps_corrupt_file() {
    use std::os::unix::fs::PermissionsExt;
    let dir = TempDir::new("quarfail");
    fs::write(presets_file(dir.path()), b"garbage").unwrap();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o555)).unwrap();
    // От root права не действуют: тогда сценарий воспроизвести нельзя.
    if fs::write(dir.path().join("probe"), b"x").is_ok() {
        eprintln!("пропуск: каталог остался записываемым (root)");
        return;
    }
    let opened = open(dir.path());
    assert!(
        matches!(opened.outcome, LoadOutcome::QuarantineFailed { .. }),
        "{:?}",
        opened.outcome
    );
    let result = opened.store.save(&opened.library.snapshot());
    assert!(matches!(
        result,
        Err(PresetError::Storage(StorageFailure::SavesRefused(_)))
    ));
    assert_eq!(fs::read(presets_file(dir.path())).unwrap(), b"garbage");
}

/// Тестовый шаг v0 → v1: в старой схеме поле называлось `preset`, а не `active`.
fn rename_preset_field(mut document: Value) -> Result<Value, MigrationError> {
    let object = document.as_object_mut().ok_or(MigrationError::RootNotObject)?;
    let old = object.remove("preset").ok_or(MigrationError::StepFailed {
        from_version: 0,
        reason: "нет поля preset".into(),
    })?;
    object.insert("active".into(), old);
    Ok(document)
}

#[test]
fn test_migration_table_upgrades_v0_file_end_to_end() {
    let dir = TempDir::new("migrate");
    fs::write(
        presets_file(dir.path()),
        br#"{"schema_version":0,"next_id":1,"preset":{"builtin":"honest"},"edits":null,"user_presets":[]}"#,
    )
    .unwrap();
    let table = v0_table();
    let opened = PresetFileStore::open(dir.path(), &PresetStoreConfig::default(), &environment(), &table).unwrap();
    assert_eq!(opened.outcome, LoadOutcome::Loaded { migrated_from: Some(0) });
    assert_eq!(
        opened.library.active_look().preset,
        PresetId::Builtin(BuiltinPreset::Honest)
    );
    // После сохранения файл уже в текущей схеме и читается без таблицы.
    opened.store.save(&opened.library.snapshot()).unwrap();
    assert_eq!(open(dir.path()).outcome, LoadOutcome::Loaded { migrated_from: None });
}

#[test]
fn old_schema_without_migration_step_is_quarantined_not_lost() {
    let dir = TempDir::new("nostep");
    let content = br#"{"schema_version":0,"preset":{"builtin":"honest"}}"#;
    fs::write(presets_file(dir.path()), content).unwrap();
    let opened = open(dir.path());
    let LoadOutcome::Quarantined { reason, moved_to } = opened.outcome else {
        panic!("ожидали карантин");
    };
    assert_eq!(
        reason,
        CorruptionReason::MigrationFailed(MigrationError::NoStep { from_version: 0 })
    );
    assert_eq!(fs::read(moved_to).unwrap(), content);
}

#[test]
fn invalid_config_is_rejected_before_touching_disk() {
    let dir = TempDir::new("badconfig");
    fs::write(presets_file(dir.path()), b"garbage").unwrap();
    let config = PresetStoreConfig {
        file_name: "a/b.json".into(),
        ..PresetStoreConfig::default()
    };
    assert!(PresetFileStore::open(dir.path(), &config, &environment(), PRODUCT_MIGRATIONS).is_err());
    assert_eq!(fs::read(presets_file(dir.path())).unwrap(), b"garbage");
}

#[test]
fn knob_values_roundtrip_exactly() {
    let dir = TempDir::new("exact");
    let mut opened = open(dir.path());
    opened.library.set_look(contrast(0.123_456_79)).unwrap();
    opened.store.save(&opened.library.snapshot()).unwrap();
    let again = open(dir.path());
    assert_eq!(
        again.library.active_look().recipe.settings.contrast,
        LookKnob::new(0.123_456_79).unwrap()
    );
}

/// Сбой сброса каталога после удавшегося rename — не потеря данных, а отдельный исход.
/// Реальный сбой: каталог без права чтения (`-wx`): временный файл и rename проходят, а `File::open(dir)` — нет.
#[cfg(unix)]
#[test]
fn directory_sync_failure_after_rename_is_a_distinct_success_not_an_error() {
    use std::os::unix::fs::PermissionsExt;
    let dir = TempDir::new("dirsync");
    let mut opened = open(dir.path());
    opened.library.save_as_new_preset(name("Kept")).unwrap();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o300)).unwrap();
    let probe = fs::read_dir(dir.path()).is_ok();
    let result = opened.store.save(&opened.library.snapshot());
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
    if probe {
        return; // права не действуют (root): сбой не воспроизвести, тест не применим
    }
    assert!(
        matches!(
            result,
            Ok(SaveCommit::ReplacedDirectoryUnconfirmed(
                StorageFailure::SyncDirectoryFailed(_)
            ))
        ),
        "{result:?}"
    );
    // Новый файл действительно на месте и читается.
    assert_eq!(open(dir.path()).library.presets()[2].name.as_str(), "Kept");
}

#[test]
fn normal_save_is_durable() {
    let dir = TempDir::new("durable");
    let opened = open(dir.path());
    assert_eq!(
        opened.store.save(&opened.library.snapshot()).unwrap(),
        SaveCommit::Durable
    );
}

const V0_FILE: &[u8] =
    br#"{"schema_version":0,"next_id":1,"preset":{"builtin":"honest"},"edits":null,"user_presets":[]}"#;

/// Тестовый шаг v0 → v1, дальше настоящие продуктовые v1 → v2 и v2 → v3: цепочка доводит файл до текущей схемы.
fn v0_table() -> [Migration; 3] {
    [
        Migration {
            from_version: 0,
            step: rename_preset_field,
        },
        PRODUCT_MIGRATIONS[0],
        PRODUCT_MIGRATIONS[1],
    ]
}

fn backup_files(dir: &Path) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.to_string_lossy().contains(PRE_MIGRATION_BACKUP_MARKER))
        .collect();
    found.sort();
    found
}

#[test]
fn first_save_after_migration_leaves_a_byte_exact_copy_of_the_old_file() {
    let dir = TempDir::new("backup");
    fs::write(presets_file(dir.path()), V0_FILE).unwrap();
    let opened = PresetFileStore::open(dir.path(), &PresetStoreConfig::default(), &environment(), &v0_table()).unwrap();
    assert!(backup_files(dir.path()).is_empty(), "до записи копировать рано");

    opened.store.save(&opened.library.snapshot()).unwrap();
    let backups = backup_files(dir.path());
    assert_eq!(backups.len(), 1, "{backups:?}");
    assert!(backups[0].ends_with("look-presets.json.pre-v0"));
    assert_eq!(
        fs::read(&backups[0]).unwrap(),
        V0_FILE,
        "копия побайтно равна старому файлу"
    );
    assert_ne!(
        fs::read(presets_file(dir.path())).unwrap(),
        V0_FILE,
        "основной файл уже новой схемы"
    );

    // Копия делается один раз: следующее сохранение новую не плодит.
    opened.store.save(&opened.library.snapshot()).unwrap();
    assert_eq!(backup_files(dir.path()), backups);
}

#[test]
fn existing_backup_is_never_overwritten_a_suffix_is_added() {
    let dir = TempDir::new("backup-exists");
    fs::write(presets_file(dir.path()), V0_FILE).unwrap();
    let earlier = dir.path().join("look-presets.json.pre-v0");
    fs::write(&earlier, b"earlier copy").unwrap();
    let opened = PresetFileStore::open(dir.path(), &PresetStoreConfig::default(), &environment(), &v0_table()).unwrap();
    opened.store.save(&opened.library.snapshot()).unwrap();

    assert_eq!(fs::read(&earlier).unwrap(), b"earlier copy");
    assert_eq!(
        fs::read(dir.path().join("look-presets.json.pre-v0-1")).unwrap(),
        V0_FILE
    );
}

/// Копию создать нельзя (каталог только для чтения): запись отказана явной ошибкой, старый файл цел.
#[cfg(unix)]
#[test]
fn failed_backup_refuses_the_save_and_keeps_the_old_file() {
    use std::os::unix::fs::PermissionsExt;
    let dir = TempDir::new("backup-fail");
    fs::write(presets_file(dir.path()), V0_FILE).unwrap();
    let opened = PresetFileStore::open(dir.path(), &PresetStoreConfig::default(), &environment(), &v0_table()).unwrap();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o500)).unwrap();
    let writable_anyway = fs::File::create(dir.path().join("probe")).is_ok();
    let result = opened.store.save(&opened.library.snapshot());
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
    if writable_anyway {
        return; // права не действуют (root): сбой не воспроизвести
    }
    assert!(
        matches!(result, Err(PresetError::Storage(StorageFailure::BackupFailed(_)))),
        "{result:?}"
    );
    assert_eq!(
        fs::read(presets_file(dir.path())).unwrap(),
        V0_FILE,
        "старый файл не тронут"
    );
    assert!(
        !dir.path().join("look-presets.json.tmp").exists(),
        "до записи дело не дошло"
    );
}

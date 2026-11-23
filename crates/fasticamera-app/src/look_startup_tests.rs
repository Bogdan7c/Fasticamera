//! Запуск хранилища пресетов на настоящих файлах во временной папке: первый запуск, сохранение и перезапуск,
//! испорченный файл (карантин, встроенные работают, факт виден в счётчиках исполнителя).

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use look_contract::{BuiltinPreset, LookIntent, LookSettings, PresetId};
use look_presets::{QUARANTINE_FILE_MARKER, SaveOutcome};

use super::*;
use crate::look_effects::LookEffects;
use crate::look_test_support::{contrast_look, default_environment};

static NEXT_DIRECTORY: AtomicU32 = AtomicU32::new(0);

/// Новая пустая папка приложения (каждый тест — своя).
fn app_directory() -> (AppDirectory, PathBuf) {
    let path = std::env::temp_dir().join(format!(
        "fasticamera-look-startup-{}-{}",
        std::process::id(),
        NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&path).expect("папка создана");
    let directory = AppDirectory::new(path.to_string_lossy().into_owned()).expect("папка годится");
    (directory, path)
}

/// Ждёт, пока поток записи отдаст итог записи (диск ждёт он, а не рендер, поэтому итог приходит позже).
fn wait_for_write(handle: &LookHandle) -> SaveOutcome {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(outcome) = handle.drain_save_outcomes().into_iter().next() {
            return outcome;
        }
        assert!(Instant::now() < deadline, "поток записи не ответил за 5 с");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn the_first_launch_has_no_file_and_starts_on_the_builtin_as_eye() {
    let (directory, path) = app_directory();

    let opened = open_look_service(&directory, &PresetStoreConfig::default(), &default_environment()).expect("открыто");

    assert_eq!(opened.startup, LookStartup::NoFileYet);
    let active = opened.handle.active_look();
    assert_eq!(active.preset, PresetId::Builtin(BuiltinPreset::AsEye));
    assert_ne!(
        active.recipe.settings,
        LookSettings::NEUTRAL,
        "«Как глаз» — не нейтраль"
    );
    std::fs::remove_dir_all(path).expect("уборка");
}

#[test]
fn a_saved_look_survives_the_restart_with_the_same_values() {
    let (directory, path) = app_directory();
    let config = PresetStoreConfig::default();
    {
        let opened = open_look_service(&directory, &config, &default_environment()).expect("открыто");
        opened
            .handle
            .apply(LookIntent::SetLook(contrast_look(0.5)))
            .expect("применено");
        assert_eq!(wait_for_write(&opened.handle), SaveOutcome::Saved);
    }

    let reopened = open_look_service(&directory, &config, &default_environment()).expect("открыто снова");

    assert_eq!(reopened.startup, LookStartup::Loaded);
    assert_eq!(reopened.handle.active_settings(), contrast_look(0.5));
    std::fs::remove_dir_all(path).expect("уборка");
}

#[test]
fn a_corrupt_file_is_quarantined_builtins_work_and_the_fact_is_in_the_counters() {
    let (directory, path) = app_directory();
    let config = PresetStoreConfig::default();
    let file = path.join(&config.file_name);
    std::fs::write(&file, b"{ this is not json").expect("испорченный файл");

    let opened = open_look_service(&directory, &config, &default_environment()).expect("открыто");

    assert_eq!(opened.startup, LookStartup::Quarantined);
    assert!(!file.exists(), "испорченный файл убран с пути");
    let quarantined: Vec<_> = std::fs::read_dir(&path)
        .expect("папка читается")
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.contains(QUARANTINE_FILE_MARKER))
        .collect();
    assert_eq!(quarantined.len(), 1, "ровно один карантинный файл: {quarantined:?}");
    assert_eq!(
        std::fs::read(path.join(&quarantined[0])).expect("карантинный файл читается"),
        b"{ this is not json",
        "содержимое пользователя не тронуто"
    );
    // Встроенные пресеты работают: активен «Как глаз» с конфигурными значениями.
    let active = opened.handle.active_look();
    assert_eq!(active.preset, PresetId::Builtin(BuiltinPreset::AsEye));
    // Факт запуска виден в счётчиках исполнителя эффектов.
    let effects = LookEffects::new(Some(opened.handle.clone()));
    assert_eq!(effects.counters().startup, LookStartup::Quarantined);
    // И сохранения дальше идут в новый файл.
    opened
        .handle
        .apply(LookIntent::SetLook(contrast_look(0.25)))
        .expect("применено");
    assert_eq!(wait_for_write(&opened.handle), SaveOutcome::Saved);
    assert!(file.exists(), "новый файл записан");
    std::fs::remove_dir_all(path).expect("уборка");
}

#[test]
fn a_config_with_a_broken_builtin_name_is_refused_with_a_distinct_error() {
    let (directory, path) = app_directory();
    let mut config = PresetStoreConfig::default();
    config.builtin_names.honest = String::new();

    let refused = open_look_service(&directory, &config, &default_environment());

    assert!(matches!(
        refused,
        Err(ConfigError::InvalidBuiltinName { field: "honest", .. })
    ));
    std::fs::remove_dir_all(path).expect("уборка");
}

/// Окружение каталога берёт основу и полосы из конфига рендера, и встроенные пресеты реально получают эту основу.
#[test]
fn catalog_environment_follows_render_config_and_reaches_builtin_presets() {
    use render_loop::config::{RenderLoopConfig, ToneBaseChoice};

    let mut render = RenderLoopConfig::default();
    render.viewfinder.tone.base = ToneBaseChoice::Filmic;
    let environment = catalog_environment(&render);
    assert_eq!(environment.default_base, look_contract::ToneBaseModel::Filmic);
    let expected_bands: Vec<_> = render
        .viewfinder
        .look_model
        .bands
        .iter()
        .map(|band| band.id.clone())
        .collect();
    assert!(!expected_bands.is_empty(), "в конфиге по умолчанию полосы есть");
    assert_eq!(environment.band_ids, expected_bands);

    let handle = LookHandle::in_memory(&PresetStoreConfig::default(), &environment).expect("конфиг верен");
    assert_eq!(handle.active_recipe().base, look_contract::ToneBaseModel::Filmic);
    // Сброс полосы из конфига допустим, полосы вне конфига — нет.
    let known = look_contract::LookControl::Band {
        band: expected_bands[0].clone(),
        axis: look_contract::BandAxis::Hue,
    };
    assert!(handle.apply(LookIntent::ResetControl(known)).is_ok());
    let ghost = look_contract::LookControl::Band {
        band: look_contract::HueBandId::new("ghost").unwrap(),
        axis: look_contract::BandAxis::Hue,
    };
    assert!(handle.apply(LookIntent::ResetControl(ghost)).is_err());
}

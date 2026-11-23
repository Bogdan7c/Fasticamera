//! Общие помощники интеграционных тестов хранилища пресетов.
#![allow(dead_code, clippy::expect_used, clippy::unwrap_used)]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use look_contract::{BandAdjustment, BandAdjustments, HueBandId, LookKnob, LookSettings, PresetName};
use look_presets::{PresetLibrary, PresetStoreConfig};

/// Уникальный каталог в `std::env::temp_dir()`; убирается при `Drop`.
pub struct TempDir {
    path: PathBuf,
}

impl TempDir {
    pub fn new(label: &str) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("look-presets-test-{label}-{}-{unique}", std::process::id()));
        std::fs::create_dir_all(&path).expect("создать временный каталог");
        Self { path }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        // Каталог мог стать нечитаемым в тесте прав: вернём права перед уборкой.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&self.path, std::fs::Permissions::from_mode(0o755));
        }
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

pub fn knob(value: f32) -> LookKnob {
    LookKnob::new(value).expect("значение в диапазоне")
}

pub fn name(raw: &str) -> PresetName {
    PresetName::new(raw).expect("имя валидно")
}

/// Настройки с одной тронутой ручкой контраста.
pub fn contrast(value: f32) -> LookSettings {
    LookSettings {
        contrast: knob(value),
        ..LookSettings::NEUTRAL
    }
}

/// Настройки с правкой полосы «sky».
pub fn with_sky_band(hue: f32) -> LookSettings {
    let band = BandAdjustment {
        hue: knob(hue),
        ..BandAdjustment::neutral(HueBandId::new("sky").expect("id валиден"))
    };
    LookSettings {
        bands: BandAdjustments::new(vec![band]).expect("уникальные id"),
        ..LookSettings::NEUTRAL
    }
}

pub fn new_library() -> PresetLibrary {
    let catalog = PresetStoreConfig::default()
        .builtin_catalog()
        .expect("конфиг по умолчанию валиден");
    PresetLibrary::new(catalog)
}

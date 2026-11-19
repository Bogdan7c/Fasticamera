//! Общее для тестов: код запекания из `build_support` (тот же, что в `build.rs`) и загрузка манифеста.
//!
//! Подключается только часть без `kurbo` (он есть лишь в build-зависимостях): генератор SDF, упаковка,
//! манифест, снятие шрифта и ячейка глифа.

#![allow(dead_code, clippy::expect_used)]

#[path = "../../build_support/error.rs"]
pub mod error;
#[path = "../../build_support/font_bake.rs"]
pub mod font_bake;
#[path = "../../build_support/glyph_cell.rs"]
pub mod glyph_cell;
#[path = "../../build_support/manifest.rs"]
pub mod manifest;
#[path = "../../build_support/pack.rs"]
pub mod pack;
#[path = "../../build_support/sdf.rs"]
pub mod sdf;

use manifest::BakeManifest;
use std::path::PathBuf;

/// Папка `assets/` crate.
pub fn assets_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets")
}

/// Манифест запекания, по которому собрана сама библиотека.
pub fn load_manifest() -> BakeManifest {
    let text = std::fs::read_to_string(assets_dir().join("bake.json")).expect("bake.json читается");
    BakeManifest::parse(&text).expect("bake.json валиден")
}

/// Байты шрифта из манифеста.
pub fn load_font_bytes(manifest: &BakeManifest) -> Vec<u8> {
    std::fs::read(assets_dir().join(&manifest.font)).expect("файл шрифта читается")
}

//! Запекание шрифта и иконок в SDF-атлас (решения 87, 88).
//!
//! Читает `assets/bake.json`, снимает метрики и контуры Inter (`harfrust` + `skrifa` живут только здесь),
//! строит SDF своим генератором и кладёт в `OUT_DIR` байты атласа и исходный код таблиц, которые
//! подключают `src/atlas.rs`, `src/text/tables.rs` и `src/icons.rs`. Любая ошибка входа — ошибка сборки
//! с понятным сообщением, молчаливых упрощений нет.

mod build_support;

use build_support::bake::bake_all;
use build_support::error::{BakeError, BakeResult};
use build_support::manifest::BakeManifest;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("ui-draw/build.rs: ошибка запекания: {error}");
            ExitCode::FAILURE
        }
    }
}

/// Читает окружение Cargo, запекает и записывает результат.
fn run() -> BakeResult<()> {
    let manifest_dir = required_env_path("CARGO_MANIFEST_DIR")?;
    let out_dir = required_env_path("OUT_DIR")?;
    let assets_dir = manifest_dir.join("assets");

    let manifest_path = assets_dir.join("bake.json");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=build_support");
    println!("cargo:rerun-if-changed={}", manifest_path.display());
    let manifest_text = std::fs::read_to_string(&manifest_path)
        .map_err(|error| BakeError::new(format!("{}: {error}", manifest_path.display())))?;
    let manifest = BakeManifest::parse(&manifest_text)?;

    // Каждый входной файл манифеста — вход сборки.
    println!("cargo:rerun-if-changed={}", assets_dir.join(&manifest.font).display());
    for icon in &manifest.icons {
        println!("cargo:rerun-if-changed={}", assets_dir.join(&icon.file).display());
    }

    let output = bake_all(&manifest, &assets_dir)?;
    write_output(&out_dir.join("sdf_atlas.bin"), &output.texels)?;
    write_output(&out_dir.join("atlas_meta.rs"), output.atlas_meta_source.as_bytes())?;
    write_output(&out_dir.join("font_tables.rs"), output.font_tables_source.as_bytes())?;
    write_output(&out_dir.join("icon_tables.rs"), output.icon_tables_source.as_bytes())?;
    Ok(())
}

/// Путь из обязательной переменной окружения Cargo.
fn required_env_path(name: &str) -> BakeResult<PathBuf> {
    std::env::var_os(name)
        .map(PathBuf::from)
        .ok_or_else(|| BakeError::new(format!("Cargo не задал переменную {name}")))
}

/// Записывает файл результата; ошибка записи — ошибка сборки.
fn write_output(path: &Path, bytes: &[u8]) -> BakeResult<()> {
    std::fs::write(path, bytes).map_err(|error| BakeError::new(format!("{}: {error}", path.display())))
}

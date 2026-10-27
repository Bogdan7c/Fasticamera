//! Сборка шейдеров: `shaders/*.{vert,frag,comp}` → SPIR-V в `OUT_DIR` + модуль с константами.
//!
//! Компилятор — `glslc` из NDK (решение 33), проверка результата — `spirv-val` из того же каталога.
//! Пути ищет чистая логика из `build_support/shader_tools.rs` (она же тестируется в `cargo test`).
//! Нет glslc → сборка падает с понятным текстом, молчаливого пропуска шейдеров нет.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[path = "build_support/shader_tools.rs"]
mod shader_tools;

use shader_tools::{
    NDK_VERSION_PROPERTY, ToolchainEnvironment, constant_name_for, host_tag_for, parse_gradle_property, resolve_glslc,
    sibling_tool,
};

/// Целевая среда SPIR-V: Vulkan 1.3 (решение сессии 03).
const TARGET_ENVIRONMENT: &str = "vulkan1.3";

fn main() {
    if let Err(message) = build_shaders() {
        eprintln!("\nошибка сборки шейдеров gpu-vulkan: {message}\n");
        std::process::exit(1);
    }
}

/// Читает окружение процесса в структуру для чистой логики поиска.
fn read_environment(gradle_properties: &Path) -> ToolchainEnvironment {
    let path_variable = |name: &str| env::var_os(name).filter(|value| !value.is_empty()).map(PathBuf::from);
    let mut sdk_roots: Vec<PathBuf> = ["ANDROID_HOME", "ANDROID_SDK_ROOT"]
        .iter()
        .filter_map(|name| path_variable(name))
        .collect();
    if let Some(home) = path_variable("HOME") {
        sdk_roots.push(home.join("Android").join("Sdk"));
    }
    let ndk_version = fs::read_to_string(gradle_properties)
        .ok()
        .and_then(|text| parse_gradle_property(&text, NDK_VERSION_PROPERTY));
    ToolchainEnvironment {
        glslc_override: path_variable("FASTICAMERA_GLSLC"),
        ndk_home: path_variable("ANDROID_NDK_HOME"),
        sdk_roots,
        ndk_version,
        host_tag: host_tag_for(env::consts::OS).to_owned(),
    }
}

/// Список исходников шейдеров (отсортирован — сборка детерминирована).
fn list_shader_sources(shaders_directory: &Path) -> Result<Vec<PathBuf>, String> {
    let entries = fs::read_dir(shaders_directory)
        .map_err(|error| format!("не удалось прочитать {}: {error}", shaders_directory.display()))?;
    let mut sources: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| constant_name_for(name).is_some())
        })
        .collect();
    sources.sort();
    Ok(sources)
}

/// Запускает внешний инструмент; при ошибке возвращает его вывод (stdout и stderr).
fn run_tool(mut command: Command, description: &str) -> Result<(), String> {
    let output = command
        .output()
        .map_err(|error| format!("{description}: не удалось запустить: {error}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "{description} завершился с ошибкой ({}):\n{}{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ))
    }
}

fn build_shaders() -> Result<(), String> {
    let manifest_directory = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").ok_or("нет CARGO_MANIFEST_DIR")?);
    let out_directory = PathBuf::from(env::var_os("OUT_DIR").ok_or("нет OUT_DIR")?);
    let shaders_directory = manifest_directory.join("shaders");
    let gradle_properties = manifest_directory.join("../../android/gradle.properties");

    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=build_support");
    println!("cargo:rerun-if-changed=shaders");
    println!("cargo:rerun-if-changed={}", gradle_properties.display());
    for variable in [
        "FASTICAMERA_GLSLC",
        "ANDROID_NDK_HOME",
        "ANDROID_HOME",
        "ANDROID_SDK_ROOT",
        "HOME",
    ] {
        println!("cargo:rerun-if-env-changed={variable}");
    }

    let environment = read_environment(&gradle_properties);
    let exists = |path: &Path| path.is_file();
    let glslc = resolve_glslc(&environment, &exists).map_err(|error| error.to_string())?;
    let spirv_val = sibling_tool(&glslc, "spirv-val", &exists).map_err(|error| error.to_string())?;

    let mut generated =
        String::from("// Сгенерировано build.rs: SPIR-V шейдеров, вшитый в бинарь. Не редактировать.\n\n");
    for source in list_shader_sources(&shaders_directory)? {
        let file_name = source
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| format!("странное имя файла шейдера: {}", source.display()))?;
        let constant = constant_name_for(file_name).ok_or_else(|| format!("не шейдер: {file_name}"))?;
        let spirv_path = out_directory.join(format!("{file_name}.spv"));

        let mut compile = Command::new(&glslc);
        compile
            .arg(format!("--target-env={TARGET_ENVIRONMENT}"))
            .arg("-Werror")
            .arg("-O")
            // Общие части шейдеров (`#include "*.glsl"`) лежат рядом с исходниками.
            .arg("-I")
            .arg(&shaders_directory)
            .arg("-o")
            .arg(&spirv_path)
            .arg(&source);
        run_tool(compile, &format!("glslc {file_name}"))?;

        let mut validate = Command::new(&spirv_val);
        // spirv-val принимает значение --target-env отдельным аргументом (форма с `=` не работает).
        validate.arg("--target-env").arg(TARGET_ENVIRONMENT).arg(&spirv_path);
        run_tool(validate, &format!("spirv-val {file_name}"))?;

        generated.push_str(&format!(
            "/// SPIR-V шейдера `{file_name}`.\npub const {constant}: &[u8] = include_bytes!({:?});\n",
            spirv_path.display().to_string()
        ));
    }
    fs::write(out_directory.join("shader_blobs.rs"), generated)
        .map_err(|error| format!("не удалось записать shader_blobs.rs: {error}"))
}

//! build.rs: компилирует GLSL compute-шейдеры в SPIR-V через `glslc` из Android NDK
//! и линкует `camera2ndk` (его нет среди link-атрибутов `ndk-sys`).
//!
//! NDK ищется так: `ANDROID_NDK_HOME` → `ANDROID_NDK_ROOT` → `$HOME/Android/Sdk/ndk/30.0.16248370`.
//! `.spv` пишутся только в `OUT_DIR` и в репозиторий не попадают.

use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Версия NDK по умолчанию (используется, если переменные окружения не заданы).
const DEFAULT_NDK_VERSION: &str = "30.0.16248370";
/// Шейдеры из `shaders/` (имя файла без расширения).
const SHADERS: &[&str] = &["sum_r16", "sum_words"];

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=ANDROID_NDK_HOME");
    println!("cargo:rerun-if-env-changed=ANDROID_NDK_ROOT");

    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("android") {
        println!("cargo:rustc-link-lib=camera2ndk");
    }

    let glslc = locate_glslc().unwrap_or_else(|message| fail(&message));
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap_or_else(|_| fail("OUT_DIR не задан")));
    for shader in SHADERS {
        compile_shader(&glslc, shader, &out_dir);
    }
}

/// Останавливает сборку с понятным сообщением.
fn fail(message: &str) -> ! {
    eprintln!("\nОШИБКА build.rs: {message}\n");
    std::process::exit(1);
}

/// Кандидаты на корень NDK в порядке приоритета.
fn ndk_root_candidates() -> Vec<PathBuf> {
    let mut candidates: Vec<PathBuf> = ["ANDROID_NDK_HOME", "ANDROID_NDK_ROOT"]
        .iter()
        .filter_map(env::var_os)
        .map(PathBuf::from)
        .collect();
    if let Some(home) = env::var_os("HOME") {
        candidates.push(PathBuf::from(home).join("Android/Sdk/ndk").join(DEFAULT_NDK_VERSION));
    }
    candidates
}

/// Ищет `glslc` внутри NDK (`shader-tools/<host>/glslc`).
fn locate_glslc() -> Result<PathBuf, String> {
    let candidates = ndk_root_candidates();
    for root in &candidates {
        let glslc = root.join("shader-tools/linux-x86_64/glslc");
        if glslc.is_file() {
            return Ok(glslc);
        }
    }
    Err(format!(
        "не найден glslc. Задайте ANDROID_NDK_HOME (или ANDROID_NDK_ROOT) на NDK с shader-tools. Проверены корни: {candidates:?}"
    ))
}

/// Компилирует `shaders/<name>.comp` → `$OUT_DIR/<name>.spv`.
fn compile_shader(glslc: &Path, name: &str, out_dir: &Path) {
    let source = format!("shaders/{name}.comp");
    let output = out_dir.join(format!("{name}.spv"));
    println!("cargo:rerun-if-changed={source}");

    let status = Command::new(glslc)
        .args(["--target-env=vulkan1.1", "-O", "-fshader-stage=compute"])
        .arg(&source)
        .arg("-o")
        .arg(&output)
        .status()
        .unwrap_or_else(|error| fail(&format!("не удалось запустить {glslc:?}: {error}")));
    if !status.success() {
        fail(&format!("glslc отверг {source} (код {status})"));
    }
}

//! Сборка шейдеров: `shaders/*.{vert,frag,comp}` → SPIR-V в `OUT_DIR` + модуль с константами.
//!
//! Компилятор — `glslc` из NDK (решение 33), проверка результата — `spirv-val` из того же каталога.
//! Вся логика — в `build_support/shader_tools.rs` (самодостаточный файл, его же подключает
//! `isp-vulkan/build.rs`; чистая часть тестируется в `cargo test`). Нет glslc → сборка падает с
//! понятным текстом, молчаливого пропуска шейдеров нет.

#[path = "build_support/shader_tools.rs"]
mod shader_tools;

fn main() {
    if let Err(message) = shader_tools::build_shader_blobs_from_cargo_env() {
        eprintln!("\nошибка сборки шейдеров gpu-vulkan: {message}\n");
        std::process::exit(1);
    }
}

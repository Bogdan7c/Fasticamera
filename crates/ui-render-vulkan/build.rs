//! Сборка шейдеров `ui-render-vulkan`: `shaders/*.{vert,frag,comp}` → SPIR-V в `OUT_DIR` + модуль констант.
//!
//! Логика общая с `gpu-vulkan` (glslc и spirv-val из закреплённого NDK, решение 33): файл
//! `gpu-vulkan/build_support/shader_tools.rs` самодостаточен и подключается сюда по пути.

#[path = "../gpu-vulkan/build_support/shader_tools.rs"]
mod shader_tools;

fn main() {
    if let Err(message) = shader_tools::build_shader_blobs_from_cargo_env() {
        eprintln!("\nошибка сборки шейдеров ui-render-vulkan: {message}\n");
        std::process::exit(1);
    }
}

//! Вшитые SPIR-V шейдеры `isp-vulkan` (генерирует `build.rs`: `name.comp` → `blobs::NAME_COMP`).

/// Константы со SPIR-V.
pub mod blobs {
    include!(concat!(env!("OUT_DIR"), "/shader_blobs.rs"));
}

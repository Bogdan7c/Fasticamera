//! Вшитые SPIR-V шейдеры и создание `VkShaderModule`.
//!
//! GLSL лежит в `crates/gpu-vulkan/shaders/`, компилируется в `build.rs` (glslc из NDK, затем
//! spirv-val) и вшивается в бинарь константами в [`blobs`]. Новый шейдер = новый файл
//! `*.vert|*.frag|*.comp` в каталоге шейдеров; константа появится сама
//! (`name.comp` → `blobs::NAME_COMP`).

use std::io::Cursor;

use ash::{util::read_spv, vk};

use crate::context::GpuContext;
use crate::error::{GpuError, run_step};
use crate::live_objects::OwnedShaderModule;

/// Константы со SPIR-V (генерирует `build.rs`).
pub mod blobs {
    include!(concat!(env!("OUT_DIR"), "/shader_blobs.rs"));
}

impl GpuContext {
    /// Создаёт шейдерный модуль из байтов SPIR-V (слова читаются `ash::util::read_spv`:
    /// проверяет длину, кратную 4, и магическое число, учитывает выравнивание).
    pub(crate) fn create_shader_module(&self, spirv: &[u8]) -> Result<OwnedShaderModule, GpuError> {
        let words = read_spv(&mut Cursor::new(spirv)).map_err(|error| GpuError::InvalidArgument {
            what: format!("некорректный SPIR-V: {error}"),
        })?;
        let create_info = vk::ShaderModuleCreateInfo::default().code(&words);
        // SAFETY: устройство валидно; words живут до конца вызова.
        let module = run_step(
            unsafe { self.device().create_shader_module(&create_info, None) },
            "vkCreateShaderModule",
        )?;
        Ok(OwnedShaderModule::from_raw(self, module))
    }
}

/// Логика поиска инструментов сборки (общая с `build.rs`) — подключена только для тестов.
#[cfg(test)]
#[allow(
    dead_code,
    reason = "часть функций нужна только build.rs, здесь файл подключён ради тестов"
)]
#[path = "../build_support/shader_tools.rs"]
mod shader_tools;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::test_support::gpu_context_or_skip;

    /// SPIR-V начинается с магического числа 0x07230203 (little-endian).
    fn starts_with_spirv_magic(blob: &[u8]) -> bool {
        blob.len().is_multiple_of(4) && blob.len() > 20 && blob[..4] == 0x0723_0203u32.to_le_bytes()
    }

    #[test]
    fn embedded_shaders_are_valid_spirv_blobs() {
        assert!(starts_with_spirv_magic(blobs::FULLSCREEN_TRIANGLE_VERT));
        assert!(starts_with_spirv_magic(blobs::TEST_PATTERN_FRAG));
    }

    #[test]
    fn broken_spirv_is_rejected_before_reaching_the_driver() {
        let Some(context) = gpu_context_or_skip() else { return };
        let error = context.create_shader_module(&[1, 2, 3]).unwrap_err();
        assert!(matches!(error, GpuError::InvalidArgument { .. }), "{error:?}");
        assert_eq!(context.live_objects().total(), 0);
    }

    #[test]
    fn embedded_shaders_create_modules_on_real_driver_and_are_counted() {
        let Some(context) = gpu_context_or_skip() else { return };
        let baseline = context.live_objects();
        {
            let vertex = context.create_shader_module(blobs::FULLSCREEN_TRIANGLE_VERT).unwrap();
            let fragment = context.create_shader_module(blobs::TEST_PATTERN_FRAG).unwrap();
            assert_ne!(vertex.handle(), vk::ShaderModule::null());
            assert_ne!(fragment.handle(), vk::ShaderModule::null());
            assert_eq!(
                context
                    .live_objects()
                    .count(crate::live_objects::LiveObjectKind::ShaderModule),
                2
            );
        }
        assert_eq!(context.live_objects(), baseline);
        assert_eq!(
            context.validation_report().errors,
            0,
            "{:?}",
            context.validation_report()
        );
    }
}

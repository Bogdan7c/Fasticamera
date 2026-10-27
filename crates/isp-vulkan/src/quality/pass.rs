//! Один compute-проход качественного конвейера: пайплайн + размер рабочей группы + push-константы.
//!
//! Все проходы делят раскладку push-констант (`PlaneGeometry` в `quality_common.glsl`, 32 байта) и
//! размер рабочей группы (specialization-константы 0 и 1) — одна точка правды для числа групп.

use ash::vk;
use gpu_vulkan::compute::{
    BindingKind, BindingWrite, ComputePipeline, ComputePipelineDesc, LocalSize, SpecializationConstant, WorkgroupCount,
};
use gpu_vulkan::context::GpuContext;
use gpu_vulkan::error::GpuError;

use super::frame_params::CfaCode;

/// Размер push-констант, байт: 4 × u32 + vec4.
const PUSH_CONSTANT_BYTES: u32 = 32;
/// `constant_id` размера рабочей группы по X и Y (`local_size_x_id`/`local_size_y_id`).
const LOCAL_SIZE_X_ID: u32 = 0;
const LOCAL_SIZE_Y_ID: u32 = 1;

/// Push-константы прохода (раскладка — `PlaneGeometry`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct PassPushConstants {
    /// Размер плоскостей, пиксели.
    pub extent: vk::Extent2D,
    /// Раскладка CFA.
    pub cfa: CfaCode,
    /// Слово режима (смысл у прохода; шумодав: 0 — сброс истории).
    pub pass_mode: u32,
    /// Числа параметров прохода (шумодав: strength, порог, шум чтения, дробовой шум).
    pub pass_parameters: [f32; 4],
}

impl PassPushConstants {
    /// Геометрия без режима и параметров (демозаик, уровни, выход).
    pub(crate) fn geometry(extent: vk::Extent2D, cfa: CfaCode) -> Self {
        Self {
            extent,
            cfa,
            pass_mode: 0,
            pass_parameters: [0.0; 4],
        }
    }

    fn to_bytes(self) -> Vec<u8> {
        let words = [
            self.extent.width,
            self.extent.height,
            self.cfa.shader_value(),
            self.pass_mode,
        ];
        words
            .iter()
            .flat_map(|word| word.to_ne_bytes())
            .chain(self.pass_parameters.iter().flat_map(|value| value.to_ne_bytes()))
            .collect()
    }
}

/// Проход: пайплайн с кольцом наборов дескрипторов и размер его рабочей группы.
pub(crate) struct QualityPass {
    pipeline: ComputePipeline,
    local: LocalSize,
}

impl QualityPass {
    /// Создаёт пайплайн прохода.
    pub(crate) fn new(
        context: &GpuContext,
        label: &'static str,
        spirv: &[u8],
        bindings: &[BindingKind],
        local: LocalSize,
        frame_slots: usize,
    ) -> Result<Self, GpuError> {
        let pipeline = ComputePipeline::new(
            context,
            &ComputePipelineDesc {
                label,
                spirv,
                bindings,
                push_constant_bytes: PUSH_CONSTANT_BYTES,
                specialization: &[
                    SpecializationConstant {
                        constant_id: LOCAL_SIZE_X_ID,
                        value: local.x.get(),
                    },
                    SpecializationConstant {
                        constant_id: LOCAL_SIZE_Y_ID,
                        value: local.y.get(),
                    },
                ],
                frame_slots,
            },
        )?;
        Ok(Self { pipeline, local })
    }

    /// Переписывает привязки набора слота (контракт `ComputePipeline::bind`: слот свободен от GPU).
    pub(crate) fn bind(&mut self, frame_slot: usize, writes: &[BindingWrite]) -> Result<(), GpuError> {
        self.pipeline.bind(frame_slot, writes)
    }

    /// Подтверждает, что fence слота сигнален: набор слота снова можно переписывать (`ComputePipeline::bind`).
    pub(crate) fn release_slot_after_fence(&self, frame_slot: usize) -> Result<(), GpuError> {
        self.pipeline.release_slot_after_fence(frame_slot)
    }

    /// Пишет одни и те же привязки во все слоты (неизменные ресурсы: плоскости).
    pub(crate) fn bind_all_slots(&mut self, writes: &[BindingWrite]) -> Result<(), GpuError> {
        for frame_slot in 0..self.pipeline.frame_slots() {
            self.pipeline.bind(frame_slot, writes)?;
        }
        Ok(())
    }

    /// Проверяет dispatch по плоскости `push.extent` без записи команд (`ComputePipeline::check_dispatch`).
    pub(crate) fn check(&self, frame_slot: usize, push: &PassPushConstants) -> Result<(), GpuError> {
        let groups = WorkgroupCount::covering(push.extent.width, push.extent.height, self.local);
        self.pipeline
            .check_dispatch(frame_slot, PUSH_CONSTANT_BYTES as usize, groups)
    }

    /// Как [`Self::check`], но без проверки набора: привязки прохода пишутся позже (цель приёмника).
    pub(crate) fn check_shape(&self, push: &PassPushConstants) -> Result<(), GpuError> {
        let groups = WorkgroupCount::covering(push.extent.width, push.extent.height, self.local);
        self.pipeline.check_dispatch_shape(PUSH_CONSTANT_BYTES as usize, groups)
    }

    /// Записывает dispatch, покрывающий плоскость `push.extent`.
    pub(crate) fn record(
        &self,
        command_buffer: vk::CommandBuffer,
        frame_slot: usize,
        push: &PassPushConstants,
    ) -> Result<(), GpuError> {
        let groups = WorkgroupCount::covering(push.extent.width, push.extent.height, self.local);
        self.pipeline
            .record_dispatch(command_buffer, frame_slot, &push.to_bytes(), groups)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use isp_model::cfa::BayerPattern;

    #[test]
    fn push_constants_follow_plane_geometry_layout() {
        let push = PassPushConstants {
            extent: vk::Extent2D {
                width: 4096,
                height: 2304,
            },
            cfa: CfaCode::from_pattern(BayerPattern::BGGR),
            pass_mode: 1,
            pass_parameters: [0.8, 3.0, 0.002, 0.0004],
        };
        let bytes = push.to_bytes();
        assert_eq!(bytes.len() as u32, PUSH_CONSTANT_BYTES);
        let word = |index: usize| {
            u32::from_ne_bytes([
                bytes[4 * index],
                bytes[4 * index + 1],
                bytes[4 * index + 2],
                bytes[4 * index + 3],
            ])
        };
        assert_eq!([word(0), word(1), word(2), word(3)], [4096, 2304, 3, 1]);
        assert_eq!(f32::from_bits(word(4)), 0.8);
        assert_eq!(f32::from_bits(word(7)), 0.0004);
    }
}

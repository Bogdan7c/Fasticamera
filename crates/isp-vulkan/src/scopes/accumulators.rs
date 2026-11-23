//! Реестр шейдеров накопления: пайплайны ВСЕХ приборов создаются один раз при создании
//! [`crate::scopes::ColorScopes`]; смена прибора, осей и размера позже пайплайнов не создаёт.
//!
//! Все шейдеры накопления имеют ОДИН интерфейс (`scope_accumulate_common.glsl`): привязки 0…2 и push-константы
//! `SamplingPush`; различается только SPIR-V. Поэтому новый прибор регистрируется парой «идентификатор + SPIR-V»
//! ([`AccumulatorShader`]) и не требует кода в проходах.

use gpu_vulkan::compute::{BindingKind, ComputePipeline, ComputePipelineDesc, LocalSize, SpecializationConstant};
use gpu_vulkan::context::GpuContext;

use super::error::ScopeError;
use super::rule::AccumulatorId;
use crate::shaders::blobs;

/// Привязки шейдера накопления: показанная картинка, параметры, счётчики.
pub(super) const ACCUMULATE_BINDINGS: [BindingKind; 3] = [
    BindingKind::SampledImage,
    BindingKind::ReadOnlyStorageBuffer,
    BindingKind::ReadWriteStorageBuffer,
];

/// Размер push-констант прохода накопления: 7 слов `SamplingPush`.
pub(super) const SAMPLING_PUSH_BYTES: u32 = 28;

/// Размер рабочей группы всех проходов приборов (128 нитей — гарантия спецификации Vulkan для любого устройства).
pub(super) const LOCAL_WIDTH: u32 = 16;
/// Высота рабочей группы.
pub(super) const LOCAL_HEIGHT: u32 = 8;

/// `constant_id` ширины группы в шейдерах (`layout(local_size_x_id = 0)`).
pub(super) const LOCAL_SIZE_X_ID: u32 = 0;
/// `constant_id` высоты группы в шейдерах (`layout(local_size_y_id = 1)`).
pub(super) const LOCAL_SIZE_Y_ID: u32 = 1;

/// Specialization-константы размера группы: шейдеры получают размер от Rust, а не из своих литералов.
pub(super) fn local_size_specialization(local: LocalSize) -> [SpecializationConstant; 2] {
    [
        SpecializationConstant {
            constant_id: LOCAL_SIZE_X_ID,
            value: local.x.get(),
        },
        SpecializationConstant {
            constant_id: LOCAL_SIZE_Y_ID,
            value: local.y.get(),
        },
    ]
}

/// Шейдер накопления для регистрации.
#[derive(Debug, Clone, Copy)]
pub struct AccumulatorShader {
    /// Идентификатор, по которому прибор выбирает шейдер.
    pub id: AccumulatorId,
    /// SPIR-V (точка входа `main`, интерфейс — `scope_accumulate_common.glsl`).
    pub spirv: &'static [u8],
}

/// Шейдеры приборов `scope-model`: целочисленная проекция и OKLab.
pub fn standard_accumulators() -> [AccumulatorShader; 2] {
    [
        AccumulatorShader {
            id: AccumulatorId::INTEGER_PROJECTION,
            spirv: blobs::SCOPE_ACCUMULATE_INTEGER_COMP,
        },
        AccumulatorShader {
            id: AccumulatorId::OKLAB_VECTORSCOPE,
            spirv: blobs::SCOPE_ACCUMULATE_OKLAB_COMP,
        },
    ]
}

/// Пайплайны накопления по идентификаторам.
pub(super) struct AccumulatorSet {
    entries: Vec<(AccumulatorId, ComputePipeline)>,
}

impl AccumulatorSet {
    /// Создаёт пайплайн на каждый шейдер. Повтор идентификатора — ошибка.
    pub(super) fn new(
        context: &GpuContext,
        shaders: &[AccumulatorShader],
        local: LocalSize,
        frame_slots: usize,
    ) -> Result<Self, ScopeError> {
        let mut entries: Vec<(AccumulatorId, ComputePipeline)> = Vec::with_capacity(shaders.len());
        for shader in shaders {
            if entries.iter().any(|(id, _)| *id == shader.id) {
                return Err(ScopeError::DuplicateAccumulator { id: shader.id });
            }
            let pipeline = ComputePipeline::new(
                context,
                &ComputePipelineDesc {
                    label: shader.id.name(),
                    spirv: shader.spirv,
                    bindings: &ACCUMULATE_BINDINGS,
                    push_constant_bytes: SAMPLING_PUSH_BYTES,
                    specialization: &local_size_specialization(local),
                    frame_slots,
                },
            )?;
            entries.push((shader.id, pipeline));
        }
        Ok(Self { entries })
    }

    /// Сколько пайплайнов накопления создано.
    pub(super) fn len(&self) -> usize {
        self.entries.len()
    }

    pub(super) fn get(&self, id: AccumulatorId) -> Result<&ComputePipeline, ScopeError> {
        self.entries
            .iter()
            .find(|(candidate, _)| *candidate == id)
            .map(|(_, pipeline)| pipeline)
            .ok_or(ScopeError::UnknownAccumulator { id })
    }

    pub(super) fn get_mut(&mut self, id: AccumulatorId) -> Result<&mut ComputePipeline, ScopeError> {
        self.entries
            .iter_mut()
            .find(|(candidate, _)| *candidate == id)
            .map(|(_, pipeline)| pipeline)
            .ok_or(ScopeError::UnknownAccumulator { id })
    }

    /// Fence слота сигнален: наборы слота всех пайплайнов снова можно переписывать.
    pub(super) fn release_slot_after_fence(&self, frame_slot: usize) -> Result<(), ScopeError> {
        for (_, pipeline) in &self.entries {
            pipeline.release_slot_after_fence(frame_slot)?;
        }
        Ok(())
    }
}

/// Размер рабочей группы проходов приборов.
pub(super) fn local_size() -> Result<LocalSize, ScopeError> {
    Ok(LocalSize::new(LOCAL_WIDTH, LOCAL_HEIGHT)?)
}

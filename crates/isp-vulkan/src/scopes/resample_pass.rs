//! Первый проход показа приборов — перенос сетки счётчиков в столбцы картинки (`scope_resample.comp`, решение 322).
//!
//! Владелец — [`super::ColorScopes`]: он решает, когда проход пишется, и держит порядок кадра «очистка → накопление →
//! барьер → ЭТОТ проход → барьер → показ». Модуль владеет только пайплайном прохода и знает его привязки:
//! * 0 — промежуточный слой картинки (`PictureTarget::resampled_columns`, запись);
//! * 1 — блок параметров показа (тот же буфер, что читает `scope_render.comp`);
//! * 2 — счётчики (только чтение).
//!
//! Промежуточный слой перезаписывается каждым кадром целиком (`ширина картинки × строк сетки × слотов`), поэтому
//! очистка ему не нужна. Запись поверх чтения прошлого кадра (`scope_render.comp` читал слой) закрывает барьер
//! «compute → compute» после накопления: его первая область — все команды раньше в порядке отправки.

use ash::vk;
use gpu_vulkan::compute::{
    BindingKind, BindingWrite, ComputePipeline, ComputePipelineDesc, DescriptorResource, LocalSize, WorkgroupCount,
};
use gpu_vulkan::context::GpuContext;

use super::accumulators::local_size_specialization;
use super::error::ScopeError;
use crate::shaders::blobs;

/// Привязки прохода переноса: промежуточный слой (запись), параметры показа, счётчики.
const RESAMPLE_BINDINGS: [BindingKind; 3] = [
    BindingKind::ReadWriteStorageBuffer,
    BindingKind::ReadOnlyStorageBuffer,
    BindingKind::ReadOnlyStorageBuffer,
];

/// Ресурсы одного кадра прохода переноса.
pub(super) struct ResampleBindings {
    /// Промежуточный слой картинки (запись).
    pub resampled_columns: DescriptorResource,
    /// Блок параметров показа слота.
    pub render_parameters: DescriptorResource,
    /// Счётчики (только чтение).
    pub counters: DescriptorResource,
}

/// Пайплайн прохода переноса по горизонтали.
pub(super) struct ResamplePass {
    pipeline: ComputePipeline,
    local_size: LocalSize,
}

impl ResamplePass {
    /// Строит пайплайн (один раз, вместе с остальными пайплайнами приборов).
    pub(super) fn new(context: &GpuContext, local_size: LocalSize, frame_slots: usize) -> Result<Self, ScopeError> {
        let pipeline = ComputePipeline::new(
            context,
            &ComputePipelineDesc {
                label: "scope-resample",
                spirv: blobs::SCOPE_RESAMPLE_COMP,
                bindings: &RESAMPLE_BINDINGS,
                push_constant_bytes: 0,
                specialization: &local_size_specialization(local_size),
                frame_slots,
            },
        )?;
        Ok(Self { pipeline, local_size })
    }

    /// Группы dispatch: нить на пару (столбец картинки, строка сетки).
    pub(super) fn groups_for(&self, picture_width: u32, grid_rows: u32) -> WorkgroupCount {
        WorkgroupCount::covering(picture_width, grid_rows, self.local_size)
    }

    /// Проверка формы dispatch против лимитов устройства — до любых записей в наборы.
    pub(super) fn check_dispatch_shape(&self, groups: WorkgroupCount) -> Result<(), ScopeError> {
        Ok(self.pipeline.check_dispatch_shape(0, groups)?)
    }

    /// Привязывает ресурсы кадра к набору слота.
    pub(super) fn bind(&mut self, slot: usize, bindings: ResampleBindings) -> Result<(), ScopeError> {
        Ok(self.pipeline.bind(
            slot,
            &[
                BindingWrite {
                    binding: 0,
                    resource: bindings.resampled_columns,
                },
                BindingWrite {
                    binding: 1,
                    resource: bindings.render_parameters,
                },
                BindingWrite {
                    binding: 2,
                    resource: bindings.counters,
                },
            ],
        )?)
    }

    /// Пишет dispatch прохода (набор слота уже привязан).
    pub(super) fn record_dispatch(
        &self,
        command_buffer: vk::CommandBuffer,
        slot: usize,
        groups: WorkgroupCount,
    ) -> Result<(), ScopeError> {
        Ok(self.pipeline.record_dispatch(command_buffer, slot, &[], groups)?)
    }

    /// Fence слота сигнален: набор дескрипторов слота свободен.
    pub(super) fn release_slot_after_fence(&mut self, slot: usize) -> Result<(), ScopeError> {
        Ok(self.pipeline.release_slot_after_fence(slot)?)
    }
}

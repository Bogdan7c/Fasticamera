//! Проходы на краях конвейера: «уровни и баланс» (кадр камеры → мозаика) и «выход» (линейный RGB →
//! цвет → охват → тон → OETF → цель приёмника кадра). Их привязки меняются каждый кадр (источник,
//! uniform слота, цель), поэтому пишутся при записи кадра, а не при создании.

use ash::vk;
use gpu_vulkan::compute::{BindingKind, BindingWrite, DescriptorResource, LocalSize};
use gpu_vulkan::context::GpuContext;
use gpu_vulkan::device_buffer::DeviceBuffer;
use gpu_vulkan::error::GpuError;
use gpu_vulkan::frame_sink::FrameSinkTarget;
use gpu_vulkan::raw_mosaic_preview::MosaicSource;
use gpu_vulkan::storage_image::StorageImageFormat;

use super::frame_params::CfaCode;
use super::pass::{PassPushConstants, QualityPass};
use super::planes::LinearRgbPlanes;
use crate::shaders::blobs;

/// Проход 0 для обоих видов кадра (RAW16 — картинка, RAW10 — упакованный буфер).
pub(crate) struct LevelsStage {
    raw16: QualityPass,
    raw10: QualityPass,
}

impl LevelsStage {
    pub(crate) fn new(context: &GpuContext, local: LocalSize, frame_slots: usize) -> Result<Self, GpuError> {
        let bindings = |source: BindingKind| {
            [
                source,
                BindingKind::UniformBuffer,
                BindingKind::ReadWriteStorageBuffer,
                BindingKind::ReadOnlyStorageBuffer,
            ]
        };
        Ok(Self {
            raw16: QualityPass::new(
                context,
                "isp-quality-levels-raw16",
                blobs::QUALITY_LEVELS_RAW16_COMP,
                &bindings(BindingKind::SampledImage),
                local,
                frame_slots,
            )?,
            raw10: QualityPass::new(
                context,
                "isp-quality-levels-raw10",
                blobs::QUALITY_LEVELS_RAW10_COMP,
                &bindings(BindingKind::ReadOnlyStorageBuffer),
                local,
                frame_slots,
            )?,
        })
    }

    /// Fence слота сигнален: наборы слота обоих вариантов снова можно переписывать.
    pub(crate) fn release_slot_after_fence(&self, frame_slot: usize) -> Result<(), GpuError> {
        self.raw16.release_slot_after_fence(frame_slot)?;
        self.raw10.release_slot_after_fence(frame_slot)
    }

    /// Привязывает кадр, uniform слота, карту виньетирования слота и мозаику и проверяет dispatch — БЕЗ записи команд (звать до
    /// первой команды кадра). Возвращает подготовленный проход для [`Self::record`].
    pub(crate) fn bind_and_check(
        &mut self,
        frame_slot: usize,
        source: &MosaicSource,
        frame_parameters: DescriptorResource,
        lens_shading_map: DescriptorResource,
        mosaic: &DeviceBuffer,
        push: &PassPushConstants,
    ) -> Result<(), GpuError> {
        let pass = self.pass_for(source);
        pass.bind(
            frame_slot,
            &[
                BindingWrite {
                    binding: 0,
                    resource: source.compute_binding(),
                },
                BindingWrite {
                    binding: 1,
                    resource: frame_parameters,
                },
                BindingWrite {
                    binding: 2,
                    resource: mosaic.read_write_binding(),
                },
                BindingWrite {
                    binding: 3,
                    resource: lens_shading_map,
                },
            ],
        )?;
        pass.check(frame_slot, push)
    }

    /// Пишет dispatch прохода, подготовленного [`Self::bind_and_check`] для того же кадра.
    pub(crate) fn record(
        &mut self,
        command_buffer: vk::CommandBuffer,
        frame_slot: usize,
        source: &MosaicSource,
        push: &PassPushConstants,
    ) -> Result<(), GpuError> {
        self.pass_for(source).record(command_buffer, frame_slot, push)
    }

    fn pass_for(&mut self, source: &MosaicSource) -> &mut QualityPass {
        match source {
            MosaicSource::SampledR16 { .. } => &mut self.raw16,
            MosaicSource::PackedRaw10 { .. } => &mut self.raw10,
        }
    }
}

/// Выходной проход: вариант на каждый формат цели (квалификатор формата в шейдере).
pub(crate) struct OutputStage {
    ten_bit: QualityPass,
    half_float: QualityPass,
}

/// Привязки выходного прохода: R, G, B (чтение), uniform кадра, цель.
const OUTPUT_BINDINGS: [BindingKind; 5] = [
    BindingKind::ReadOnlyStorageBuffer,
    BindingKind::ReadOnlyStorageBuffer,
    BindingKind::ReadOnlyStorageBuffer,
    BindingKind::UniformBuffer,
    BindingKind::StorageImageWrite,
];

impl OutputStage {
    pub(crate) fn new(context: &GpuContext, local: LocalSize, frame_slots: usize) -> Result<Self, GpuError> {
        Ok(Self {
            ten_bit: QualityPass::new(
                context,
                "isp-quality-output-rgb10a2",
                blobs::QUALITY_OUTPUT_RGB10A2_COMP,
                &OUTPUT_BINDINGS,
                local,
                frame_slots,
            )?,
            half_float: QualityPass::new(
                context,
                "isp-quality-output-rgba16f",
                blobs::QUALITY_OUTPUT_RGBA16F_COMP,
                &OUTPUT_BINDINGS,
                local,
                frame_slots,
            )?,
        })
    }

    /// Fence слота сигнален: наборы слота обоих вариантов снова можно переписывать.
    pub(crate) fn release_slot_after_fence(&self, frame_slot: usize) -> Result<(), GpuError> {
        self.ten_bit.release_slot_after_fence(frame_slot)?;
        self.half_float.release_slot_after_fence(frame_slot)
    }

    /// Проверяет dispatch выхода для формата цели `format` без записи команд. Цель ещё не выдана, поэтому
    /// набор не проверяется (он полностью переписывается в [`Self::record`]).
    pub(crate) fn check(&self, format: StorageImageFormat, extent: vk::Extent2D) -> Result<(), GpuError> {
        let pass = match format {
            StorageImageFormat::A2B10G10R10Unorm => &self.ten_bit,
            StorageImageFormat::Rgba16Float => &self.half_float,
        };
        pass.check_shape(&output_push(extent))
    }

    /// Привязывает вход, uniform и цель, пишет dispatch по размеру плоскостей.
    pub(crate) fn record(
        &mut self,
        command_buffer: vk::CommandBuffer,
        frame_slot: usize,
        input: LinearRgbPlanes<'_>,
        frame_parameters: DescriptorResource,
        target: &FrameSinkTarget,
        extent: vk::Extent2D,
    ) -> Result<(), GpuError> {
        let pass = match target.description.format {
            StorageImageFormat::A2B10G10R10Unorm => &mut self.ten_bit,
            StorageImageFormat::Rgba16Float => &mut self.half_float,
        };
        let resources = [
            input.red.read_only_binding(),
            input.green.read_only_binding(),
            input.blue.read_only_binding(),
            frame_parameters,
            target.storage,
        ];
        let writes: Vec<BindingWrite> = (0_u32..)
            .zip(resources)
            .map(|(binding, resource)| BindingWrite { binding, resource })
            .collect();
        pass.bind(frame_slot, &writes)?;
        pass.record(command_buffer, frame_slot, &output_push(extent))
    }
}

/// Push-константы выхода: выходу CFA не нужна (вход уже RGB), код любой — RGGB.
fn output_push(extent: vk::Extent2D) -> PassPushConstants {
    PassPushConstants::geometry(extent, CfaCode::from_pattern(isp_model::cfa::BayerPattern::RGGB))
}

//! Кэш вариантов пайплайна ISP видоискателя.
//!
//! Вариант = SPIR-V (источник RAW10/RAW16 × квалификатор формата выхода × способ сведения AWB, один на
//! весь ISP — [`AwbReduction`]) + specialization-константы
//! (размер группы и [`IspStageSet`]). Создаётся лениво при первом кадре с таким ключом; кэш ограничен
//! (`pipeline_variant_capacity`): при переполнении давно не использованный вариант уходит в
//! отложенное уничтожение ([`super::retirement`]) — его могут ещё читать отправленные кадры.

use std::collections::HashMap;

use gpu_vulkan::compute::{BindingKind, ComputePipeline, ComputePipelineDesc, LocalSize, SpecializationConstant};
use gpu_vulkan::context::GpuContext;
use gpu_vulkan::raw_mosaic_preview::MosaicSource;
use gpu_vulkan::storage_image::StorageImageFormat;

use super::awb_reduction::AwbReduction;
use super::error::ViewfinderError;
use super::retirement::RecordSerial;
use super::stages::IspStageSet;
use crate::shaders::blobs;
use crate::tone_base::ToneBaseKind;

/// Вид входа шейдера (разный тип привязки 0 — разный SPIR-V).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum SourceKind {
    /// Упакованный буфер RAW10.
    PackedRaw10,
    /// Картинка `R16_UINT`.
    SampledRaw16,
}

impl SourceKind {
    pub fn of(source: &MosaicSource) -> Self {
        match source {
            MosaicSource::PackedRaw10 { .. } => Self::PackedRaw10,
            MosaicSource::SampledR16 { .. } => Self::SampledRaw16,
        }
    }
}

/// Ключ варианта.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct VariantKey {
    pub source: SourceKind,
    pub format: StorageImageFormat,
    pub stages: IspStageSet,
    /// Основа тона: другая модель — другой вариант (specialization-константа `TONE_BASE_MODEL`).
    pub tone_base: ToneBaseKind,
}

/// Привязки набора дескрипторов (номер = позиция; зеркало `viewfinder_isp.glsl`).
pub(crate) const BINDING_SOURCE: u32 = 0;
pub(crate) const BINDING_PARAMETERS: u32 = 1;
pub(crate) const BINDING_OUTPUT: u32 = 2;
pub(crate) const BINDING_AWB_TILES: u32 = 3;
pub(crate) const BINDING_LENS_SHADING_MAP: u32 = 4;
pub(crate) const BINDING_TONE_BASE: u32 = 5;
pub(crate) const BINDING_LOOK: u32 = 6;
pub(crate) const BINDING_LOOK_LUT: u32 = 7;
/// `constant_id` выбора модели основы тона в шейдере (стадии занимают 2…11 и 13…14).
const TONE_BASE_CONSTANT_ID: u32 = 12;

/// SPIR-V варианта под источник, формат выхода и способ сведения AWB (суффикс `_SUBGROUP` — с быстрым
/// путём `subgroupAdd`; его можно создавать только на устройстве с арифметикой subgroup в compute).
fn spirv(source: SourceKind, format: StorageImageFormat, reduction: AwbReduction) -> &'static [u8] {
    use AwbReduction::{SharedMemoryWindow, SubgroupSingleTile};
    use SourceKind::{PackedRaw10, SampledRaw16};
    use StorageImageFormat::{A2B10G10R10Unorm, Rgba16Float};
    match (source, format, reduction) {
        (PackedRaw10, Rgba16Float, SharedMemoryWindow) => blobs::VIEWFINDER_RAW10_RGBA16F_COMP,
        (PackedRaw10, A2B10G10R10Unorm, SharedMemoryWindow) => blobs::VIEWFINDER_RAW10_RGB10A2_COMP,
        (SampledRaw16, Rgba16Float, SharedMemoryWindow) => blobs::VIEWFINDER_RAW16_RGBA16F_COMP,
        (SampledRaw16, A2B10G10R10Unorm, SharedMemoryWindow) => blobs::VIEWFINDER_RAW16_RGB10A2_COMP,
        (PackedRaw10, Rgba16Float, SubgroupSingleTile) => blobs::VIEWFINDER_RAW10_RGBA16F_SUBGROUP_COMP,
        (PackedRaw10, A2B10G10R10Unorm, SubgroupSingleTile) => blobs::VIEWFINDER_RAW10_RGB10A2_SUBGROUP_COMP,
        (SampledRaw16, Rgba16Float, SubgroupSingleTile) => blobs::VIEWFINDER_RAW16_RGBA16F_SUBGROUP_COMP,
        (SampledRaw16, A2B10G10R10Unorm, SubgroupSingleTile) => blobs::VIEWFINDER_RAW16_RGB10A2_SUBGROUP_COMP,
    }
}

fn source_binding_kind(source: SourceKind) -> BindingKind {
    match source {
        SourceKind::PackedRaw10 => BindingKind::ReadOnlyStorageBuffer,
        SourceKind::SampledRaw16 => BindingKind::SampledImage,
    }
}

struct CachedVariant {
    pipeline: ComputePipeline,
    last_used: RecordSerial,
}

/// Ограниченный кэш вариантов.
pub(crate) struct VariantCache {
    entries: HashMap<VariantKey, CachedVariant>,
    capacity: usize,
    local_size: LocalSize,
    frame_slots: usize,
    awb_reduction: AwbReduction,
}

impl VariantCache {
    pub fn new(capacity: usize, local_size: LocalSize, frame_slots: usize, awb_reduction: AwbReduction) -> Self {
        Self {
            entries: HashMap::new(),
            capacity,
            local_size,
            frame_slots,
            awb_reduction,
        }
    }

    /// Способ сведения статистики AWB у всех вариантов кэша.
    pub fn awb_reduction(&self) -> AwbReduction {
        self.awb_reduction
    }

    /// Сколько вариантов создано и держится.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Готовит вариант `key` для записи `serial`. Если кэш полон, вытесняет самый давно
    /// использованный вариант и отдаёт его вызывающему для отложенного уничтожения.
    pub fn prepare(
        &mut self,
        context: &GpuContext,
        key: VariantKey,
        serial: RecordSerial,
    ) -> Result<Option<ComputePipeline>, ViewfinderError> {
        if let Some(entry) = self.entries.get_mut(&key) {
            entry.last_used = serial;
            return Ok(None);
        }
        // Сначала создаём (ошибка — кэш не тронут), потом вытесняем.
        let pipeline = self.create(context, key)?;
        let evicted = if self.entries.len() >= self.capacity {
            self.evict_least_recently_used()
        } else {
            None
        };
        self.entries.insert(
            key,
            CachedVariant {
                pipeline,
                last_used: serial,
            },
        );
        Ok(evicted)
    }

    /// Пайплайн подготовленного варианта.
    pub fn pipeline_mut(&mut self, key: &VariantKey) -> Result<&mut ComputePipeline, ViewfinderError> {
        self.entries
            .get_mut(key)
            .map(|entry| &mut entry.pipeline)
            .ok_or_else(|| ViewfinderError::InvalidConfig {
                what: format!("вариант {key:?} не подготовлен (внутренняя ошибка порядка вызовов)"),
            })
    }

    /// Подтверждает всем вариантам, что fence слота сигнален (наборы слота снова можно переписывать).
    pub fn release_slot_after_fence(&self, frame_slot: usize) -> Result<(), ViewfinderError> {
        for entry in self.entries.values() {
            entry.pipeline.release_slot_after_fence(frame_slot)?;
        }
        Ok(())
    }

    fn evict_least_recently_used(&mut self) -> Option<ComputePipeline> {
        let oldest = self
            .entries
            .iter()
            .min_by_key(|(_, entry)| entry.last_used)
            .map(|(key, _)| *key)?;
        self.entries.remove(&oldest).map(|entry| entry.pipeline)
    }

    fn create(&self, context: &GpuContext, key: VariantKey) -> Result<ComputePipeline, ViewfinderError> {
        let mut specialization = vec![
            SpecializationConstant {
                constant_id: 0,
                value: self.local_size.x.get(),
            },
            SpecializationConstant {
                constant_id: 1,
                value: self.local_size.y.get(),
            },
        ];
        specialization.extend(key.stages.specialization_constants());
        specialization.push(SpecializationConstant {
            constant_id: TONE_BASE_CONSTANT_ID,
            value: key.tone_base.model_id(),
        });
        let bindings = [
            source_binding_kind(key.source),
            BindingKind::UniformBuffer,
            BindingKind::StorageImageWrite,
            BindingKind::ReadWriteStorageBuffer,
            BindingKind::ReadOnlyStorageBuffer,
            BindingKind::UniformBuffer,
            BindingKind::UniformBuffer,
            BindingKind::ReadOnlyStorageBuffer,
        ];
        Ok(ComputePipeline::new(
            context,
            &ComputePipelineDesc {
                label: "isp-viewfinder",
                spirv: spirv(key.source, key.format, self.awb_reduction),
                bindings: &bindings,
                push_constant_bytes: 0,
                specialization: &specialization,
                frame_slots: self.frame_slots,
            },
        )?)
    }
}

//! Кэш вариантов пайплайна ISP видоискателя.
//!
//! Вариант = SPIR-V (источник RAW10/RAW16 × квалификатор формата выхода × способ сведения AWB, один на
//! весь ISP — [`AwbReduction`]) + specialization-константы (размер группы, [`IspStageSet`], модель основы тона,
//! раскладка групп по плиткам AWB). Создаётся по требованию при первом кадре с таким ключом, а при включённой заготовке
//! ([`VariantPrewarm`]) вместе с ним создаётся его семейство ([`super::variant_family`]): остальные основы, кадр со
//! статистикой и без, обе раскладки — чтобы смена любой из этих осей не компилировала пайплайн в кадре.
//! Кэш ограничен (`pipeline_variant_capacity`): при переполнении самый давно использованный вариант (заготовленные и ни
//! разу не использованные — первыми) уходит в отложенное уничтожение ([`super::retirement`]) — его могут ещё читать
//! отправленные кадры. Всё созданное, вытесненное и потраченное на компиляцию считается ([`VariantCacheStats`]).

use std::collections::HashMap;
use std::time::Instant;

use gpu_vulkan::compute::{BindingKind, ComputePipeline, ComputePipelineDesc, LocalSize, SpecializationConstant};
use gpu_vulkan::context::GpuContext;
use gpu_vulkan::raw_mosaic_preview::MosaicSource;
use gpu_vulkan::storage_image::StorageImageFormat;

use super::awb_group_layout::AwbGroupLayout;
use super::awb_reduction::AwbReduction;
use super::config::VariantPrewarm;
use super::error::ViewfinderError;
use super::retirement::RecordSerial;
use super::stages::{IspStage, IspStageSet};
use super::variant_family::{FamilyContext, related_variants};
use super::variant_stats::{CreationOutcome, VariantCacheStats};
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
    /// Как группы лежат на плитках AWB: короткий путь статистики или общий. Решает CPU по геометрии кадра; у вариантов,
    /// где он ничего не меняет (статистика выключена, сведение через subgroup), всегда `General`.
    pub awb_group_layout: AwbGroupLayout,
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
/// `constant_id` раскладки групп по плиткам AWB (`AWB_GROUPS_IN_ONE_TILE` в шейдере).
const AWB_GROUP_LAYOUT_CONSTANT_ID: u32 = 16;

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

/// Давность использования варианта (для вытеснения). Заготовленный, но ни разу не использованный вариант НЕ «свежее»
/// рабочего: он старше любого использованного и вытесняется первым.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Recency {
    /// Создан заготовкой и ещё ни разу не использовался кадром.
    NeverUsed,
    /// Последний раз использован записью с этим номером.
    UsedAt(RecordSerial),
}

struct CachedVariant {
    pipeline: ComputePipeline,
    last_used: Recency,
}

/// Вытесненный из кэша пайплайн: вызывающий обязан отдать его в отложенное уничтожение (его могут ещё читать
/// отправленные кадры), а не бросить.
pub(crate) type EvictedPipeline = Option<ComputePipeline>;

/// Создатель пайплайнов вариантов: всё, что нужно знать о ISP, кроме ключа (одинаково для всех вариантов).
#[derive(Debug, Clone, Copy)]
pub(crate) struct VariantBuilder {
    local_size: LocalSize,
    frame_slots: usize,
    awb_reduction: AwbReduction,
}

/// Ограниченный кэш вариантов.
pub(crate) struct VariantCache {
    entries: HashMap<VariantKey, CachedVariant>,
    capacity: usize,
    builder: VariantBuilder,
    /// Этот ISP уже записывал кадры со статистикой AWB: тогда у кадра без неё заготавливаются и варианты со статистикой.
    statistics_in_use: bool,
    stats: VariantCacheStats,
}

/// Как кэш создаёт пайплайн варианта (шов: настоящий — [`VariantBuilder::build`], в тестах — с отказами).
type BuildVariant<'a> = dyn FnMut(VariantKey) -> Result<ComputePipeline, ViewfinderError> + 'a;

impl VariantCache {
    pub fn new(capacity: usize, local_size: LocalSize, frame_slots: usize, awb_reduction: AwbReduction) -> Self {
        Self {
            entries: HashMap::new(),
            capacity,
            builder: VariantBuilder {
                local_size,
                frame_slots,
                awb_reduction,
            },
            statistics_in_use: false,
            stats: VariantCacheStats::default(),
        }
    }

    /// Способ сведения статистики AWB у всех вариантов кэша.
    pub fn awb_reduction(&self) -> AwbReduction {
        self.builder.awb_reduction
    }

    /// Сколько вариантов создано и держится.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Счётчики и времена кэша (накопительные) плюс текущее число вариантов.
    pub fn stats(&self) -> VariantCacheStats {
        VariantCacheStats {
            in_cache: self.entries.len(),
            ..self.stats
        }
    }

    /// Готовит вариант `key` для записи `serial`. Если кэш полон, вытесняет самый давно использованный вариант
    /// (никогда не использованные заготовки — первыми) и отдаёт его вызывающему для отложенного уничтожения.
    ///
    /// Если вариант создан сейчас и заготовка разрешена (`prewarm`), следом заготавливается его семейство
    /// ([`related_variants`]): смена основы, зум и чередование статистики не будут компилировать пайплайн в кадре.
    /// Заготовка занимает ТОЛЬКО свободные места и не роняет кадр: ошибка создания считается и пишется в журнал.
    pub fn prepare(
        &mut self,
        context: &GpuContext,
        key: VariantKey,
        serial: RecordSerial,
        prewarm: VariantPrewarm,
    ) -> Result<EvictedPipeline, ViewfinderError> {
        let builder = self.builder;
        self.prepare_with(key, serial, prewarm, &mut |variant| builder.build(context, variant))
    }

    /// То же, что [`Self::prepare`], с заданным способом создания пайплайна (шов для тестов отказов).
    fn prepare_with(
        &mut self,
        key: VariantKey,
        serial: RecordSerial,
        prewarm: VariantPrewarm,
        build: &mut BuildVariant<'_>,
    ) -> Result<EvictedPipeline, ViewfinderError> {
        // Раньше проверки кэша: даже попадание в кэш говорит, что статистика в работе.
        if key.stages.contains(IspStage::AwbStatistics) {
            self.statistics_in_use = true;
        }
        if let Some(entry) = self.entries.get_mut(&key) {
            // Использование обновляет давность: рабочий вариант не вытесняется раньше заготовленных и забытых.
            entry.last_used = Recency::UsedAt(serial);
            return Ok(None);
        }
        // Сначала создаём (ошибка — кэш не тронут), потом вытесняем.
        let started = Instant::now();
        let built = build(key);
        let elapsed = started.elapsed();
        let pipeline = match built {
            Ok(pipeline) => {
                self.stats.note_frame_creation(CreationOutcome::Created, elapsed);
                pipeline
            }
            Err(error) => {
                self.stats.note_frame_creation(CreationOutcome::Failed, elapsed);
                return Err(error);
            }
        };
        let evicted = if self.entries.len() >= self.capacity {
            self.evict_least_recently_used()
        } else {
            None
        };
        self.entries.insert(
            key,
            CachedVariant {
                pipeline,
                last_used: Recency::UsedAt(serial),
            },
        );
        if prewarm == VariantPrewarm::AllRelated {
            self.prewarm_related(key, build);
        }
        Ok(evicted)
    }

    /// Заготовка семейства ключа `key`: создаёт родственные варианты, которых нет. Занимает ТОЛЬКО свободные места
    /// кэша: не вытесняет ничего (ни вариант текущего кадра, ни недавно использованные), а при нехватке места
    /// останавливается. Заготовленный вариант помечается «ни разу не использован». Ошибка создания не роняет кадр
    /// (рисуется вариантом `key`, он уже готов): она считается и пишется в журнал.
    fn prewarm_related(&mut self, key: VariantKey, build: &mut BuildVariant<'_>) {
        let context = FamilyContext {
            awb_reduction: self.builder.awb_reduction,
            statistics_in_use: self.statistics_in_use,
        };
        let started = Instant::now();
        let (mut created, mut failed) = (0_u64, 0_u64);
        for sibling in related_variants(key, context) {
            if self.entries.contains_key(&sibling) {
                continue;
            }
            if self.entries.len() >= self.capacity {
                break;
            }
            match build(sibling) {
                Ok(pipeline) => {
                    self.entries.insert(
                        sibling,
                        CachedVariant {
                            pipeline,
                            last_used: Recency::NeverUsed,
                        },
                    );
                    created += 1;
                }
                Err(error) => {
                    tracing::warn!(
                        target: "fasticamera::isp_viewfinder",
                        %error,
                        ?sibling,
                        "заготовка родственного варианта не удалась: он будет создан на первом кадре, которому понадобится"
                    );
                    failed += 1;
                }
            }
        }
        if created + failed > 0 {
            self.stats.note_prewarm(created, failed, started.elapsed());
        }
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
        self.stats.evicted += 1;
        self.entries.remove(&oldest).map(|entry| entry.pipeline)
    }
}

impl VariantBuilder {
    /// Создаёт (компилирует) пайплайн варианта `key`.
    pub fn build(&self, context: &GpuContext, key: VariantKey) -> Result<ComputePipeline, ViewfinderError> {
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
        specialization.push(SpecializationConstant {
            constant_id: AWB_GROUP_LAYOUT_CONSTANT_ID,
            value: key.awb_group_layout.specialization_value(),
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

#[cfg(test)]
#[path = "variants_tests.rs"]
mod tests;

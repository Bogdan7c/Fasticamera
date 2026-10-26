//! Диагностика `ahb_roundtrip`: кадр, записанный CPU в `AHardwareBuffer`, доходит до compute-шейдера
//! по импорту без копий (инвариант 1 «0 копий»), а контрольные суммы GPU и CPU совпадают.
//!
//! Что проходит проверку: аллокация AHB → запись известного образца с учётом шага строки →
//! импорт (через кэш) → compute-контрольные суммы на GPU → сравнение с CPU-эталоном по тем же байтам →
//! повторный импорт того же буфера обязан быть попаданием в кэш → объекты Vulkan вернулись к
//! исходному числу. Функция запускается на телефоне smoke-сценарием `ahb-import`.

use std::collections::BTreeMap;
use std::fmt;
use std::num::NonZeroUsize;

use super::hardware_buffer::{HardwareBufferError, HardwareBufferRef};
use super::hardware_buffer_ffi::{AHardwareBufferDesc, USAGE_CPU_WRITE_OFTEN, USAGE_GPU_SAMPLED_IMAGE};
use super::import::{
    HardwareBufferImportError, HardwareBufferVulkanProperties, ImportedHandles, query_hardware_buffer_properties,
};
use super::import_cache::AndroidImportCache;
use crate::context::GpuContext;
use crate::error::GpuError;
use crate::hardware_buffer_cache::checksum_gpu::{ChecksumRunnerConfig, ChecksumSource, GpuChecksumRunner};
use crate::hardware_buffer_cache::queue_handoff::{BufferAcquire, ImageAcquire, ReleaseTarget};
use crate::hardware_buffer_cache::raw_reference::{
    BufferTooSmall, ChecksumPair, RawPixelFormat, RowGeometry, StrideError, checksum_strided_words,
    checksum_u16_texels, fill_pattern, resolve_row_stride_bytes,
};
use crate::hardware_buffer_cache::{
    AcquireOutcome, HardwareBufferId, ImportCacheCounters, ImportLayout, SubmissionSerial,
};
use crate::live_objects::LiveObjectSnapshot;

/// Параметры диагностики.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AhbRoundtripParams {
    /// Ширина, пиксели.
    pub width: u32,
    /// Высота, пиксели.
    pub height: u32,
    /// Формат RAW (определяет и путь импорта: RAW16 → картинка, RAW10 → буфер).
    pub format: RawPixelFormat,
}

/// Ёмкость кэша диагностики: двух слотов хватает, нужен лишь один (проверяем попадание).
const DIAGNOSTIC_CACHE_CAPACITY: NonZeroUsize = NonZeroUsize::MIN.saturating_add(1);

/// Итог диагностики.
#[derive(Debug, Clone)]
pub enum AhbRoundtripOutcome {
    /// Полный прогон; смотрите [`AhbRoundtripReport::matched`].
    Completed(Box<AhbRoundtripReport>),
    /// Телефон не выделяет буфер с таким форматом/usage (`AHardwareBuffer_allocate` отказал).
    /// Это данные для отчёта, а не сбой диагностики.
    AllocationUnsupported {
        /// Код возврата NDK.
        status: i32,
        /// Запрошенное описание.
        requested: AHardwareBufferDesc,
    },
}

/// Отчёт полного прогона.
#[derive(Debug, Clone)]
pub struct AhbRoundtripReport {
    /// Параметры запуска.
    pub params: AhbRoundtripParams,
    /// Описание буфера от `AHardwareBuffer_describe` (формат, `stride`, usage).
    pub description: AHardwareBufferDesc,
    /// `AHardwareBuffer_getId`.
    pub buffer_id: HardwareBufferId,
    /// Свойства буфера для Vulkan (allocationSize, memoryTypeBits, format/externalFormat).
    pub vulkan_properties: HardwareBufferVulkanProperties,
    /// Выведенный шаг строки, байт (для RAW10 — по согласованности с `allocationSize`).
    pub row_stride_bytes: u64,
    /// Путь импорта (картинка или буфер) вместе с размерами.
    pub import_path: ImportLayout,
    /// Контрольные суммы GPU по импортированному ресурсу (первый запуск).
    pub gpu_checksum: ChecksumPair,
    /// Контрольные суммы CPU по тем же байтам.
    pub cpu_checksum: ChecksumPair,
    /// GPU = CPU (обе суммы) и во втором запуске, через кэш, тоже.
    pub matched: bool,
    /// Чем закончился второй импорт того же буфера (ожидается `Hit`).
    pub second_import: AcquireOutcome,
    /// Счётчики кэша после двух импортов.
    pub cache_counters: ImportCacheCounters,
    /// Объекты Vulkan после уничтожения всего созданного диагностикой равны исходным.
    pub live_objects_restored: bool,
    /// Снимок живых объектов до начала.
    pub live_objects_before: LiveObjectSnapshot,
    /// Снимок после завершения.
    pub live_objects_after: LiveObjectSnapshot,
    /// Сколько сообщений-ошибок validation появилось за время диагностики.
    pub validation_errors_during: u64,
    /// Сколько раз за время диагностики сработал каждый VUID из списка известных отступлений
    /// (решение 35). В `validation_errors_during` эти сообщения не входят.
    pub known_deviations_during: BTreeMap<String, u64>,
}

impl fmt::Display for AhbRoundtripReport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{:?} {}x{} ahb(format=0x{:X} stride={} usage=0x{:X}) alloc={} path={:?} \
             gpu(sum=0x{:08X} mix=0x{:08X}) cpu(sum=0x{:08X} mix=0x{:08X}) matched={} second={:?} \
             cache(hits={} misses={}) live_restored={} validation_errors={} known_deviations={:?}",
            self.params.format,
            self.params.width,
            self.params.height,
            self.description.format,
            self.description.stride,
            self.description.usage,
            self.vulkan_properties.allocation_size,
            self.import_path,
            self.gpu_checksum.sum,
            self.gpu_checksum.mix,
            self.cpu_checksum.sum,
            self.cpu_checksum.mix,
            self.matched,
            self.second_import,
            self.cache_counters.hits,
            self.cache_counters.misses,
            self.live_objects_restored,
            self.validation_errors_during,
            self.known_deviations_during,
        )
    }
}

/// Ошибка диагностики (сбой, а не «телефон не умеет»).
#[derive(Debug, thiserror::Error)]
pub enum AhbRoundtripError {
    /// Недопустимые параметры (нулевой размер).
    #[error("недопустимые параметры диагностики: {what}")]
    InvalidParams {
        /// Что именно.
        what: &'static str,
    },
    /// Ошибка Vulkan.
    #[error(transparent)]
    Gpu(#[from] GpuError),
    /// Ошибка NDK при работе с буфером.
    #[error(transparent)]
    Buffer(#[from] HardwareBufferError),
    /// Ошибка импорта.
    #[error(transparent)]
    Import(#[from] HardwareBufferImportError),
    /// Не удалось вывести шаг строки.
    #[error(transparent)]
    Stride(#[from] StrideError),
    /// Байт не хватает для геометрии.
    #[error(transparent)]
    TooSmall(#[from] BufferTooSmall),
    /// Геометрия не сходится с аллокацией или шаг не кратен слову.
    #[error("геометрия буфера не сходится: {what}")]
    Geometry {
        /// Что именно.
        what: String,
    },
}

/// Запускает диагностику.
pub fn ahb_roundtrip(
    context: &GpuContext,
    params: AhbRoundtripParams,
) -> Result<AhbRoundtripOutcome, AhbRoundtripError> {
    if params.width == 0 || params.height == 0 {
        return Err(AhbRoundtripError::InvalidParams {
            what: "ширина и высота должны быть больше нуля",
        });
    }
    let live_objects_before = context.live_objects();
    let validation_before = context.validation_report();

    let requested = AHardwareBufferDesc {
        width: params.width,
        height: params.height,
        layers: 1,
        format: params.format.ahb_format_code(),
        usage: USAGE_GPU_SAMPLED_IMAGE | USAGE_CPU_WRITE_OFTEN,
        ..AHardwareBufferDesc::default()
    };
    let mut hardware_buffer = match HardwareBufferRef::allocate(&requested) {
        Ok(buffer) => buffer,
        Err(HardwareBufferError::AllocateFailed { status }) => {
            return Ok(AhbRoundtripOutcome::AllocationUnsupported { status, requested });
        }
        Err(other) => return Err(other.into()),
    };

    let description = hardware_buffer.describe();
    let buffer_id = hardware_buffer.id()?;
    let vulkan_properties = query_hardware_buffer_properties(context, &hardware_buffer)?;
    let (geometry, import_path) = resolve_geometry(&params, &description, &vulkan_properties)?;

    let cpu_checksum = write_pattern(&mut hardware_buffer, &params, &geometry)?;

    // Всё, что создаёт Vulkan-объекты, живёт в этом блоке: к его концу они обязаны исчезнуть.
    let (gpu_checksum, second_import, second_matches, cache_counters) = {
        let runner = GpuChecksumRunner::new(context, ChecksumRunnerConfig::default())?;
        let mut cache = AndroidImportCache::new(context, DIAGNOSTIC_CACHE_CAPACITY);

        let first_serial = SubmissionSerial(1);
        let first_source = {
            let acquired = cache.acquire(&hardware_buffer, import_path, first_serial)?;
            checksum_source(acquired.import.handles(), &geometry)?
        };
        let gpu_checksum = runner.run(&first_source)?;
        // `run` дождался GPU: серия 1 завершена.
        cache.retire_completed(first_serial);

        // Тот же буфер ещё раз: обязан быть попаданием в кэш, а не новым импортом.
        let second_serial = SubmissionSerial(2);
        let (second_import, second_source) = {
            let acquired = cache.acquire(&hardware_buffer, import_path, second_serial)?;
            (acquired.outcome, checksum_source(acquired.import.handles(), &geometry)?)
        };
        let second_checksum = runner.run(&second_source)?;
        cache.retire_completed(second_serial);
        (
            gpu_checksum,
            second_import,
            second_checksum == cpu_checksum,
            cache.counters(),
        )
    };

    drop(hardware_buffer);
    let live_objects_after = context.live_objects();
    let validation_after = context.validation_report();
    let validation_errors_during = validation_after.errors - validation_before.errors;
    let known_deviations_during = validation_after.known_deviations_since(&validation_before);
    Ok(AhbRoundtripOutcome::Completed(Box::new(AhbRoundtripReport {
        params,
        description,
        buffer_id,
        vulkan_properties,
        row_stride_bytes: geometry.row_stride_bytes,
        import_path,
        gpu_checksum,
        cpu_checksum,
        matched: gpu_checksum == cpu_checksum && second_matches,
        second_import,
        cache_counters,
        live_objects_restored: live_objects_after == live_objects_before,
        live_objects_before,
        live_objects_after,
        validation_errors_during,
        known_deviations_during,
    })))
}

/// Геометрия строк и раскладка импорта по описанию AHB и свойствам Vulkan.
fn resolve_geometry(
    params: &AhbRoundtripParams,
    description: &AHardwareBufferDesc,
    properties: &HardwareBufferVulkanProperties,
) -> Result<(RowGeometry, ImportLayout), AhbRoundtripError> {
    let row_stride_bytes = resolve_row_stride_bytes(
        params.format,
        params.width,
        params.height,
        description.stride,
        properties.allocation_size,
    )?;
    let geometry = RowGeometry {
        rows: params.height,
        row_valid_bytes: params.format.valid_row_bytes(params.width),
        row_stride_bytes,
    };
    // Отображение CPU покрывает `stride × height` байт: оно обязано помещаться в аллокацию.
    let mapped_bytes = row_stride_bytes * u64::from(params.height);
    if mapped_bytes > properties.allocation_size {
        return Err(AhbRoundtripError::Geometry {
            what: format!(
                "stride {row_stride_bytes} × {} строк = {mapped_bytes} байт больше аллокации {}",
                params.height, properties.allocation_size
            ),
        });
    }
    let row_stride_u32 = u32::try_from(row_stride_bytes).map_err(|_| AhbRoundtripError::Geometry {
        what: format!("шаг строки {row_stride_bytes} не помещается в u32"),
    })?;
    let layout = match params.format {
        RawPixelFormat::Raw16 => ImportLayout::SampledImageR16Uint {
            width: params.width,
            height: params.height,
        },
        RawPixelFormat::Raw10 => ImportLayout::PackedBuffer {
            row_stride_bytes: row_stride_u32,
            rows: params.height,
        },
    };
    Ok((geometry, layout))
}

/// Записывает образец в буфер (CPU, с шагом строки), разблокирует (блокирующий unlock — GPU увидит
/// данные без fence) и возвращает контрольные суммы CPU по тем же байтам.
///
/// Образец готовится в `Vec`, а в буфер копируется целиком: читать из write-only отображения
/// (write-combined память) нельзя полагаться, а «те же байты» обеспечивает эта копия.
fn write_pattern(
    hardware_buffer: &mut HardwareBufferRef,
    params: &AhbRoundtripParams,
    geometry: &RowGeometry,
) -> Result<ChecksumPair, AhbRoundtripError> {
    let mapped_len = usize::try_from(geometry.row_stride_bytes * u64::from(geometry.rows)).map_err(|_| {
        AhbRoundtripError::Geometry {
            what: "размер отображения не помещается в usize".to_owned(),
        }
    })?;
    let mut pattern = vec![0_u8; mapped_len];
    if !fill_pattern(&mut pattern, params.format, geometry) {
        return Err(AhbRoundtripError::Geometry {
            what: "образец не помещается в буфер".to_owned(),
        });
    }
    let cpu_checksum = match params.format {
        RawPixelFormat::Raw16 => checksum_u16_texels(&pattern, params.width, geometry)?,
        RawPixelFormat::Raw10 => checksum_strided_words(&pattern, geometry)?,
    };

    let mut mapping = hardware_buffer.lock_for_cpu_write()?;
    // SAFETY: mapped_len = stride × height, проверено в resolve_geometry: не больше allocationSize;
    // отображение gralloc покрывает как минимум это.
    unsafe { mapping.bytes_mut(mapped_len) }.copy_from_slice(&pattern);
    // Блокирующий unlock: после возврата запись завершена и видна GPU. (Вариант с fd вместо блокировки —
    // ждать через GpuWaitFence из `gpu_vulkan::sync` — подключит главный агент вместе с smoke.)
    mapping.unlock()?;
    Ok(cpu_checksum)
}

/// Источник для compute по дескрипторам импорта: acquire у FOREIGN до и release в FOREIGN после.
fn checksum_source(handles: ImportedHandles, geometry: &RowGeometry) -> Result<ChecksumSource, AhbRoundtripError> {
    match handles {
        ImportedHandles::SampledImage { view, image } => Ok(ChecksumSource::SampledImage {
            view,
            image,
            width: u32::try_from(geometry.row_valid_bytes / 2).map_err(|_| AhbRoundtripError::Geometry {
                what: "ширина не помещается в u32".to_owned(),
            })?,
            height: geometry.rows,
            acquire: ImageAcquire::FromForeignQueue,
            release: ReleaseTarget::ForeignQueue,
        }),
        ImportedHandles::PackedBuffer { buffer, size } => {
            if !geometry.row_stride_bytes.is_multiple_of(4) {
                return Err(AhbRoundtripError::Geometry {
                    what: format!("шаг строки {} байт не кратен размеру слова", geometry.row_stride_bytes),
                });
            }
            Ok(ChecksumSource::PackedBuffer {
                buffer,
                size,
                row_words: (geometry.row_valid_bytes / 4) as u32,
                stride_words: (geometry.row_stride_bytes / 4) as u32,
                rows: geometry.rows,
                acquire: BufferAcquire::FromForeignQueue,
                release: ReleaseTarget::ForeignQueue,
            })
        }
    }
}

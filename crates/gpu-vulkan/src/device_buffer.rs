//! [`DeviceBuffer`] — storage-буфер в памяти GPU (device-local) для промежуточных плоскостей
//! многопроходных compute-конвейеров (сессия 07: качественный демозаик полного кадра, стенд V4).
//!
//! # Зачем отдельный тип
//! [`crate::storage_image::StorageImage`] умеет только 10-бит UNORM и fp16 — для паритета
//! демозаика Menon 2007 с эталоном `isp-model` (f32) этого мало, а [`crate::host_buffer::HostBuffer`]
//! живёт в памяти, видимой CPU (медленно для GPU на полном кадре). Плоскость f32 в device-local
//! буфере — самый простой формат, который сохраняет точность эталона и быстрый для шейдера.
//!
//! # Лимит диапазона
//! Дескриптор storage-буфера не может видеть больше `maxStorageBufferRange` байт
//! (`VUID-VkWriteDescriptorSet-descriptorType-00333`). На Adreno 740 это 128 МиБ, поэтому буфер
//! больше лимита отвергается при создании: одна плоскость f32 кадра 4096×2304 — 36 МиБ, многоканальные
//! данные вызывающий раскладывает по отдельным буферам-плоскостям.
//!
//! # Синхронизация
//! Буфер не отслеживает «кто писал последним»: между проходами вызывающий пишет одну глобальную
//! зависимость [`record_compute_pass_dependency`] (рекомендация Khronos «Synchronization Examples»:
//! глобальный барьер памяти дешевле набора барьеров по ресурсам, когда не нужны переходы раскладок
//! и передача владения между очередями).

use ash::vk;

use crate::compute::DescriptorResource;
use crate::context::{GpuContext, MemoryLocation};
use crate::error::{GpuError, run_step};
use crate::live_objects::{OwnedBuffer, OwnedDeviceMemory};

/// Буфер в памяти GPU. Порядок полей = порядок уничтожения: буфер → память.
pub struct DeviceBuffer {
    buffer: OwnedBuffer,
    _memory: OwnedDeviceMemory,
    size_bytes: u64,
    /// Контекст нужен командам, которые буфер записывает сам (`record_clear_for_compute`), и блокирующим
    /// загрузке и чтению (тесты): сам буфер держит устройство через `Owned*`.
    context: GpuContext,
}

impl std::fmt::Debug for DeviceBuffer {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "DeviceBuffer({} байт)", self.size_bytes)
    }
}

/// Usage буфера: storage для шейдеров + приёмник/источник копирования (загрузка и чтение в тестах,
/// диагностика). Лишние биты transfer на скорость storage-доступа не влияют.
const DEVICE_BUFFER_USAGE: vk::BufferUsageFlags = vk::BufferUsageFlags::from_raw(
    vk::BufferUsageFlags::STORAGE_BUFFER.as_raw()
        | vk::BufferUsageFlags::TRANSFER_SRC.as_raw()
        | vk::BufferUsageFlags::TRANSFER_DST.as_raw(),
);

impl DeviceBuffer {
    /// Создаёт буфер `size_bytes` байт. Ноль, размер не кратный 4 (шейдер читает 32-битные слова) и
    /// размер больше `maxStorageBufferRange` — `InvalidArgument` до создания объектов Vulkan.
    /// Содержимое не определено: первый проход обязан записать то, что потом читает.
    pub fn new(context: &GpuContext, size_bytes: u64) -> Result<Self, GpuError> {
        validate_size(size_bytes, max_storage_buffer_range(context))?;
        let device = context.device();
        let info = vk::BufferCreateInfo::default()
            .size(size_bytes)
            .usage(DEVICE_BUFFER_USAGE)
            .sharing_mode(vk::SharingMode::EXCLUSIVE);
        // SAFETY: устройство валидно; info — живая локальная структура.
        let buffer = OwnedBuffer::from_raw(
            context,
            run_step(
                unsafe { device.create_buffer(&info, None) },
                "vkCreateBuffer(DeviceBuffer)",
            )?,
        );
        // SAFETY: буфер только что создан этим устройством.
        let requirements = unsafe { device.get_buffer_memory_requirements(buffer.handle()) };
        let memory = context.allocate_memory(requirements, MemoryLocation::DeviceLocal, "DeviceBuffer")?;
        // SAFETY: память подобрана по требованиям буфера и ещё не привязана; смещение 0.
        run_step(
            unsafe { device.bind_buffer_memory(buffer.handle(), memory.handle(), 0) },
            "vkBindBufferMemory(DeviceBuffer)",
        )?;
        Ok(Self {
            buffer,
            _memory: memory,
            size_bytes,
            context: context.clone(),
        })
    }

    /// Размер, байт.
    pub fn size_bytes(&self) -> u64 {
        self.size_bytes
    }

    /// Ресурс для привязки, в которую шейдер пишет (`ReadWriteStorageBuffer`; подходит и к «только чтение»).
    pub fn read_write_binding(&self) -> DescriptorResource {
        DescriptorResource::ReadWriteBuffer {
            buffer: self.buffer.handle(),
            range_bytes: self.size_bytes,
        }
    }

    /// Ресурс только для чтения (`ReadOnlyStorageBuffer`): к привязке записи не подходит, поэтому
    /// ошибка «проход пишет во вход» ловится ещё при записи набора дескрипторов.
    pub fn read_only_binding(&self) -> DescriptorResource {
        DescriptorResource::ReadOnlyBuffer {
            buffer: self.buffer.handle(),
            range_bytes: self.size_bytes,
        }
    }

    /// Обнуляет первые `byte_count` байт буфера командой GPU (`vkCmdFillBuffer`) и делает нули видимыми
    /// compute-шейдерам (накопление счётчиков начинается с нуля). Остальная часть буфера не меняется.
    ///
    /// `byte_count` — от 4 до размера буфера, кратно 4; иначе `InvalidArgument` ДО записи команд.
    ///
    /// Первая область барьера перед заливкой охватывает все более ранние compute-команды очереди (в том
    /// числе прошлых сабмитов), поэтому прошлые записи и чтения этого буфера шейдерами закончены до
    /// заливки (WAR достаточно зависимости исполнения: `srcAccessMask` — запись, чтения она покрывает).
    /// Записывать вне динамического рендеринга.
    pub fn record_clear_for_compute(&self, command_buffer: vk::CommandBuffer, byte_count: u64) -> Result<(), GpuError> {
        validate_clear_size(byte_count, self.size_bytes)?;
        let device = self.context.device();
        let before_fill = self.whole_buffer_barrier(
            (
                vk::PipelineStageFlags2::COMPUTE_SHADER,
                vk::AccessFlags2::SHADER_STORAGE_WRITE,
            ),
            (vk::PipelineStageFlags2::CLEAR, vk::AccessFlags2::TRANSFER_WRITE),
        );
        let after_fill = self.whole_buffer_barrier(
            (vk::PipelineStageFlags2::CLEAR, vk::AccessFlags2::TRANSFER_WRITE),
            (
                vk::PipelineStageFlags2::COMPUTE_SHADER,
                vk::AccessFlags2::SHADER_STORAGE_READ | vk::AccessFlags2::SHADER_STORAGE_WRITE,
            ),
        );
        // SAFETY: командный буфер в состоянии записи вне рендеринга (контракт вызывающего); буфер создан с
        // TRANSFER_DST; смещение 0 и размер (кратный 4, в пределах буфера) проверены выше; барьеры живут
        // до конца вызова.
        unsafe {
            device.cmd_pipeline_barrier2(
                command_buffer,
                &vk::DependencyInfo::default().buffer_memory_barriers(&[before_fill]),
            );
            device.cmd_fill_buffer(command_buffer, self.buffer.handle(), 0, byte_count, 0);
            device.cmd_pipeline_barrier2(
                command_buffer,
                &vk::DependencyInfo::default().buffer_memory_barriers(&[after_fill]),
            );
        }
        Ok(())
    }

    /// Барьер всего буфера между двумя областями `(стадия, доступ)`.
    fn whole_buffer_barrier(
        &self,
        (source_stage, source_access): (vk::PipelineStageFlags2, vk::AccessFlags2),
        (destination_stage, destination_access): (vk::PipelineStageFlags2, vk::AccessFlags2),
    ) -> vk::BufferMemoryBarrier2<'static> {
        vk::BufferMemoryBarrier2::default()
            .src_stage_mask(source_stage)
            .src_access_mask(source_access)
            .dst_stage_mask(destination_stage)
            .dst_access_mask(destination_access)
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .buffer(self.buffer.handle())
            .offset(0)
            .size(vk::WHOLE_SIZE)
    }

    /// Загружает `bytes` в начало буфера и ждёт завершения (разовая отправка с таймаутом контекста).
    /// Только тесты и фича `test-support`: на горячем пути ожиданий GPU нет.
    ///
    /// Контракт: GPU сейчас не использует буфер. Загрузка идёт `vkCmdUpdateBuffer` кусками по 64 КиБ
    /// (лимит команды), поэтому staging-буфер не нужен; данные копируются в командный буфер.
    #[cfg(any(test, feature = "test-support"))]
    pub fn upload_blocking(&self, bytes: &[u8]) -> Result<(), GpuError> {
        test_support::upload_blocking(self, bytes)
    }

    /// Читает весь буфер на CPU и ждёт завершения. Только тесты и фича `test-support`.
    ///
    /// Записи compute-шейдеров до этого вызова (в более ранних сабмитах той же очереди) становятся
    /// видимы копированию барьером внутри разовой отправки.
    #[cfg(any(test, feature = "test-support"))]
    pub fn read_back_blocking(&self) -> Result<Vec<u8>, GpuError> {
        test_support::read_back_blocking(self)
    }
}

/// Глобальная зависимость между compute-проходами: все записи compute-шейдеров ДО барьера видимы
/// чтениям и записям compute-шейдеров ПОСЛЕ него (RAW, WAW), а WAR закрыт зависимостью исполнения.
///
/// Первая область барьера охватывает все более ранние команды очереди в порядке отправки, в том числе
/// команды прошлых сабмитов, поэтому тот же барьер в начале кадра защищает буферы, которые прошлый
/// кадр ещё читал или писал (одна очередь — без fence и семафоров). Переходов раскладки картинок
/// барьер не делает: для них есть intent-методы `StorageImage`.
pub fn record_compute_pass_dependency(context: &GpuContext, command_buffer: vk::CommandBuffer) {
    let barriers = [vk::MemoryBarrier2::default()
        .src_stage_mask(vk::PipelineStageFlags2::COMPUTE_SHADER)
        .src_access_mask(vk::AccessFlags2::SHADER_STORAGE_WRITE)
        .dst_stage_mask(vk::PipelineStageFlags2::COMPUTE_SHADER)
        .dst_access_mask(vk::AccessFlags2::SHADER_STORAGE_READ | vk::AccessFlags2::SHADER_STORAGE_WRITE)];
    let dependency = vk::DependencyInfo::default().memory_barriers(&barriers);
    // SAFETY: командный буфер в состоянии записи вне динамического рендеринга (контракт вызывающего);
    // массив барьеров живёт до конца вызова.
    unsafe { context.device().cmd_pipeline_barrier2(command_buffer, &dependency) };
}

/// Проверка размера заливки — чистая функция (тестируется без GPU).
fn validate_clear_size(byte_count: u64, buffer_bytes: u64) -> Result<(), GpuError> {
    if byte_count == 0 || !byte_count.is_multiple_of(4) || byte_count > buffer_bytes {
        return Err(GpuError::InvalidArgument {
            what: format!("заливка {byte_count} байт в DeviceBuffer {buffer_bytes} байт: нужно 4..=размер, кратно 4"),
        });
    }
    Ok(())
}

/// `maxStorageBufferRange` устройства.
fn max_storage_buffer_range(context: &GpuContext) -> u32 {
    // SAFETY: физическое устройство получено из этого instance; вызов только читает свойства.
    let properties = unsafe {
        context
            .instance()
            .get_physical_device_properties(context.physical_device())
    };
    properties.limits.max_storage_buffer_range
}

/// Проверка размера — чистая функция (тестируется без GPU).
fn validate_size(size_bytes: u64, max_storage_range: u32) -> Result<(), GpuError> {
    if size_bytes == 0 || !size_bytes.is_multiple_of(4) {
        return Err(GpuError::InvalidArgument {
            what: format!("DeviceBuffer {size_bytes} байт: нужен ненулевой размер, кратный 4"),
        });
    }
    if size_bytes > u64::from(max_storage_range) {
        return Err(GpuError::InvalidArgument {
            what: format!(
                "DeviceBuffer {size_bytes} байт больше maxStorageBufferRange {max_storage_range}: \
                 разложите данные по нескольким буферам"
            ),
        });
    }
    Ok(())
}

#[cfg(any(test, feature = "test-support"))]
mod test_support {
    use ash::vk;

    use super::DeviceBuffer;
    use crate::error::GpuError;
    use crate::host_buffer::{HostBuffer, HostBufferUsage};

    /// Лимит `vkCmdUpdateBuffer` на одну команду, байт (`VUID-vkCmdUpdateBuffer-dataSize-00037`).
    const UPDATE_CHUNK_BYTES: usize = 65_536;

    pub(super) fn upload_blocking(target: &DeviceBuffer, bytes: &[u8]) -> Result<(), GpuError> {
        if bytes.is_empty() || !bytes.len().is_multiple_of(4) || bytes.len() as u64 > target.size_bytes {
            return Err(GpuError::InvalidArgument {
                what: format!(
                    "загрузка {} байт в DeviceBuffer {} байт: нужно 1..=размер, кратно 4",
                    bytes.len(),
                    target.size_bytes
                ),
            });
        }
        let context = &target.context;
        let device = context.device();
        context.run_one_shot(|command_buffer| {
            // Прежние compute-записи и чтения этого буфера (ранее на очереди) — до перезаписи копированием.
            let before = buffer_barrier(
                target.buffer.handle(),
                (
                    vk::PipelineStageFlags2::COMPUTE_SHADER,
                    vk::AccessFlags2::SHADER_STORAGE_WRITE,
                ),
                (vk::PipelineStageFlags2::COPY, vk::AccessFlags2::TRANSFER_WRITE),
            );
            // SAFETY: командный буфер в состоянии записи (контракт run_one_shot); барьер живёт до конца вызова.
            unsafe {
                device.cmd_pipeline_barrier2(
                    command_buffer,
                    &vk::DependencyInfo::default().buffer_memory_barriers(&[before]),
                );
            }
            for (chunk_index, chunk) in bytes.chunks(UPDATE_CHUNK_BYTES).enumerate() {
                let offset = (chunk_index * UPDATE_CHUNK_BYTES) as u64;
                // SAFETY: буфер с usage TRANSFER_DST; смещение и размер кратны 4, размер ≤ 65536,
                // диапазон внутри буфера (проверено выше).
                unsafe { device.cmd_update_buffer(command_buffer, target.buffer.handle(), offset, chunk) };
            }
            // Загруженное — видимо следующим compute-проходам.
            let after = buffer_barrier(
                target.buffer.handle(),
                (vk::PipelineStageFlags2::COPY, vk::AccessFlags2::TRANSFER_WRITE),
                (
                    vk::PipelineStageFlags2::COMPUTE_SHADER,
                    vk::AccessFlags2::SHADER_STORAGE_READ | vk::AccessFlags2::SHADER_STORAGE_WRITE,
                ),
            );
            // SAFETY: как выше.
            unsafe {
                device.cmd_pipeline_barrier2(
                    command_buffer,
                    &vk::DependencyInfo::default().buffer_memory_barriers(&[after]),
                );
            }
            Ok(())
        })
    }

    pub(super) fn read_back_blocking(source: &DeviceBuffer) -> Result<Vec<u8>, GpuError> {
        let context = &source.context;
        let staging = HostBuffer::new(context, source.size_bytes, HostBufferUsage::ReadWriteStorage)?;
        let device = context.device();
        context.run_one_shot(|command_buffer| {
            let before = buffer_barrier(
                source.buffer.handle(),
                (
                    vk::PipelineStageFlags2::COMPUTE_SHADER,
                    vk::AccessFlags2::SHADER_STORAGE_WRITE,
                ),
                (vk::PipelineStageFlags2::COPY, vk::AccessFlags2::TRANSFER_READ),
            );
            let to_host = buffer_barrier(
                staging.buffer_handle(),
                (vk::PipelineStageFlags2::COPY, vk::AccessFlags2::TRANSFER_WRITE),
                (vk::PipelineStageFlags2::HOST, vk::AccessFlags2::HOST_READ),
            );
            let region = vk::BufferCopy::default().size(source.size_bytes);
            // SAFETY: командный буфер в состоянии записи; источник с TRANSFER_SRC, приёмник с TRANSFER_DST
            // того же размера; барьеры живут до конца вызовов.
            unsafe {
                device.cmd_pipeline_barrier2(
                    command_buffer,
                    &vk::DependencyInfo::default().buffer_memory_barriers(&[before]),
                );
                device.cmd_copy_buffer(
                    command_buffer,
                    source.buffer.handle(),
                    staging.buffer_handle(),
                    &[region],
                );
                device.cmd_pipeline_barrier2(
                    command_buffer,
                    &vk::DependencyInfo::default().buffer_memory_barriers(&[to_host]),
                );
            }
            Ok(())
        })?;
        let length = usize::try_from(source.size_bytes).map_err(|_| GpuError::InvalidArgument {
            what: "размер буфера не помещается в usize".to_owned(),
        })?;
        staging.read_bytes(0, length)
    }

    /// Барьер всего буфера между двумя областями `(стадия, доступ)`.
    fn buffer_barrier(
        buffer: vk::Buffer,
        (source_stage, source_access): (vk::PipelineStageFlags2, vk::AccessFlags2),
        (destination_stage, destination_access): (vk::PipelineStageFlags2, vk::AccessFlags2),
    ) -> vk::BufferMemoryBarrier2<'static> {
        vk::BufferMemoryBarrier2::default()
            .src_stage_mask(source_stage)
            .src_access_mask(source_access)
            .dst_stage_mask(destination_stage)
            .dst_access_mask(destination_access)
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .buffer(buffer)
            .offset(0)
            .size(vk::WHOLE_SIZE)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compute::BindingKind;
    use crate::context::test_support::gpu_context_or_skip;
    use crate::live_objects::LiveObjectKind;

    #[test]
    fn size_must_be_nonzero_word_multiple_and_within_storage_range() {
        assert!(validate_size(0, 1024).is_err(), "ноль");
        assert!(validate_size(6, 1024).is_err(), "не кратно 4");
        assert!(validate_size(1028, 1024).is_err(), "больше maxStorageBufferRange");
        assert_eq!(validate_size(1024, 1024), Ok(()));
        // Плоскость f32 кадра 4096×2304 помещается в лимит Adreno 740 (128 МиБ).
        assert_eq!(validate_size(4096 * 2304 * 4, 134_217_728), Ok(()));
    }

    #[test]
    fn clear_size_must_be_nonzero_word_multiple_within_buffer() {
        assert!(validate_clear_size(0, 64).is_err(), "ноль");
        assert!(validate_clear_size(6, 64).is_err(), "не кратно 4");
        assert!(validate_clear_size(68, 64).is_err(), "больше буфера");
        assert_eq!(validate_clear_size(4, 64), Ok(()));
        assert_eq!(validate_clear_size(64, 64), Ok(()));
    }

    #[test]
    fn clear_zeroes_only_the_requested_prefix_and_rejects_bad_sizes_without_commands() {
        let Some(context) = gpu_context_or_skip() else { return };
        let buffer = DeviceBuffer::new(&context, 256).unwrap();
        let pattern: Vec<u8> = (0..256_u32).map(|index| (index % 250) as u8 + 1).collect();
        buffer.upload_blocking(&pattern).unwrap();
        context
            .run_one_shot(|command_buffer| buffer.record_clear_for_compute(command_buffer, 96))
            .unwrap();
        let after = buffer.read_back_blocking().unwrap();
        assert!(after[..96].iter().all(|&byte| byte == 0), "префикс обнулён");
        assert_eq!(after[96..], pattern[96..], "хвост не тронут");
        // Плохой размер — ошибка до команд: содержимое остаётся прежним.
        context
            .run_one_shot(|command_buffer| {
                assert!(buffer.record_clear_for_compute(command_buffer, 0).is_err());
                assert!(buffer.record_clear_for_compute(command_buffer, 260).is_err());
                Ok(())
            })
            .unwrap();
        assert_eq!(buffer.read_back_blocking().unwrap(), after);
        assert_eq!(
            context.validation_report().errors,
            0,
            "{:?}",
            context.validation_report()
        );
    }

    #[test]
    fn upload_then_read_back_round_trips_and_objects_are_released() {
        let Some(context) = gpu_context_or_skip() else { return };
        let baseline = context.live_objects();
        {
            let buffer = DeviceBuffer::new(&context, 200_000).unwrap();
            assert_eq!(
                context.live_objects().count(LiveObjectKind::Buffer),
                baseline.count(LiveObjectKind::Buffer) + 1
            );
            // Больше одного куска vkCmdUpdateBuffer (64 КиБ), чтобы проверить смещения кусков.
            let bytes: Vec<u8> = (0..200_000_u32).map(|index| (index % 251) as u8).collect();
            buffer.upload_blocking(&bytes).unwrap();
            assert_eq!(buffer.read_back_blocking().unwrap(), bytes);
            assert!(buffer.upload_blocking(&[1, 2, 3]).is_err(), "не кратно 4");
            assert!(buffer.upload_blocking(&vec![0; 200_004]).is_err(), "больше буфера");
        }
        assert_eq!(context.live_objects(), baseline);
        assert_eq!(
            context.validation_report().errors,
            0,
            "{:?}",
            context.validation_report()
        );
    }

    #[test]
    fn bindings_respect_read_only_and_read_write_kinds() {
        let Some(context) = gpu_context_or_skip() else { return };
        let buffer = DeviceBuffer::new(&context, 64).unwrap();
        assert!(buffer.read_write_binding().fits(BindingKind::ReadWriteStorageBuffer));
        assert!(buffer.read_write_binding().fits(BindingKind::ReadOnlyStorageBuffer));
        assert!(buffer.read_only_binding().fits(BindingKind::ReadOnlyStorageBuffer));
        assert!(!buffer.read_only_binding().fits(BindingKind::ReadWriteStorageBuffer));
        assert!(matches!(
            buffer.read_only_binding(),
            DescriptorResource::ReadOnlyBuffer { range_bytes: 64, .. }
        ));
        let too_big = u64::from(max_storage_buffer_range(&context)) + 4;
        assert!(DeviceBuffer::new(&context, too_big).is_err());
    }
}

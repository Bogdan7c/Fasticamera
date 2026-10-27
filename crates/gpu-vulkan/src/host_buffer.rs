//! [`HostBuffer`] — буфер в памяти, видимой CPU, отображённый на всё время жизни (persistently mapped).
//!
//! Два сценария сессии 07:
//! * **параметры кадра** (`UniformParameters`): CPU пишет `IspParams` перед сабмитом, шейдер читает
//!   как uniform-блок; буфер на слот кадра, чтобы не переписать то, что GPU ещё читает;
//! * **статистика** (`ReadWriteStorage`): шейдер копит суммы по плиткам, CPU читает их после fence слота.
//!
//! # Когерентность памяти
//! Память выделяется с `HOST_VISIBLE | HOST_COHERENT` (спецификация гарантирует хотя бы один такой
//! тип), поэтому ручные `vkFlushMappedMemoryRanges`/`vkInvalidateMappedMemoryRanges` не нужны:
//! * CPU → GPU: записи хоста до `vkQueueSubmit*` видимы командам сабмита — сабмит сам выполняет
//!   «операцию домена хоста» (спецификация, «Host Write Ordering Guarantees»);
//! * GPU → CPU: шейдер пишет, затем барьер `COMPUTE_SHADER/SHADER_STORAGE_WRITE → HOST/HOST_READ`
//!   ([`HostBuffer::record_compute_write_to_host_read`]), затем CPU дожидается fence сабмита и только
//!   потом читает ([`HostBuffer::read_bytes`]). Без fence чтение вернёт недописанные данные.
//!
//! Чтение и запись — копированием с проверкой границ (`InvalidArgument` при выходе за размер), сырых
//! указателей наружу нет.

use std::ptr::NonNull;

use ash::vk;

use crate::compute::DescriptorResource;
use crate::context::{GpuContext, MemoryLocation};
use crate::error::{GpuError, run_step};
use crate::live_objects::{OwnedBuffer, OwnedDeviceMemory};

/// Для чего буфер.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostBufferUsage {
    /// Uniform-параметры (`UNIFORM_BUFFER`).
    UniformParameters,
    /// Storage-буфер чтения-записи (`STORAGE_BUFFER`), обнуляемый командой ([`HostBuffer::record_clear_for_compute`])
    /// и пригодный как приёмник копирования (`TRANSFER_DST`, readback).
    ReadWriteStorage,
}

impl HostBufferUsage {
    fn vk_usage(self) -> vk::BufferUsageFlags {
        match self {
            Self::UniformParameters => vk::BufferUsageFlags::UNIFORM_BUFFER,
            Self::ReadWriteStorage => vk::BufferUsageFlags::STORAGE_BUFFER | vk::BufferUsageFlags::TRANSFER_DST,
        }
    }
}

/// Буфер + память + постоянное отображение. Порядок полей = порядок уничтожения: буфер → память
/// (отображение снимается в `Drop` до освобождения памяти).
pub struct HostBuffer {
    buffer: OwnedBuffer,
    memory: OwnedDeviceMemory,
    /// Начало отображения (`size_bytes` байт), валидно до `Drop`.
    mapped: NonNull<u8>,
    size_bytes: u64,
    usage: HostBufferUsage,
    context: GpuContext,
}

// SAFETY: отображение принадлежит только этому буферу; запись идёт через `&mut self`, чтение — через
// `&self` копированием, поэтому гонок данных на стороне CPU нет. Хэндлы Vulkan можно использовать из
// любого потока (внешняя синхронизация нужна только для очереди — она под замком контекста).
unsafe impl Send for HostBuffer {}
// SAFETY: см. выше — через `&self` доступно только чтение копированием.
unsafe impl Sync for HostBuffer {}

impl std::fmt::Debug for HostBuffer {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "HostBuffer({:?}, {} байт)", self.usage, self.size_bytes)
    }
}

impl HostBuffer {
    /// Создаёт буфер `size_bytes` (> 0) и отображает его. Содержимое обнулено.
    pub fn new(context: &GpuContext, size_bytes: u64, usage: HostBufferUsage) -> Result<Self, GpuError> {
        if size_bytes == 0 {
            return Err(GpuError::InvalidArgument {
                what: "HostBuffer нулевого размера".to_owned(),
            });
        }
        let length = usize::try_from(size_bytes).map_err(|_| GpuError::InvalidArgument {
            what: format!("HostBuffer {size_bytes} байт не помещается в адресное пространство"),
        })?;
        let device = context.device();
        let info = vk::BufferCreateInfo::default()
            .size(size_bytes)
            .usage(usage.vk_usage())
            .sharing_mode(vk::SharingMode::EXCLUSIVE);
        // SAFETY: устройство валидно; info — живая локальная структура.
        let buffer = OwnedBuffer::from_raw(
            context,
            run_step(
                unsafe { device.create_buffer(&info, None) },
                "vkCreateBuffer(HostBuffer)",
            )?,
        );
        // SAFETY: буфер только что создан этим устройством.
        let requirements = unsafe { device.get_buffer_memory_requirements(buffer.handle()) };
        let memory = context.allocate_memory(requirements, MemoryLocation::HostVisibleCoherent, "HostBuffer")?;
        // SAFETY: память подобрана по требованиям буфера и ещё не привязана; смещение 0.
        run_step(
            unsafe { device.bind_buffer_memory(buffer.handle(), memory.handle(), 0) },
            "vkBindBufferMemory(HostBuffer)",
        )?;
        // SAFETY: память host-visible, ещё не отображена; отображаем всё выделение (WHOLE_SIZE).
        let address = run_step(
            unsafe { device.map_memory(memory.handle(), 0, vk::WHOLE_SIZE, vk::MemoryMapFlags::empty()) },
            "vkMapMemory(HostBuffer)",
        )?;
        let mapped = NonNull::new(address.cast::<u8>()).ok_or(GpuError::InvalidState {
            what: "vkMapMemory вернул нулевой указатель".to_owned(),
        })?;
        // SAFETY: отображение длиной ≥ size_bytes (requirements.size ≥ size), GPU буфер ещё не видел.
        unsafe { std::ptr::write_bytes(mapped.as_ptr(), 0, length) };
        Ok(Self {
            buffer,
            memory,
            mapped,
            size_bytes,
            usage,
            context: context.clone(),
        })
    }

    /// Размер, байт.
    pub fn size_bytes(&self) -> u64 {
        self.size_bytes
    }

    /// Назначение буфера.
    pub fn usage(&self) -> HostBufferUsage {
        self.usage
    }

    /// Копирует `bytes` в буфер начиная с `offset`. Выход за размер — `InvalidArgument`, буфер не меняется.
    ///
    /// Контракт: GPU сейчас не читает этот буфер (fence слота, которому он принадлежит, сигнален).
    pub fn write_bytes(&mut self, offset: u64, bytes: &[u8]) -> Result<(), GpuError> {
        let start = self.checked_range(offset, bytes.len())?;
        // SAFETY: диапазон start..start+len проверен внутри отображения; источник — чужой срез, не пересекается.
        unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), self.mapped.as_ptr().add(start), bytes.len()) };
        Ok(())
    }

    /// Читает `length` байт с `offset`. Выход за размер — `InvalidArgument`.
    ///
    /// Контракт: запись GPU завершена (fence сабмита дождались) и сделана видимой хосту барьером
    /// [`Self::record_compute_write_to_host_read`]; иначе прочитаются недописанные данные.
    pub fn read_bytes(&self, offset: u64, length: usize) -> Result<Vec<u8>, GpuError> {
        let start = self.checked_range(offset, length)?;
        let mut bytes = vec![0_u8; length];
        // Копируем сырым указателем, НЕ создавая `&[u8]` над отображённой памятью: её может писать
        // другой агент (GPU), а Rust-ссылка обещала бы неизменность на всё время своей жизни — нарушение
        // контракта вызывающего (чтение до fence) превратилось бы в UB из безопасной функции. Сырое
        // копирование в худшем случае вернёт недописанные байты, о чём и говорит контракт.
        // SAFETY: start..start+length проверен внутри отображения (живого до `Drop`); приёмник — свой
        // только что выделенный вектор той же длины, области не пересекаются; `u8` не имеет
        // недопустимых значений, поэтому любые байты источника корректны.
        unsafe { std::ptr::copy_nonoverlapping(self.mapped.as_ptr().add(start), bytes.as_mut_ptr(), length) };
        Ok(bytes)
    }

    /// Ресурс для привязки: uniform-буфер или storage чтения-записи (весь буфер).
    pub fn binding(&self) -> DescriptorResource {
        match self.usage {
            HostBufferUsage::UniformParameters => DescriptorResource::UniformBuffer {
                buffer: self.buffer.handle(),
                range_bytes: self.size_bytes,
            },
            HostBufferUsage::ReadWriteStorage => DescriptorResource::ReadWriteBuffer {
                buffer: self.buffer.handle(),
                range_bytes: self.size_bytes,
            },
        }
    }

    /// Обнуляет буфер командой GPU (`vkCmdFillBuffer`) и делает нули видимыми compute-шейдеру
    /// (накопление статистики начинается с нуля). Только для `ReadWriteStorage`.
    ///
    /// Первая область барьера перед заливкой — прошлые чтения хоста не в счёт (их порядок задаёт fence),
    /// поэтому ждём только прежние compute-записи этой очереди (запись после записи).
    pub fn record_clear_for_compute(&self, command_buffer: vk::CommandBuffer) -> Result<(), GpuError> {
        if self.usage != HostBufferUsage::ReadWriteStorage {
            return Err(GpuError::InvalidState {
                what: format!(
                    "обнулять командой можно только ReadWriteStorage, а буфер {:?}",
                    self.usage
                ),
            });
        }
        let device = self.context.device();
        let before_fill = self.barrier(
            (
                vk::PipelineStageFlags2::COMPUTE_SHADER,
                vk::AccessFlags2::SHADER_STORAGE_WRITE,
            ),
            (vk::PipelineStageFlags2::CLEAR, vk::AccessFlags2::TRANSFER_WRITE),
        );
        let after_fill = self.barrier(
            (vk::PipelineStageFlags2::CLEAR, vk::AccessFlags2::TRANSFER_WRITE),
            (
                vk::PipelineStageFlags2::COMPUTE_SHADER,
                vk::AccessFlags2::SHADER_STORAGE_READ | vk::AccessFlags2::SHADER_STORAGE_WRITE,
            ),
        );
        // SAFETY: командный буфер в состоянии записи (контракт вызывающего), вне рендеринга; буфер с
        // usage TRANSFER_DST; размер заливки — весь буфер (WHOLE_SIZE).
        unsafe {
            device.cmd_pipeline_barrier2(
                command_buffer,
                &vk::DependencyInfo::default().buffer_memory_barriers(&[before_fill]),
            );
            device.cmd_fill_buffer(command_buffer, self.buffer.handle(), 0, vk::WHOLE_SIZE, 0);
            device.cmd_pipeline_barrier2(
                command_buffer,
                &vk::DependencyInfo::default().buffer_memory_barriers(&[after_fill]),
            );
        }
        Ok(())
    }

    /// Делает записи compute-шейдера видимыми хосту (перед чтением после fence).
    pub fn record_compute_write_to_host_read(&self, command_buffer: vk::CommandBuffer) {
        let barrier = self.barrier(
            (
                vk::PipelineStageFlags2::COMPUTE_SHADER,
                vk::AccessFlags2::SHADER_STORAGE_WRITE,
            ),
            (vk::PipelineStageFlags2::HOST, vk::AccessFlags2::HOST_READ),
        );
        // SAFETY: командный буфер в состоянии записи (контракт вызывающего); буфер принадлежит устройству.
        unsafe {
            self.context.device().cmd_pipeline_barrier2(
                command_buffer,
                &vk::DependencyInfo::default().buffer_memory_barriers(&[barrier]),
            );
        }
    }

    /// Сам `VkBuffer` (для копирования внутри crate: readback storage-картинки).
    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn buffer_handle(&self) -> vk::Buffer {
        self.buffer.handle()
    }

    /// Барьер всего буфера между двумя областями `(стадия, доступ)`.
    fn barrier(
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

    /// Проверяет диапазон `offset..offset+length` и возвращает начало как `usize`.
    fn checked_range(&self, offset: u64, length: usize) -> Result<usize, GpuError> {
        let end = u64::try_from(length).ok().and_then(|length| offset.checked_add(length));
        match end {
            Some(end) if end <= self.size_bytes => usize::try_from(offset).map_err(|_| GpuError::InvalidArgument {
                what: format!("смещение {offset} не помещается в usize"),
            }),
            _ => Err(GpuError::InvalidArgument {
                what: format!("диапазон {offset}+{length} байт вне буфера {} байт", self.size_bytes),
            }),
        }
    }
}

impl Drop for HostBuffer {
    fn drop(&mut self) {
        // SAFETY: память отображена в `new` и с тех пор не снималась; GPU закончил с буфером (инвариант
        // владельца, как у всех `Owned*`). Память освободится после — полем `memory`.
        unsafe { self.context.device().unmap_memory(self.memory.handle()) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::test_support::gpu_context_or_skip;
    use crate::live_objects::LiveObjectKind;

    #[test]
    fn writes_and_reads_are_bounds_checked_and_do_not_touch_neighbouring_bytes() {
        let Some(context) = gpu_context_or_skip() else { return };
        let baseline = context.live_objects();
        {
            let mut buffer = HostBuffer::new(&context, 16, HostBufferUsage::UniformParameters).unwrap();
            assert_eq!(buffer.read_bytes(0, 16).unwrap(), vec![0; 16], "новый буфер обнулён");
            buffer.write_bytes(4, &[1, 2, 3, 4]).unwrap();
            assert_eq!(
                buffer.read_bytes(0, 16).unwrap(),
                vec![0, 0, 0, 0, 1, 2, 3, 4, 0, 0, 0, 0, 0, 0, 0, 0]
            );
            // Выход за границу — ошибка и буфер не меняется.
            let error = buffer.write_bytes(14, &[9, 9, 9]).unwrap_err();
            assert!(matches!(error, GpuError::InvalidArgument { .. }), "{error:?}");
            assert!(buffer.read_bytes(15, 2).is_err());
            assert!(buffer.read_bytes(u64::MAX, 1).is_err(), "переполнение смещения");
            assert_eq!(buffer.read_bytes(12, 4).unwrap(), vec![0; 4]);
            assert_eq!(context.live_objects().count(LiveObjectKind::Buffer), 1);
            assert_eq!(context.live_objects().count(LiveObjectKind::DeviceMemory), 1);
        }
        assert_eq!(context.live_objects(), baseline);
    }

    #[test]
    fn zero_size_is_rejected_and_uniform_buffer_cannot_be_cleared_by_gpu() {
        let Some(context) = gpu_context_or_skip() else { return };
        let baseline = context.live_objects();
        assert!(HostBuffer::new(&context, 0, HostBufferUsage::ReadWriteStorage).is_err());
        assert_eq!(context.live_objects(), baseline);
        let uniform = HostBuffer::new(&context, 8, HostBufferUsage::UniformParameters).unwrap();
        let error = context
            .run_one_shot(|command_buffer| uniform.record_clear_for_compute(command_buffer))
            .unwrap_err();
        assert!(matches!(error, GpuError::InvalidState { .. }), "{error:?}");
        assert!(matches!(
            uniform.binding(),
            DescriptorResource::UniformBuffer { range_bytes: 8, .. }
        ));
    }

    #[test]
    fn gpu_clear_zeroes_the_whole_buffer() {
        let Some(context) = gpu_context_or_skip() else { return };
        let mut buffer = HostBuffer::new(&context, 12, HostBufferUsage::ReadWriteStorage).unwrap();
        buffer.write_bytes(0, &[0xAB; 12]).unwrap();
        context
            .run_one_shot(|command_buffer| {
                buffer.record_clear_for_compute(command_buffer)?;
                buffer.record_compute_write_to_host_read(command_buffer);
                Ok(())
            })
            .unwrap();
        assert_eq!(buffer.read_bytes(0, 12).unwrap(), vec![0; 12]);
        assert_eq!(
            context.validation_report().errors,
            0,
            "{:?}",
            context.validation_report()
        );
    }
}

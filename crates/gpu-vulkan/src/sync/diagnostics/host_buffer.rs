//! Маленький host-visible буфер для диагностики: одно 32-битное значение, видимое и GPU, и CPU.

use ash::vk;

use crate::context::{GpuContext, MemoryLocation};
use crate::error::{GpuError, run_step};
use crate::live_objects::{OwnedBuffer, OwnedDeviceMemory};

/// Размер буфера: одно `u32` (минимум, который имеет смысл для storage-буфера).
const BUFFER_SIZE_BYTES: vk::DeviceSize = 4;

/// Storage-буфер в когерентной host-visible памяти (ручной flush/invalidate не нужен).
pub(super) struct HostBuffer {
    // Порядок полей: буфер уничтожается раньше памяти, к которой привязан.
    buffer: OwnedBuffer,
    memory: OwnedDeviceMemory,
    context: GpuContext,
}

impl HostBuffer {
    pub(super) fn new(context: &GpuContext) -> Result<Self, GpuError> {
        let device = context.device();
        let create_info = vk::BufferCreateInfo::default()
            .size(BUFFER_SIZE_BYTES)
            .usage(vk::BufferUsageFlags::STORAGE_BUFFER)
            .sharing_mode(vk::SharingMode::EXCLUSIVE);
        // SAFETY: устройство валидно; create_info — локальная структура.
        let buffer = OwnedBuffer::from_raw(
            context,
            run_step(unsafe { device.create_buffer(&create_info, None) }, "vkCreateBuffer")?,
        );
        // SAFETY: буфер только что создан этим устройством.
        let requirements = unsafe { device.get_buffer_memory_requirements(buffer.handle()) };
        let memory = context.allocate_memory(requirements, MemoryLocation::HostVisibleCoherent, "буфер диагностики")?;
        // SAFETY: память подходит по требованиям и ещё не привязана; смещение 0.
        run_step(
            unsafe { device.bind_buffer_memory(buffer.handle(), memory.handle(), 0) },
            "vkBindBufferMemory",
        )?;
        Ok(Self {
            buffer,
            memory,
            context: context.clone(),
        })
    }

    pub(super) fn handle(&self) -> vk::Buffer {
        self.buffer.handle()
    }

    /// Размер, который нужно указывать в дескрипторе.
    pub(super) fn size(&self) -> vk::DeviceSize {
        BUFFER_SIZE_BYTES
    }

    /// Записывает значение с CPU. Вызывать только когда GPU не использует буфер.
    pub(super) fn write(&self, value: u32) -> Result<(), GpuError> {
        let device = self.context.device();
        // SAFETY: память host-visible и не отображена; GPU буфер сейчас не использует.
        let mapped = run_step(
            unsafe { device.map_memory(self.memory.handle(), 0, vk::WHOLE_SIZE, vk::MemoryMapFlags::empty()) },
            "vkMapMemory",
        )?;
        // SAFETY: отображено не менее 4 байт, указатель выровнен (отображение выровнено по странице).
        unsafe { mapped.cast::<u32>().write_volatile(value) };
        // SAFETY: память отображена выше.
        unsafe { device.unmap_memory(self.memory.handle()) };
        Ok(())
    }

    /// Читает значение на CPU. Вызывать только после завершения работы GPU (fence).
    pub(super) fn read(&self) -> Result<u32, GpuError> {
        let device = self.context.device();
        // SAFETY: память host-visible и не отображена.
        let mapped = run_step(
            unsafe { device.map_memory(self.memory.handle(), 0, vk::WHOLE_SIZE, vk::MemoryMapFlags::empty()) },
            "vkMapMemory",
        )?;
        // SAFETY: отображено не менее 4 байт, указатель выровнен.
        let value = unsafe { mapped.cast::<u32>().read_volatile() };
        // SAFETY: память отображена выше.
        unsafe { device.unmap_memory(self.memory.handle()) };
        Ok(value)
    }
}

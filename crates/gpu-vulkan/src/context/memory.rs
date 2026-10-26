//! Подбор типа памяти и выделение `VkDeviceMemory` под требования ресурса.

use ash::vk;

use super::GpuContext;
use crate::error::{GpuError, run_step};
use crate::live_objects::OwnedDeviceMemory;

/// Где должна жить память ресурса.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MemoryLocation {
    /// Память GPU (картинки-цели, быстрые буферы).
    DeviceLocal,
    /// Память, видимая CPU и когерентная (readback, staging). Спецификация гарантирует
    /// хотя бы один тип с `HOST_VISIBLE | HOST_COHERENT`, поэтому ручной invalidate не нужен.
    HostVisibleCoherent,
}

impl MemoryLocation {
    fn required_flags(self) -> vk::MemoryPropertyFlags {
        match self {
            MemoryLocation::DeviceLocal => vk::MemoryPropertyFlags::DEVICE_LOCAL,
            MemoryLocation::HostVisibleCoherent => {
                vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT
            }
        }
    }
}

/// Чистый поиск: первый тип памяти из `memory_type_bits` с нужными флагами.
pub(crate) fn find_memory_type_index(
    properties: &vk::PhysicalDeviceMemoryProperties,
    memory_type_bits: u32,
    required: vk::MemoryPropertyFlags,
) -> Option<u32> {
    (0..properties.memory_type_count).find(|index| {
        let allowed = memory_type_bits & (1 << index) != 0;
        allowed
            && properties.memory_types[*index as usize]
                .property_flags
                .contains(required)
    })
}

impl GpuContext {
    /// Выделяет память под требования ресурса. `purpose` попадает в ошибку, если типа нет.
    pub(crate) fn allocate_memory(
        &self,
        requirements: vk::MemoryRequirements,
        location: MemoryLocation,
        purpose: &'static str,
    ) -> Result<OwnedDeviceMemory, GpuError> {
        let memory_type_index = find_memory_type_index(
            &self.inner.details.memory_properties,
            requirements.memory_type_bits,
            location.required_flags(),
        )
        .ok_or(GpuError::NoMemoryType { purpose })?;
        let allocate_info = vk::MemoryAllocateInfo::default()
            .allocation_size(requirements.size)
            .memory_type_index(memory_type_index);
        // SAFETY: устройство валидно; allocate_info — живая локальная структура.
        let memory = run_step(
            unsafe { self.device().allocate_memory(&allocate_info, None) },
            "vkAllocateMemory",
        )?;
        Ok(OwnedDeviceMemory::from_raw(self, memory))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn properties(types: &[vk::MemoryPropertyFlags]) -> vk::PhysicalDeviceMemoryProperties {
        let mut properties = vk::PhysicalDeviceMemoryProperties {
            memory_type_count: types.len() as u32,
            ..Default::default()
        };
        for (slot, flags) in properties.memory_types.iter_mut().zip(types) {
            slot.property_flags = *flags;
        }
        properties
    }

    #[test]
    fn picks_first_allowed_type_with_all_required_flags() {
        let host = vk::MemoryPropertyFlags::HOST_VISIBLE;
        let host_coherent = vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT;
        let memory = properties(&[vk::MemoryPropertyFlags::DEVICE_LOCAL, host, host_coherent]);
        // Тип 1 подходит по битам, но не когерентен; тип 2 — подходит.
        assert_eq!(find_memory_type_index(&memory, 0b111, host_coherent), Some(2));
        // Тип 2 запрещён маской ресурса → подходящего нет.
        assert_eq!(find_memory_type_index(&memory, 0b011, host_coherent), None);
        assert_eq!(
            find_memory_type_index(&memory, 0b111, vk::MemoryPropertyFlags::DEVICE_LOCAL),
            Some(0)
        );
    }
}

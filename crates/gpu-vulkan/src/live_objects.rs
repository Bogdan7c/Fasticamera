//! Учёт живых объектов Vulkan и RAII-обёртки `Owned*`.
//!
//! Идея: каждый объект Vulkan, созданный этим crate-ом, живёт внутри обёртки, которая
//! * хранит клон [`GpuContext`] — устройство гарантированно живёт дольше объекта;
//! * хранит [`LiveObjectGuard`] — счётчик по виду объекта растёт при создании и падает в `Drop`;
//! * уничтожает объект в `Drop` — вручную `destroy_*` не вызывает никто.
//!
//! Тест «20 циклов без роста» тривиален: снимок [`LiveObjectSnapshot`] до и после сравнивается `==`.
//! Намеренно «забытый» объект (`mem::forget`) виден в снимке — это доказывает, что счётчик считает.

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use ash::vk;

use crate::context::GpuContext;

/// Виды объектов Vulkan, которые считает crate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LiveObjectKind {
    /// `VkImage`.
    Image,
    /// `VkImageView`.
    ImageView,
    /// `VkBuffer`.
    Buffer,
    /// `VkDeviceMemory`.
    DeviceMemory,
    /// `VkSemaphore` (бинарный или timeline).
    Semaphore,
    /// `VkFence`.
    Fence,
    /// `VkCommandPool` (буферы команд освобождаются вместе с пулом).
    CommandPool,
    /// `VkPipeline`.
    Pipeline,
    /// `VkPipelineLayout`.
    PipelineLayout,
    /// `VkDescriptorSetLayout`.
    DescriptorSetLayout,
    /// `VkDescriptorPool`.
    DescriptorPool,
    /// `VkShaderModule`.
    ShaderModule,
    /// `VkSampler`.
    Sampler,
    /// `VkSwapchainKHR` (оболочка для пакета swapchain).
    Swapchain,
    /// `VkSurfaceKHR` (оболочка для пакета swapchain).
    Surface,
}

impl LiveObjectKind {
    /// Все виды в фиксированном порядке (порядок задаёт индекс в счётчиках).
    pub const ALL: [LiveObjectKind; 15] = [
        LiveObjectKind::Image,
        LiveObjectKind::ImageView,
        LiveObjectKind::Buffer,
        LiveObjectKind::DeviceMemory,
        LiveObjectKind::Semaphore,
        LiveObjectKind::Fence,
        LiveObjectKind::CommandPool,
        LiveObjectKind::Pipeline,
        LiveObjectKind::PipelineLayout,
        LiveObjectKind::DescriptorSetLayout,
        LiveObjectKind::DescriptorPool,
        LiveObjectKind::ShaderModule,
        LiveObjectKind::Sampler,
        LiveObjectKind::Swapchain,
        LiveObjectKind::Surface,
    ];

    /// Индекс вида в массиве счётчиков.
    fn index(self) -> usize {
        // Порядок `ALL` совпадает с порядком объявления; поиск по массиву исключает рассинхрон.
        Self::ALL.iter().position(|kind| *kind == self).unwrap_or(0)
    }
}

/// Атомарные счётчики живых объектов по видам. Один набор на контекст.
#[derive(Debug, Default)]
pub struct LiveObjectCounters {
    counts: [AtomicUsize; LiveObjectKind::ALL.len()],
}

impl LiveObjectCounters {
    /// Увеличивает счётчик вида; возвращает охранника, который уменьшит его в `Drop`.
    pub(crate) fn register(self: &Arc<Self>, kind: LiveObjectKind) -> LiveObjectGuard {
        self.counts[kind.index()].fetch_add(1, Ordering::Relaxed);
        LiveObjectGuard {
            counters: Arc::clone(self),
            kind,
        }
    }

    /// Снимок текущих значений всех счётчиков.
    pub fn snapshot(&self) -> LiveObjectSnapshot {
        let mut counts = [0usize; LiveObjectKind::ALL.len()];
        for (slot, counter) in counts.iter_mut().zip(self.counts.iter()) {
            *slot = counter.load(Ordering::Relaxed);
        }
        LiveObjectSnapshot { counts }
    }
}

/// Охранник одного объекта: пока жив, объект учтён в счётчике своего вида.
#[derive(Debug)]
pub struct LiveObjectGuard {
    counters: Arc<LiveObjectCounters>,
    kind: LiveObjectKind,
}

impl Drop for LiveObjectGuard {
    fn drop(&mut self) {
        self.counters.counts[self.kind.index()].fetch_sub(1, Ordering::Relaxed);
    }
}

/// Снимок числа живых объектов по видам. Сравнивается `==`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LiveObjectSnapshot {
    counts: [usize; LiveObjectKind::ALL.len()],
}

impl LiveObjectSnapshot {
    /// Сколько живых объектов данного вида.
    pub fn count(&self, kind: LiveObjectKind) -> usize {
        self.counts[kind.index()]
    }

    /// Сколько живых объектов всех видов.
    pub fn total(&self) -> usize {
        self.counts.iter().sum()
    }

    /// Виды, в которых число объектов отличается от `baseline`: `(вид, было, стало)`.
    /// Пустой результат означает «утечек и роста нет».
    pub fn differences_from(&self, baseline: &LiveObjectSnapshot) -> Vec<(LiveObjectKind, usize, usize)> {
        LiveObjectKind::ALL
            .iter()
            .filter(|kind| self.count(**kind) != baseline.count(**kind))
            .map(|kind| (*kind, baseline.count(*kind), self.count(*kind)))
            .collect()
    }
}

impl fmt::Display for LiveObjectSnapshot {
    /// Печатает только ненулевые виды: `Image=2 Buffer=1`; пустой снимок — `пусто`.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut any = false;
        for kind in LiveObjectKind::ALL {
            let count = self.count(kind);
            if count > 0 {
                if any {
                    formatter.write_str(" ")?;
                }
                write!(formatter, "{kind:?}={count}")?;
                any = true;
            }
        }
        if !any {
            formatter.write_str("пусто")?;
        }
        Ok(())
    }
}

/// Генерирует RAII-обёртку над дескриптором, который уничтожается методом `Device`.
///
/// Конструктор `from_raw` принимает УЖЕ созданный дескриптор и берёт его во владение:
/// после этого вызывающий не должен его уничтожать.
macro_rules! owned_device_object {
    ($(#[$meta:meta])* $name:ident, $handle:ty, $kind:ident, $destroy:ident) => {
        $(#[$meta])*
        pub struct $name {
            handle: $handle,
            // Порядок полей важен: охранник и контекст освобождаются после `Drop::drop`.
            _guard: LiveObjectGuard,
            context: GpuContext,
        }

        impl $name {
            /// Берёт во владение уже созданный дескриптор и регистрирует его в счётчике.
            // allow, а не expect: макрос порождает метод для каждого вида объекта, а часть видов
            // создаётся только своими помощниками (`create_*`); expect сработал бы не у всех видов.
            #[allow(dead_code, reason = "метод порождается для всех видов, часть видов его не вызывает")]
            pub(crate) fn from_raw(context: &GpuContext, handle: $handle) -> Self {
                Self {
                    handle,
                    _guard: context.live_object_counters().register(LiveObjectKind::$kind),
                    context: context.clone(),
                }
            }

            /// Сырой дескриптор Vulkan (владение остаётся у обёртки).
            pub fn handle(&self) -> $handle {
                self.handle
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(formatter, "{}({:?})", stringify!($name), self.handle)
            }
        }

        impl Drop for $name {
            fn drop(&mut self) {
                // SAFETY: дескриптор создан этим же устройством и принадлежит обёртке; обёртка
                // единственный владелец, а контекст (и устройство) живёт, пока жива обёртка.
                // Что GPU закончил с объектом, обязан обеспечить код, владеющий обёрткой
                // (fence/wait_idle до drop) — это инвариант всего crate.
                unsafe { self.context.device().$destroy(self.handle, None) };
            }
        }
    };
}

owned_device_object!(
    /// Владеемый `VkImage`.
    OwnedImage, vk::Image, Image, destroy_image
);
owned_device_object!(
    /// Владеемый `VkImageView`.
    OwnedImageView, vk::ImageView, ImageView, destroy_image_view
);
owned_device_object!(
    /// Владеемый `VkBuffer`.
    OwnedBuffer, vk::Buffer, Buffer, destroy_buffer
);
owned_device_object!(
    /// Владеемая `VkDeviceMemory` (освобождается через `vkFreeMemory`).
    OwnedDeviceMemory, vk::DeviceMemory, DeviceMemory, free_memory
);
owned_device_object!(
    /// Владеемый `VkSemaphore`.
    OwnedSemaphore, vk::Semaphore, Semaphore, destroy_semaphore
);
owned_device_object!(
    /// Владеемый `VkFence`.
    OwnedFence, vk::Fence, Fence, destroy_fence
);
owned_device_object!(
    /// Владеемый `VkCommandPool`; командные буферы из него освобождаются вместе с пулом.
    OwnedCommandPool, vk::CommandPool, CommandPool, destroy_command_pool
);
owned_device_object!(
    /// Владеемый `VkPipeline`.
    OwnedPipeline, vk::Pipeline, Pipeline, destroy_pipeline
);
owned_device_object!(
    /// Владеемый `VkPipelineLayout`.
    OwnedPipelineLayout, vk::PipelineLayout, PipelineLayout, destroy_pipeline_layout
);
owned_device_object!(
    /// Владеемый `VkDescriptorSetLayout`.
    OwnedDescriptorSetLayout, vk::DescriptorSetLayout, DescriptorSetLayout, destroy_descriptor_set_layout
);
owned_device_object!(
    /// Владеемый `VkDescriptorPool`; наборы дескрипторов освобождаются вместе с пулом.
    OwnedDescriptorPool, vk::DescriptorPool, DescriptorPool, destroy_descriptor_pool
);
owned_device_object!(
    /// Владеемый `VkShaderModule`.
    OwnedShaderModule, vk::ShaderModule, ShaderModule, destroy_shader_module
);
owned_device_object!(
    /// Владеемый `VkSampler`.
    OwnedSampler, vk::Sampler, Sampler, destroy_sampler
);

/// Владеемый `VkSwapchainKHR`. Уничтожается загрузчиком `VK_KHR_swapchain`.
///
/// Перед `drop` владелец обязан дождаться GPU (`wait_idle` или fence'ы кадров): Vulkan не
/// позволяет уничтожать swapchain, картинки которого ещё используются.
pub struct OwnedSwapchain {
    handle: vk::SwapchainKHR,
    _guard: LiveObjectGuard,
    context: GpuContext,
}

impl OwnedSwapchain {
    /// Берёт во владение созданный swapchain. Расширение swapchain в профиле должно быть включено
    /// (иначе создать swapchain было бы нечем) — иначе возвращается ошибка и дескриптор не берётся.
    pub(crate) fn from_raw(context: &GpuContext, handle: vk::SwapchainKHR) -> Result<Self, crate::error::GpuError> {
        // Проверка до регистрации: при ошибке счётчик не меняется.
        context.swapchain_loader()?;
        Ok(Self {
            handle,
            _guard: context.live_object_counters().register(LiveObjectKind::Swapchain),
            context: context.clone(),
        })
    }

    /// Сырой дескриптор.
    pub fn handle(&self) -> vk::SwapchainKHR {
        self.handle
    }
}

impl fmt::Debug for OwnedSwapchain {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "OwnedSwapchain({:?})", self.handle)
    }
}

impl Drop for OwnedSwapchain {
    fn drop(&mut self) {
        match self.context.swapchain_loader() {
            // SAFETY: swapchain создан этим же загрузчиком/устройством и принадлежит обёртке.
            Ok(loader) => unsafe { loader.destroy_swapchain(self.handle, None) },
            // Недостижимо: `from_raw` проверил загрузчик. Но молча не игнорируем.
            Err(error) => tracing::error!(target: "fasticamera::gpu", %error, "swapchain не уничтожен: нет загрузчика"),
        }
    }
}

/// Владеемый `VkSurfaceKHR`. Уничтожается загрузчиком `VK_KHR_surface` (instance-уровень).
///
/// Surface нельзя уничтожать, пока жив swapchain, созданный на нём: порядок обеспечивает
/// владелец (swapchain объявляется в структуре раньше surface — поля уничтожаются по порядку).
pub struct OwnedSurface {
    handle: vk::SurfaceKHR,
    _guard: LiveObjectGuard,
    context: GpuContext,
}

impl OwnedSurface {
    /// Берёт во владение созданный surface.
    pub(crate) fn from_raw(context: &GpuContext, handle: vk::SurfaceKHR) -> Result<Self, crate::error::GpuError> {
        context.surface_loader()?;
        Ok(Self {
            handle,
            _guard: context.live_object_counters().register(LiveObjectKind::Surface),
            context: context.clone(),
        })
    }

    /// Сырой дескриптор.
    pub fn handle(&self) -> vk::SurfaceKHR {
        self.handle
    }
}

impl fmt::Debug for OwnedSurface {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "OwnedSurface({:?})", self.handle)
    }
}

impl Drop for OwnedSurface {
    fn drop(&mut self) {
        match self.context.surface_loader() {
            // SAFETY: surface создан на этом instance и принадлежит обёртке; swapchain на нём
            // владелец уже уничтожил (инвариант порядка полей).
            Ok(loader) => unsafe { loader.destroy_surface(self.handle, None) },
            Err(error) => tracing::error!(target: "fasticamera::gpu", %error, "surface не уничтожен: нет загрузчика"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counters_track_register_and_drop_per_kind() {
        let counters = Arc::new(LiveObjectCounters::default());
        let baseline = counters.snapshot();
        let first = counters.register(LiveObjectKind::Image);
        let second = counters.register(LiveObjectKind::Image);
        let third = counters.register(LiveObjectKind::Fence);
        let snapshot = counters.snapshot();
        assert_eq!(snapshot.count(LiveObjectKind::Image), 2);
        assert_eq!(snapshot.count(LiveObjectKind::Fence), 1);
        assert_eq!(snapshot.count(LiveObjectKind::Buffer), 0);
        assert_eq!(snapshot.total(), 3);
        assert_eq!(
            snapshot.differences_from(&baseline),
            vec![(LiveObjectKind::Image, 0, 2), (LiveObjectKind::Fence, 0, 1)]
        );
        drop(first);
        assert_eq!(counters.snapshot().count(LiveObjectKind::Image), 1);
        drop((second, third));
        assert_eq!(counters.snapshot(), baseline);
    }

    #[test]
    fn forgotten_guard_stays_in_snapshot() {
        let counters = Arc::new(LiveObjectCounters::default());
        std::mem::forget(counters.register(LiveObjectKind::Sampler));
        assert_eq!(counters.snapshot().count(LiveObjectKind::Sampler), 1);
        assert_ne!(counters.snapshot(), LiveObjectSnapshot::default());
    }

    #[test]
    fn display_lists_only_nonzero_kinds() {
        let counters = Arc::new(LiveObjectCounters::default());
        assert_eq!(counters.snapshot().to_string(), "пусто");
        let _image = counters.register(LiveObjectKind::Image);
        let _buffer = counters.register(LiveObjectKind::Buffer);
        assert_eq!(counters.snapshot().to_string(), "Image=1 Buffer=1");
    }

    #[test]
    fn every_kind_has_a_distinct_slot() {
        for (position, kind) in LiveObjectKind::ALL.iter().enumerate() {
            assert_eq!(kind.index(), position);
        }
    }
}

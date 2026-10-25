//! Корзина Vulkan-объектов одного кадра: всё, что создано для кадра, уничтожается
//! в `Drop` в безопасном порядке, даже при раннем выходе по ошибке.

use ash::vk;

use super::context::VulkanContext;

/// Владелец временных объектов кадра.
pub struct FrameResources<'ctx> {
    ctx: &'ctx VulkanContext,
    image_views: Vec<vk::ImageView>,
    images: Vec<vk::Image>,
    buffers: Vec<vk::Buffer>,
    memories: Vec<vk::DeviceMemory>,
    semaphores: Vec<vk::Semaphore>,
    fences: Vec<vk::Fence>,
    descriptor_pools: Vec<vk::DescriptorPool>,
    query_pools: Vec<vk::QueryPool>,
    command_buffers: Vec<vk::CommandBuffer>,
    mapped_memories: Vec<vk::DeviceMemory>,
}

impl<'ctx> FrameResources<'ctx> {
    pub fn new(ctx: &'ctx VulkanContext) -> Self {
        Self {
            ctx,
            image_views: Vec::new(),
            images: Vec::new(),
            buffers: Vec::new(),
            memories: Vec::new(),
            semaphores: Vec::new(),
            fences: Vec::new(),
            descriptor_pools: Vec::new(),
            query_pools: Vec::new(),
            command_buffers: Vec::new(),
            mapped_memories: Vec::new(),
        }
    }

    pub fn adopt_image_view(&mut self, view: vk::ImageView) {
        self.image_views.push(view);
    }
    pub fn adopt_image(&mut self, image: vk::Image) {
        self.images.push(image);
    }
    pub fn adopt_buffer(&mut self, buffer: vk::Buffer) {
        self.buffers.push(buffer);
    }
    pub fn adopt_memory(&mut self, memory: vk::DeviceMemory) {
        self.memories.push(memory);
    }
    pub fn adopt_semaphore(&mut self, semaphore: vk::Semaphore) {
        self.semaphores.push(semaphore);
    }
    pub fn adopt_fence(&mut self, fence: vk::Fence) {
        self.fences.push(fence);
    }
    pub fn adopt_descriptor_pool(&mut self, pool: vk::DescriptorPool) {
        self.descriptor_pools.push(pool);
    }
    pub fn adopt_query_pool(&mut self, pool: vk::QueryPool) {
        self.query_pools.push(pool);
    }
    pub fn adopt_command_buffer(&mut self, buffer: vk::CommandBuffer) {
        self.command_buffers.push(buffer);
    }
    /// Запоминает память, которую нужно `unmap` перед освобождением.
    pub fn adopt_mapped_memory(&mut self, memory: vk::DeviceMemory) {
        self.mapped_memories.push(memory);
    }
}

impl Drop for FrameResources<'_> {
    fn drop(&mut self) {
        let device = &self.ctx.device;
        // SAFETY: все объекты созданы на этом device и принадлежат только этой корзине.
        // Сначала ждём очередь: после submit GPU мог ещё читать эти объекты.
        unsafe {
            if let Err(error) = device.queue_wait_idle(self.ctx.queue) {
                log::warn!("queue_wait_idle перед очисткой кадра: {error:?}");
            }
            for memory in self.mapped_memories.drain(..) {
                device.unmap_memory(memory);
            }
            if !self.command_buffers.is_empty() {
                device.free_command_buffers(self.ctx.command_pool, &self.command_buffers);
            }
            for pool in self.query_pools.drain(..) {
                device.destroy_query_pool(pool, None);
            }
            for pool in self.descriptor_pools.drain(..) {
                device.destroy_descriptor_pool(pool, None);
            }
            for fence in self.fences.drain(..) {
                device.destroy_fence(fence, None);
            }
            for semaphore in self.semaphores.drain(..) {
                device.destroy_semaphore(semaphore, None);
            }
            for view in self.image_views.drain(..) {
                device.destroy_image_view(view, None);
            }
            for image in self.images.drain(..) {
                device.destroy_image(image, None);
            }
            for buffer in self.buffers.drain(..) {
                device.destroy_buffer(buffer, None);
            }
            for memory in self.memories.drain(..) {
                device.free_memory(memory, None);
            }
        }
    }
}

//! Swapchain поверх surface кодека и все объекты, нужные для рендера в него:
//! command pool/buffers, семафоры и fence'ы. Владелец — [`Rig`]; всё освобождается в `Drop`.

use ash::{Device, khr, vk};

use super::instance::{PresentDevice, SurfaceInstance};
use super::policy::{RenderParams, choose_image_count, select_surface_format};
use super::report::SwapchainReport;
use super::surface_info::{SurfaceQuery, format_entry, to_raw};
use crate::vulkan::vk_step;

/// Сколько кадров одновременно «в полёте» (на CPU-стороне).
pub const FRAMES_IN_FLIGHT: usize = 2;

/// Набор ресурсов для кадра: свои cmd buffer, семафор получения картинки и fence.
pub struct FrameSlot {
    pub command_buffer: vk::CommandBuffer,
    pub image_available: vk::Semaphore,
    pub in_flight: vk::Fence,
}

/// Swapchain + ресурсы рендера. Поля заполняются по мере создания; `Drop` убирает то, что успело создаться.
pub struct Rig {
    pub device: Device,
    pub swapchain_loader: khr::swapchain::Device,
    pub swapchain: vk::SwapchainKHR,
    pub images: Vec<vk::Image>,
    /// Семафор «рендер готов» — по одному на картинку swapchain (его ждёт present).
    pub render_finished: Vec<vk::Semaphore>,
    pub slots: Vec<FrameSlot>,
    pub command_pool: vk::CommandPool,
    pub extent: vk::Extent2D,
}

/// Выбор usage: COLOR_ATTACHMENT обязателен, TRANSFER_DST нужен для `vkCmdClearColorImage`.
fn choose_usage(caps: &vk::SurfaceCapabilitiesKHR) -> Result<vk::ImageUsageFlags, String> {
    let wanted = vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::TRANSFER_DST;
    if caps.supported_usage_flags.contains(wanted) {
        Ok(wanted)
    } else {
        Err(format!(
            "surface не поддерживает COLOR_ATTACHMENT|TRANSFER_DST (есть {:?})",
            caps.supported_usage_flags
        ))
    }
}

/// Предпочитаем OPAQUE: кодеку альфа не нужна.
fn choose_composite_alpha(caps: &vk::SurfaceCapabilitiesKHR) -> vk::CompositeAlphaFlagsKHR {
    [
        vk::CompositeAlphaFlagsKHR::OPAQUE,
        vk::CompositeAlphaFlagsKHR::INHERIT,
        vk::CompositeAlphaFlagsKHR::PRE_MULTIPLIED,
        vk::CompositeAlphaFlagsKHR::POST_MULTIPLIED,
    ]
    .into_iter()
    .find(|flag| caps.supported_composite_alpha.contains(*flag))
    .unwrap_or(vk::CompositeAlphaFlagsKHR::OPAQUE)
}

impl Rig {
    /// Создаёт swapchain и ресурсы. Ошибка — строка `"<этап>: <VkResult>"`; частично созданное убирает `Drop`.
    pub fn create(
        host: &SurfaceInstance,
        present: &PresentDevice,
        query: &SurfaceQuery,
        params: &RenderParams,
    ) -> Result<(Self, SwapchainReport), String> {
        let raw_formats: Vec<_> = query.formats.iter().map(to_raw).collect();
        let chosen_raw =
            select_surface_format(&raw_formats, params.prefer_ten_bit).ok_or("surface не вернула ни одного формата")?;
        let chosen = query
            .formats
            .iter()
            .find(|candidate| to_raw(candidate) == chosen_raw)
            .ok_or("выбранный формат пропал из списка")?;
        let usage = choose_usage(&query.capabilities)?;
        let caps = &query.capabilities;
        let extent = vk::Extent2D {
            width: params.width.clamp(
                caps.min_image_extent.width,
                caps.max_image_extent.width.max(caps.min_image_extent.width),
            ),
            height: params.height.clamp(
                caps.min_image_extent.height,
                caps.max_image_extent.height.max(caps.min_image_extent.height),
            ),
        };
        let image_count = choose_image_count(caps.min_image_count, caps.max_image_count);
        let create_info = vk::SwapchainCreateInfoKHR::default()
            .surface(host.surface)
            .min_image_count(image_count)
            .image_format(chosen.format)
            .image_color_space(chosen.color_space)
            .image_extent(extent)
            .image_array_layers(1)
            .image_usage(usage)
            .image_sharing_mode(vk::SharingMode::EXCLUSIVE)
            .pre_transform(caps.current_transform)
            .composite_alpha(choose_composite_alpha(caps))
            // FIFO есть у любой реализации (требование спеки).
            .present_mode(vk::PresentModeKHR::FIFO)
            .clipped(true);

        let swapchain_loader = khr::swapchain::Device::new(&host.instance, &present.device);
        // SAFETY: create_info ссылается на живые surface/локальные данные.
        let swapchain = vk_step(
            unsafe { swapchain_loader.create_swapchain(&create_info, None) },
            "vkCreateSwapchainKHR",
        )?;
        let mut rig = Self {
            device: present.device.clone(),
            swapchain_loader,
            swapchain,
            images: Vec::new(),
            render_finished: Vec::new(),
            slots: Vec::new(),
            command_pool: vk::CommandPool::null(),
            extent,
        };
        rig.init_resources(present.queue_family_index)?;
        let report = SwapchainReport {
            ok: true,
            format: Some(format_entry(chosen).format),
            color_space: Some(format_entry(chosen).color_space),
            image_count: u32::try_from(rig.images.len()).unwrap_or(u32::MAX),
            extent: Some([rig.extent.width, rig.extent.height]),
            usage: Some(format!("{usage:?}")),
            error: None,
        };
        Ok((rig, report))
    }

    /// Картинки swapchain, command pool/buffers, семафоры и fence'ы.
    fn init_resources(&mut self, queue_family_index: u32) -> Result<(), String> {
        // SAFETY: swapchain только что создан.
        self.images = vk_step(
            unsafe { self.swapchain_loader.get_swapchain_images(self.swapchain) },
            "vkGetSwapchainImagesKHR",
        )?;
        let pool_info = vk::CommandPoolCreateInfo::default()
            .queue_family_index(queue_family_index)
            .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER);
        // SAFETY: device жив; pool_info валиден.
        self.command_pool = vk_step(
            unsafe { self.device.create_command_pool(&pool_info, None) },
            "vkCreateCommandPool",
        )?;
        let alloc_info = vk::CommandBufferAllocateInfo::default()
            .command_pool(self.command_pool)
            .level(vk::CommandBufferLevel::PRIMARY)
            .command_buffer_count(FRAMES_IN_FLIGHT as u32);
        // SAFETY: pool только что создан.
        let buffers = vk_step(
            unsafe { self.device.allocate_command_buffers(&alloc_info) },
            "vkAllocateCommandBuffers",
        )?;
        for command_buffer in buffers {
            let slot = self.new_slot(command_buffer)?;
            self.slots.push(slot);
        }
        for _ in 0..self.images.len() {
            // SAFETY: device жив.
            let semaphore = vk_step(
                unsafe { self.device.create_semaphore(&vk::SemaphoreCreateInfo::default(), None) },
                "vkCreateSemaphore",
            )?;
            self.render_finished.push(semaphore);
        }
        Ok(())
    }

    fn new_slot(&self, command_buffer: vk::CommandBuffer) -> Result<FrameSlot, String> {
        // SAFETY: device жив. Fence создаём сигнальным: первый кадр не должен ждать вечно.
        unsafe {
            let image_available = vk_step(
                self.device.create_semaphore(&vk::SemaphoreCreateInfo::default(), None),
                "vkCreateSemaphore",
            )?;
            let fence_info = vk::FenceCreateInfo::default().flags(vk::FenceCreateFlags::SIGNALED);
            let in_flight = vk_step(self.device.create_fence(&fence_info, None), "vkCreateFence")?;
            Ok(FrameSlot {
                command_buffer,
                image_available,
                in_flight,
            })
        }
    }

    /// Ждёт, пока GPU закончит всё отправленное; ошибка уходит в отчёт teardown.
    pub fn shutdown(&self) -> Result<(), String> {
        // SAFETY: device жив.
        vk_step(unsafe { self.device.device_wait_idle() }, "vkDeviceWaitIdle(rig)")
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        // SAFETY: shutdown() уже дождался простоя; уничтожаем в обратном порядке создания.
        // Null-хэндлы Vulkan разрешает передавать в destroy-функции.
        unsafe {
            for slot in &self.slots {
                self.device.destroy_fence(slot.in_flight, None);
                self.device.destroy_semaphore(slot.image_available, None);
            }
            for semaphore in &self.render_finished {
                self.device.destroy_semaphore(*semaphore, None);
            }
            // Command buffers освобождаются вместе с pool.
            self.device.destroy_command_pool(self.command_pool, None);
            self.swapchain_loader.destroy_swapchain(self.swapchain, None);
        }
    }
}

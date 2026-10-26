//! Ресурсы одного поколения swapchain: сам swapchain, его картинки и представления.
//!
//! Объект живёт, пока swapchain не пересоздан; пересоздание создаёт НОВЫЙ набор (с `oldSwapchain`)
//! и затем роняет старый. Порядок полей = порядок уничтожения: представления раньше swapchain.

use ash::vk;

use super::policy::SwapchainPlan;
use super::{SwapchainError, swapchain_step};
use crate::context::GpuContext;
use crate::error::GpuError;
use crate::live_objects::{OwnedImageView, OwnedSurface, OwnedSwapchain};

/// Swapchain с картинками и представлениями.
pub(super) struct SwapchainResources {
    // Порядок важен: представления картинок swapchain уничтожаются до самого swapchain.
    views: Vec<OwnedImageView>,
    images: Vec<vk::Image>,
    swapchain: OwnedSwapchain,
    plan: SwapchainPlan,
    /// Этот swapchain уже передан как `oldSwapchain`: по спецификации он «retired» и дальше
    /// использоваться как `oldSwapchain` не может (VUID-VkSwapchainCreateInfoKHR-oldSwapchain-01933),
    /// даже если создание преемника провалилось или преемник потом был отброшен.
    retired: bool,
}

impl SwapchainResources {
    /// Создаёт swapchain по плану на surface. `old` — прежний swapchain для `oldSwapchain`
    /// (драйвер может переиспользовать его ресурсы); после успеха прежний надо уничтожить (уронить).
    ///
    /// Прежний swapchain помечается retired СРАЗУ, до вызова драйвера: спецификация делает его
    /// retired при любом исходе `vkCreateSwapchainKHR`. Если он уже был retired (прошлое пересоздание
    /// сорвалось позже), `oldSwapchain` будет null — повторная передача была бы нарушением VUID.
    pub(super) fn create(
        context: &GpuContext,
        surface: &OwnedSurface,
        plan: &SwapchainPlan,
        old: Option<&mut SwapchainResources>,
    ) -> Result<Self, SwapchainError> {
        let loader = context.swapchain_loader()?;
        let old_swapchain = old.map_or(vk::SwapchainKHR::null(), SwapchainResources::retire_as_old_swapchain);
        let create_info = vk::SwapchainCreateInfoKHR::default()
            .surface(surface.handle())
            .min_image_count(plan.image_count)
            .image_format(plan.surface_format.format)
            .image_color_space(plan.surface_format.color_space)
            .image_extent(plan.extent)
            .image_array_layers(1)
            .image_usage(plan.usage)
            .image_sharing_mode(vk::SharingMode::EXCLUSIVE)
            // preTransform = currentTransform: композитор не вращает, поворот делает рисующий код.
            .pre_transform(plan.transform.to_vk())
            .composite_alpha(plan.composite_alpha)
            .present_mode(plan.present_mode)
            .clipped(true)
            .old_swapchain(old_swapchain);
        // SAFETY: surface принадлежит этому instance и жив; oldSwapchain (если есть) создан на этой же surface;
        // create_info ссылается на живые локальные данные.
        let raw_swapchain = swapchain_step(
            unsafe { loader.create_swapchain(&create_info, None) },
            "vkCreateSwapchainKHR",
        )?;
        // Сразу берём во владение: любая ошибка ниже уничтожит swapchain RAII-ом.
        let swapchain = OwnedSwapchain::from_raw(context, raw_swapchain)?;
        // SAFETY: swapchain только что создан этим загрузчиком.
        let images = swapchain_step(
            unsafe { loader.get_swapchain_images(swapchain.handle()) },
            "vkGetSwapchainImagesKHR",
        )?;

        let mut views = Vec::with_capacity(images.len());
        for image in &images {
            views.push(create_view(context, *image, plan.surface_format.format)?);
        }
        Ok(Self {
            views,
            images,
            swapchain,
            plan: *plan,
            retired: false,
        })
    }

    /// Отдаёт дескриптор для `oldSwapchain` и помечает swapchain retired. Повторный вызов
    /// возвращает null: retired swapchain как `oldSwapchain` передавать нельзя.
    fn retire_as_old_swapchain(&mut self) -> vk::SwapchainKHR {
        if self.retired {
            return vk::SwapchainKHR::null();
        }
        self.retired = true;
        self.swapchain.handle()
    }

    /// Был ли swapchain уже передан как `oldSwapchain` (для тестов и диагностики).
    #[cfg(any(test, feature = "fault-injection"))]
    pub(super) fn is_retired(&self) -> bool {
        self.retired
    }

    /// План, по которому создан swapchain.
    pub(super) fn plan(&self) -> &SwapchainPlan {
        &self.plan
    }

    /// Дескриптор swapchain.
    pub(super) fn swapchain_handle(&self) -> vk::SwapchainKHR {
        self.swapchain.handle()
    }

    /// Фактическое число картинок (драйвер мог создать больше запрошенного).
    pub(super) fn image_count(&self) -> usize {
        self.images.len()
    }

    /// Картинка и её представление по индексу из `vkAcquireNextImageKHR`.
    pub(super) fn image(&self, image_index: u32) -> Result<(vk::Image, vk::ImageView), GpuError> {
        let index = image_index as usize;
        match (self.images.get(index), self.views.get(index)) {
            (Some(image), Some(view)) => Ok((*image, view.handle())),
            _ => Err(GpuError::InvalidState {
                what: format!(
                    "драйвер вернул индекс картинки {image_index}, а в swapchain их {}",
                    self.images.len()
                ),
            }),
        }
    }
}

fn create_view(context: &GpuContext, image: vk::Image, format: vk::Format) -> Result<OwnedImageView, GpuError> {
    let info = vk::ImageViewCreateInfo::default()
        .image(image)
        .view_type(vk::ImageViewType::TYPE_2D)
        .format(format)
        .subresource_range(
            vk::ImageSubresourceRange::default()
                .aspect_mask(vk::ImageAspectFlags::COLOR)
                .level_count(1)
                .layer_count(1),
        );
    // SAFETY: image принадлежит живому swapchain; info — живая локальная структура.
    let handle = crate::error::run_step(
        unsafe { context.device().create_image_view(&info, None) },
        "vkCreateImageView",
    )?;
    Ok(OwnedImageView::from_raw(context, handle))
}

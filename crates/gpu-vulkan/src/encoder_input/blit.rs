//! Запись команд показа: `UNDEFINED → TRANSFER_DST → blit → PRESENT_SRC` для картинки swapchain.

use ash::vk;

use crate::context::GpuContext;
use crate::rgba8_image::Rgba8StorageImage;

/// Подресурс «весь цвет, один уровень, один слой» — картинки swapchain именно такие.
fn color_range() -> vk::ImageSubresourceRange {
    vk::ImageSubresourceRange {
        aspect_mask: vk::ImageAspectFlags::COLOR,
        base_mip_level: 0,
        level_count: 1,
        base_array_layer: 0,
        layer_count: 1,
    }
}

/// Записывает копию `source` (в [`Rgba8StorageImage::TRANSFER_SOURCE_LAYOUT`]) в картинку swapchain.
///
/// Барьеры:
/// 1. `UNDEFINED → TRANSFER_DST_OPTIMAL`: содержимое не важно (пишем целиком). Первая область барьера — стадия
///    `ALL_COMMANDS`, потому что сабмит ждёт семафор acquire на `ALL_COMMANDS` (без maintenance8 точнее нельзя);
/// 2. `vkCmdBlitImage2` 1:1 (`NEAREST`) — blit, а не copy: при разном порядке RGBA/BGRA он переставляет каналы;
/// 3. `TRANSFER_DST_OPTIMAL → PRESENT_SRC_KHR`: передача картинки показу (видимость обеспечивает семафор
///    «рендер готов», который ждёт present).
///
/// Исходная картинка остаётся в своей раскладке: метод её не меняет (ею продолжает пользоваться JPEG/readback).
pub(super) fn record_present_blit(
    context: &GpuContext,
    command_buffer: vk::CommandBuffer,
    source: &Rgba8StorageImage,
    destination: vk::Image,
    extent: vk::Extent2D,
) {
    let device = context.device();
    let to_transfer_dst = [vk::ImageMemoryBarrier2::default()
        .src_stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)
        .src_access_mask(vk::AccessFlags2::NONE)
        .dst_stage_mask(vk::PipelineStageFlags2::BLIT)
        .dst_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
        .old_layout(vk::ImageLayout::UNDEFINED)
        .new_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
        .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .image(destination)
        .subresource_range(color_range())];
    let to_present = [vk::ImageMemoryBarrier2::default()
        .src_stage_mask(vk::PipelineStageFlags2::BLIT)
        .src_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
        .dst_stage_mask(vk::PipelineStageFlags2::NONE)
        .dst_access_mask(vk::AccessFlags2::NONE)
        .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
        .new_layout(vk::ImageLayout::PRESENT_SRC_KHR)
        .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .image(destination)
        .subresource_range(color_range())];
    let layers = vk::ImageSubresourceLayers {
        aspect_mask: vk::ImageAspectFlags::COLOR,
        mip_level: 0,
        base_array_layer: 0,
        layer_count: 1,
    };
    // Размеры Vulkan для blit — `i32`; снимок ≤ лимита устройства (≤ 16384), приведение не теряет значение,
    // но на случай аномалии насыщаем, а не паникуем (драйвер/validation сообщит о выходе за границы).
    let corner = vk::Offset3D {
        x: i32::try_from(extent.width).unwrap_or(i32::MAX),
        y: i32::try_from(extent.height).unwrap_or(i32::MAX),
        z: 1,
    };
    let region = [vk::ImageBlit2::default()
        .src_subresource(layers)
        .src_offsets([vk::Offset3D::default(), corner])
        .dst_subresource(layers)
        .dst_offsets([vk::Offset3D::default(), corner])];
    let blit = vk::BlitImageInfo2::default()
        .src_image(source.handle())
        .src_image_layout(Rgba8StorageImage::TRANSFER_SOURCE_LAYOUT)
        .dst_image(destination)
        .dst_image_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
        .regions(&region)
        .filter(vk::Filter::NEAREST);
    // SAFETY: командный буфер в состоянии записи вне рендеринга (его только что открыла очередь одноразовой
    // работы); картинка swapchain acquired и принадлежит этому устройству; исходная картинка — в
    // TRANSFER_SRC_OPTIMAL (контракт `present_once`); массивы живут до конца вызовов.
    unsafe {
        device.cmd_pipeline_barrier2(
            command_buffer,
            &vk::DependencyInfo::default().image_memory_barriers(&to_transfer_dst),
        );
        device.cmd_blit_image2(command_buffer, &blit);
        device.cmd_pipeline_barrier2(
            command_buffer,
            &vk::DependencyInfo::default().image_memory_barriers(&to_present),
        );
    }
}

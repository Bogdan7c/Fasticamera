//! Барьеры раскладки цветовых картинок: общий «кирпич» для `ColorTarget` и `SampledTexture`.
//!
//! Область синхронизации описана тройкой «раскладка + стадия + доступ». Чтения несут `access = NONE`:
//! для «запись после чтения» (WAR) достаточно зависимости исполнения (обоснование — в документации
//! `storage_image::transitions`).

use ash::vk;

/// Область «до» или «после» барьера.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ImageScope {
    pub layout: vk::ImageLayout,
    pub stage: vk::PipelineStageFlags2,
    pub access: vk::AccessFlags2,
}

/// Записывает барьер одной цветовой картинки (1 mip, 1 слой) из `source` в `destination`.
pub(crate) fn record_image_barrier(
    device: &ash::Device,
    command_buffer: vk::CommandBuffer,
    image: vk::Image,
    source: ImageScope,
    destination: ImageScope,
) {
    let barriers = [vk::ImageMemoryBarrier2::default()
        .src_stage_mask(source.stage)
        .src_access_mask(source.access)
        .dst_stage_mask(destination.stage)
        .dst_access_mask(destination.access)
        .old_layout(source.layout)
        .new_layout(destination.layout)
        .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .image(image)
        .subresource_range(vk::ImageSubresourceRange {
            aspect_mask: vk::ImageAspectFlags::COLOR,
            base_mip_level: 0,
            level_count: 1,
            base_array_layer: 0,
            layer_count: 1,
        })];
    let dependency = vk::DependencyInfo::default().image_memory_barriers(&barriers);
    // SAFETY: командный буфер в состоянии записи вне динамического рендеринга (контракт вызывающего);
    // картинка принадлежит этому устройству; barriers живут до конца вызова.
    unsafe { device.cmd_pipeline_barrier2(command_buffer, &dependency) };
}

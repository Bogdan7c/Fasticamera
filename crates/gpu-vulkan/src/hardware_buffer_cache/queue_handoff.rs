//! Передача владения ресурсом между очередью Vulkan и «внешним владельцем» (камера/gralloc/компоновщик).
//!
//! Ресурс, чья память импортирована из `AHardwareBuffer`, пишет не только наш GPU: камера и CPU тоже.
//! Спецификация требует явно передавать владение через семейство очередей
//! `VK_QUEUE_FAMILY_FOREIGN_EXT` (расширение `VK_EXT_queue_family_foreign`, включено профилем
//! `AndroidDisplay`):
//! * **acquire** — забрать ресурс у внешнего владельца перед чтением в шейдере;
//! * **release** — вернуть его внешнему владельцу после (камера снова может писать).
//!
//! Для картинки из AHB acquire с `oldLayout = UNDEFINED` содержимое НЕ теряет (спецификация
//! `VK_ANDROID_external_memory_android_hardware_buffer`: для внешней памяти переход из FOREIGN
//! сохраняет данные) — именно так делал проверенный на телефоне зонд 00.
//!
//! Все барьеры — synchronization2 (`vkCmdPipelineBarrier2`), ядро Vulkan 1.3. Функции только
//! записывают команды в уже начатый командный буфер; владение очередью и порядок отправок — на
//! вызывающем (сессия 05 применит их же для кадров камеры).

use ash::{Device, vk};

/// Как картинка попадает в compute-шейдер (acquire).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageAcquire {
    /// Забрать у внешнего владельца (`FOREIGN → наша очередь`), содержимое сохраняется.
    FromForeignQueue,
    /// Картинка уже наша; перевести из `old_layout` (прежние записи завершены другой отправкой,
    /// дождавшейся fence). Используется на ПК, где `VK_EXT_queue_family_foreign` не включён.
    FromLayout(vk::ImageLayout),
}

/// Как буфер попадает в compute-шейдер (acquire).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BufferAcquire {
    /// Забрать у внешнего владельца.
    FromForeignQueue,
    /// Буфер уже наш; барьера владения не нужно.
    AlreadyOwned,
}

/// Куда ресурс уходит после чтения (release).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReleaseTarget {
    /// Вернуть внешнему владельцу (`наша очередь → FOREIGN`).
    ForeignQueue,
    /// Оставить у нас (барьера владения не нужно).
    KeepOwned,
}

/// Какая стадия конвейера читает ресурс после acquire (и до release): от неё зависят маски барьеров.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConsumerStage {
    /// Compute-шейдер (контрольные суммы).
    Compute,
    /// Фрагментный шейдер (предпросмотр кадра камеры на экране).
    Fragment,
    /// Вершинный И фрагментный шейдеры читают один ресурс (пирамида размытия: вершинный шейдер стекла берёт
    /// из неё среднюю яркость фона под группой, фрагментный — размытый фон).
    VertexAndFragment,
}

impl ConsumerStage {
    /// Стадия конвейера synchronization2, соответствующая потребителю.
    pub(crate) fn pipeline_stage(self) -> vk::PipelineStageFlags2 {
        match self {
            Self::Compute => vk::PipelineStageFlags2::COMPUTE_SHADER,
            Self::Fragment => vk::PipelineStageFlags2::FRAGMENT_SHADER,
            Self::VertexAndFragment => {
                vk::PipelineStageFlags2::VERTEX_SHADER | vk::PipelineStageFlags2::FRAGMENT_SHADER
            }
        }
    }
}

fn color_subresource_range() -> vk::ImageSubresourceRange {
    vk::ImageSubresourceRange {
        aspect_mask: vk::ImageAspectFlags::COLOR,
        base_mip_level: 0,
        level_count: 1,
        base_array_layer: 0,
        layer_count: 1,
    }
}

/// Записывает acquire картинки перед чтением в compute-шейдере. Итоговая раскладка —
/// `SHADER_READ_ONLY_OPTIMAL`: картинка читается как sampled image (`texelFetch`), потому что
/// камера отдаёт AHB только с usage `GPU_SAMPLED_IMAGE`, а storage image требовал бы
/// `GPU_DATA_BUFFER` (`VUID-VkMemoryAllocateInfo-pNext-02390`, 00 C4).
pub fn record_image_acquire(
    device: &Device,
    command_buffer: vk::CommandBuffer,
    image: vk::Image,
    acquire: ImageAcquire,
    queue_family_index: u32,
) {
    record_image_acquire_for_stage(
        device,
        command_buffer,
        image,
        acquire,
        queue_family_index,
        ConsumerStage::Compute,
    );
}

/// То же, что [`record_image_acquire`], но читать картинку будет стадия `stage`.
pub fn record_image_acquire_for_stage(
    device: &Device,
    command_buffer: vk::CommandBuffer,
    image: vk::Image,
    acquire: ImageAcquire,
    queue_family_index: u32,
    stage: ConsumerStage,
) {
    let barrier = match acquire {
        ImageAcquire::FromForeignQueue => vk::ImageMemoryBarrier2::default()
            // Предыдущие записи сделал внешний владелец; их видимость обеспечивает сам acquire.
            .src_stage_mask(vk::PipelineStageFlags2::NONE)
            .src_access_mask(vk::AccessFlags2::NONE)
            .old_layout(vk::ImageLayout::UNDEFINED)
            .src_queue_family_index(vk::QUEUE_FAMILY_FOREIGN_EXT)
            .dst_queue_family_index(queue_family_index),
        ImageAcquire::FromLayout(old_layout) => vk::ImageMemoryBarrier2::default()
            // Консервативно: ждём любые прежние записи (они уже завершены, барьер лишь делает их видимыми).
            .src_stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)
            .src_access_mask(vk::AccessFlags2::MEMORY_WRITE)
            .old_layout(old_layout)
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED),
    }
    .dst_stage_mask(stage.pipeline_stage())
    .dst_access_mask(vk::AccessFlags2::SHADER_SAMPLED_READ)
    .new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
    .image(image)
    .subresource_range(color_subresource_range());
    let barriers = [barrier];
    let dependency = vk::DependencyInfo::default().image_memory_barriers(&barriers);
    // SAFETY: командный буфер в состоянии recording (контракт функции); barriers живут до конца вызова.
    unsafe { device.cmd_pipeline_barrier2(command_buffer, &dependency) };
}

/// Записывает release картинки после чтения: возвращает внешнему владельцу.
/// Раскладка остаётся `SHADER_READ_ONLY_OPTIMAL` (смена раскладки при передаче владения не нужна).
pub fn record_image_release(
    device: &Device,
    command_buffer: vk::CommandBuffer,
    image: vk::Image,
    target: ReleaseTarget,
    queue_family_index: u32,
) {
    record_image_release_for_stage(
        device,
        command_buffer,
        image,
        target,
        queue_family_index,
        ConsumerStage::Compute,
    );
}

/// То же, что [`record_image_release`], но картинку читала стадия `stage`.
pub fn record_image_release_for_stage(
    device: &Device,
    command_buffer: vk::CommandBuffer,
    image: vk::Image,
    target: ReleaseTarget,
    queue_family_index: u32,
    stage: ConsumerStage,
) {
    if target == ReleaseTarget::KeepOwned {
        return;
    }
    let barrier = vk::ImageMemoryBarrier2::default()
        .src_stage_mask(stage.pipeline_stage())
        .src_access_mask(vk::AccessFlags2::SHADER_SAMPLED_READ)
        .dst_stage_mask(vk::PipelineStageFlags2::NONE)
        .dst_access_mask(vk::AccessFlags2::NONE)
        .old_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
        .new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
        .src_queue_family_index(queue_family_index)
        .dst_queue_family_index(vk::QUEUE_FAMILY_FOREIGN_EXT)
        .image(image)
        .subresource_range(color_subresource_range());
    let barriers = [barrier];
    let dependency = vk::DependencyInfo::default().image_memory_barriers(&barriers);
    // SAFETY: см. record_image_acquire.
    unsafe { device.cmd_pipeline_barrier2(command_buffer, &dependency) };
}

/// Записывает acquire буфера перед чтением в compute-шейдере.
pub fn record_buffer_acquire(
    device: &Device,
    command_buffer: vk::CommandBuffer,
    buffer: vk::Buffer,
    acquire: BufferAcquire,
    queue_family_index: u32,
) {
    record_buffer_acquire_for_stage(
        device,
        command_buffer,
        buffer,
        acquire,
        queue_family_index,
        ConsumerStage::Compute,
    );
}

/// То же, что [`record_buffer_acquire`], но читать буфер будет стадия `stage`.
pub fn record_buffer_acquire_for_stage(
    device: &Device,
    command_buffer: vk::CommandBuffer,
    buffer: vk::Buffer,
    acquire: BufferAcquire,
    queue_family_index: u32,
    stage: ConsumerStage,
) {
    if acquire == BufferAcquire::AlreadyOwned {
        return;
    }
    let barrier = vk::BufferMemoryBarrier2::default()
        .src_stage_mask(vk::PipelineStageFlags2::NONE)
        .src_access_mask(vk::AccessFlags2::NONE)
        .dst_stage_mask(stage.pipeline_stage())
        .dst_access_mask(vk::AccessFlags2::SHADER_STORAGE_READ)
        .src_queue_family_index(vk::QUEUE_FAMILY_FOREIGN_EXT)
        .dst_queue_family_index(queue_family_index)
        .buffer(buffer)
        .offset(0)
        .size(vk::WHOLE_SIZE);
    let barriers = [barrier];
    let dependency = vk::DependencyInfo::default().buffer_memory_barriers(&barriers);
    // SAFETY: см. record_image_acquire.
    unsafe { device.cmd_pipeline_barrier2(command_buffer, &dependency) };
}

/// Записывает release буфера после чтения.
pub fn record_buffer_release(
    device: &Device,
    command_buffer: vk::CommandBuffer,
    buffer: vk::Buffer,
    target: ReleaseTarget,
    queue_family_index: u32,
) {
    record_buffer_release_for_stage(
        device,
        command_buffer,
        buffer,
        target,
        queue_family_index,
        ConsumerStage::Compute,
    );
}

/// То же, что [`record_buffer_release`], но буфер читала стадия `stage`.
pub fn record_buffer_release_for_stage(
    device: &Device,
    command_buffer: vk::CommandBuffer,
    buffer: vk::Buffer,
    target: ReleaseTarget,
    queue_family_index: u32,
    stage: ConsumerStage,
) {
    if target == ReleaseTarget::KeepOwned {
        return;
    }
    let barrier = vk::BufferMemoryBarrier2::default()
        .src_stage_mask(stage.pipeline_stage())
        .src_access_mask(vk::AccessFlags2::SHADER_STORAGE_READ)
        .dst_stage_mask(vk::PipelineStageFlags2::NONE)
        .dst_access_mask(vk::AccessFlags2::NONE)
        .src_queue_family_index(queue_family_index)
        .dst_queue_family_index(vk::QUEUE_FAMILY_FOREIGN_EXT)
        .buffer(buffer)
        .offset(0)
        .size(vk::WHOLE_SIZE);
    let barriers = [barrier];
    let dependency = vk::DependencyInfo::default().buffer_memory_barriers(&barriers);
    // SAFETY: см. record_image_acquire.
    unsafe { device.cmd_pipeline_barrier2(command_buffer, &dependency) };
}

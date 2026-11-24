//! [`Rgba8StorageImage`] — картинка RGBA 8 бит (`R8G8B8A8_UNORM`), которую пишет compute-шейдер, а потом
//! читают КОМАНДЫ КОПИРОВАНИЯ (сессия 14, пакет P3: готовый снимок).
//!
//! Отличие от [`crate::storage_image::StorageImage`]: та выбирает формат по возможностям устройства
//! (10 бит или fp16) и читается как sampled; здесь формат ФИКСИРОВАН (`R8G8B8A8_UNORM` обязан
//! поддерживать storage-доступ на любом устройстве Vulkan — спецификация, «Required Format Support»),
//! а читают её копированием: в картинку поверхности кодера HEIF (`vkCmdCopyImage`) или в буфер
//! ([`HostReadbackBuffer`]) для JPEG.
//!
//! # Раскладки
//! Картинка не отслеживает раскладку сама (как и `StorageImage`): барьеры пишутся в командные буферы,
//! поэтому порядок задаёт вызывающий:
//!
//! ```text
//! UNDEFINED ─record_prepare_for_compute_write─▶ GENERAL (шейдер пишет)
//!           ─record_compute_write_to_transfer_read─▶ TRANSFER_SRC_OPTIMAL (копируют)
//! ```
//!
//! Раскладка после `record_compute_write_to_transfer_read` — [`Rgba8StorageImage::TRANSFER_SOURCE_LAYOUT`];
//! стадия и доступ копирующих команд — `COPY` / `TRANSFER_READ`. Картинку в эту раскладку и оставляют:
//! возвращать её в `GENERAL` нужно, только если её будут писать снова (новая запись — с `UNDEFINED`,
//! прежнее содержимое при этом отбрасывается).

use ash::vk;

use crate::compute::DescriptorResource;
use crate::context::{GpuContext, MemoryLocation};
use crate::error::{GpuError, run_step};
use crate::host_readback::HostReadbackBuffer;
use crate::live_objects::{OwnedDeviceMemory, OwnedImage, OwnedImageView};

/// Формат картинки: RGBA, 8 бит на канал, UNORM (байты в памяти — R, G, B, A).
const RGBA8_FORMAT: vk::Format = vk::Format::R8G8B8A8_UNORM;
/// Байт на пиксель.
const BYTES_PER_PIXEL: u64 = 4;

/// Картинка RGBA8 с памятью GPU. Порядок полей = порядок уничтожения: представление → картинка → память.
pub struct Rgba8StorageImage {
    view: OwnedImageView,
    image: OwnedImage,
    _memory: OwnedDeviceMemory,
    extent: vk::Extent2D,
    context: GpuContext,
}

impl std::fmt::Debug for Rgba8StorageImage {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "Rgba8StorageImage({}x{})",
            self.extent.width, self.extent.height
        )
    }
}

impl Rgba8StorageImage {
    /// Раскладка, в которой картинку читают копирующие команды (после `record_compute_write_to_transfer_read`).
    pub const TRANSFER_SOURCE_LAYOUT: vk::ImageLayout = vk::ImageLayout::TRANSFER_SRC_OPTIMAL;

    /// Создаёт картинку. Нулевой размер и размер сверх лимита устройства — `InvalidArgument`. Содержимое
    /// не определено: первый переход — из `UNDEFINED` ([`Self::record_prepare_for_compute_write`]).
    pub fn new(context: &GpuContext, extent: vk::Extent2D) -> Result<Self, GpuError> {
        let limit = context.max_image_dimension_2d();
        let vk::Extent2D { width, height } = extent;
        if width == 0 || height == 0 || width > limit || height > limit {
            return Err(GpuError::InvalidArgument {
                what: format!("размер картинки RGBA8 {width}x{height} вне 1..={limit}"),
            });
        }
        let device = context.device();
        let image_info = vk::ImageCreateInfo::default()
            .image_type(vk::ImageType::TYPE_2D)
            .format(RGBA8_FORMAT)
            .extent(vk::Extent3D {
                width,
                height,
                depth: 1,
            })
            .mip_levels(1)
            .array_layers(1)
            .samples(vk::SampleCountFlags::TYPE_1)
            .tiling(vk::ImageTiling::OPTIMAL)
            // STORAGE — пишет compute; TRANSFER_SRC — копирование в surface кодера и в буфер JPEG;
            // TRANSFER_DST — очистка/загрузка (диагностика и тесты, на скорость storage-доступа не влияет).
            .usage(vk::ImageUsageFlags::STORAGE | vk::ImageUsageFlags::TRANSFER_SRC | vk::ImageUsageFlags::TRANSFER_DST)
            .sharing_mode(vk::SharingMode::EXCLUSIVE)
            .initial_layout(vk::ImageLayout::UNDEFINED);
        // SAFETY: устройство валидно; image_info — живая локальная структура.
        let image = OwnedImage::from_raw(
            context,
            run_step(
                unsafe { device.create_image(&image_info, None) },
                "vkCreateImage(Rgba8StorageImage)",
            )?,
        );
        // SAFETY: картинка только что создана этим устройством.
        let requirements = unsafe { device.get_image_memory_requirements(image.handle()) };
        let memory = context.allocate_memory(requirements, MemoryLocation::DeviceLocal, "Rgba8StorageImage")?;
        // SAFETY: память подобрана по требованиям картинки и ещё не привязана; смещение 0.
        run_step(
            unsafe { device.bind_image_memory(image.handle(), memory.handle(), 0) },
            "vkBindImageMemory(Rgba8StorageImage)",
        )?;
        let view_info = vk::ImageViewCreateInfo::default()
            .image(image.handle())
            .view_type(vk::ImageViewType::TYPE_2D)
            .format(RGBA8_FORMAT)
            .subresource_range(color_range());
        // SAFETY: картинка с привязанной памятью; формат совпадает.
        let view = OwnedImageView::from_raw(
            context,
            run_step(
                unsafe { device.create_image_view(&view_info, None) },
                "vkCreateImageView(Rgba8StorageImage)",
            )?,
        );
        Ok(Self {
            view,
            image,
            _memory: memory,
            extent,
            context: context.clone(),
        })
    }

    /// Размер, пиксели.
    pub fn extent(&self) -> vk::Extent2D {
        self.extent
    }

    /// Сколько байт займёт картинка плотными строками RGBA (размер буфера для `record_copy_to_host_readback`).
    pub fn packed_size_bytes(&self) -> u64 {
        u64::from(self.extent.width) * u64::from(self.extent.height) * BYTES_PER_PIXEL
    }

    /// Ресурс для привязки [`crate::compute::BindingKind::StorageImageWrite`] (раскладка `GENERAL`).
    pub fn storage_binding(&self) -> DescriptorResource {
        DescriptorResource::StorageImage {
            view: self.view.handle(),
        }
    }

    /// Сам `VkImage` — ТОЛЬКО как источник команд копирования (`vkCmdCopyImage`, `vkCmdCopyImageToBuffer`)
    /// в раскладке [`Self::TRANSFER_SOURCE_LAYOUT`] после `record_compute_write_to_transfer_read`. Картинка
    /// должна жить, пока GPU не закончит копирование: следит владелец сабмита.
    pub fn handle(&self) -> vk::Image {
        self.image.handle()
    }

    /// Готовит картинку к записи в compute: `UNDEFINED → GENERAL` (прежнее содержимое отбрасывается).
    ///
    /// Первая область барьера охватывает все прежние команды очереди (в том числе прошлых сабмитов), поэтому
    /// прежние копирования из картинки закончатся до новой записи.
    pub fn record_prepare_for_compute_write(&self, command_buffer: vk::CommandBuffer) {
        self.record_barrier(
            command_buffer,
            Scope {
                layout: vk::ImageLayout::UNDEFINED,
                stage: vk::PipelineStageFlags2::COPY,
                access: vk::AccessFlags2::NONE,
            },
            Scope {
                layout: vk::ImageLayout::GENERAL,
                stage: vk::PipelineStageFlags2::COMPUTE_SHADER,
                access: vk::AccessFlags2::SHADER_STORAGE_WRITE,
            },
        );
    }

    /// Записанное compute-шейдером отдаёт командам передачи (копирование и blit): `GENERAL → TRANSFER_SRC_OPTIMAL`.
    pub fn record_compute_write_to_transfer_read(&self, command_buffer: vk::CommandBuffer) {
        self.record_barrier(
            command_buffer,
            Scope {
                layout: vk::ImageLayout::GENERAL,
                stage: vk::PipelineStageFlags2::COMPUTE_SHADER,
                access: vk::AccessFlags2::SHADER_STORAGE_WRITE,
            },
            Scope {
                layout: Self::TRANSFER_SOURCE_LAYOUT,
                // Читателей два рода: `vkCmdCopyImageToBuffer` (стадия COPY) и `vkCmdBlitImage2` показа в кодер
                // (стадия BLIT). В sync2 COPY не включает BLIT, поэтому охватываем всю передачу данных, а не
                // угадываем читателя (спецификация Vulkan, VkPipelineStageFlagBits2: ALL_TRANSFER = COPY | BLIT |
                // RESOLVE | CLEAR | ACCELERATION_STRUCTURE_COPY).
                stage: vk::PipelineStageFlags2::ALL_TRANSFER,
                access: vk::AccessFlags2::TRANSFER_READ,
            },
        );
    }

    /// Копирует всю картинку в `destination` плотными строками RGBA (байты R, G, B, A) и делает запись
    /// видимой хосту. Картинка должна быть в [`Self::TRANSFER_SOURCE_LAYOUT`] (после
    /// `record_compute_write_to_transfer_read`); раскладку копирование не меняет.
    ///
    /// `destination` обязан вмещать ровно [`Self::packed_size_bytes`] (иначе `InvalidArgument` до записи команд).
    pub fn record_copy_to_host_readback(
        &self,
        command_buffer: vk::CommandBuffer,
        destination: &HostReadbackBuffer,
    ) -> Result<(), GpuError> {
        if destination.size_bytes() != self.packed_size_bytes() {
            return Err(GpuError::InvalidArgument {
                what: format!(
                    "буфер {} байт, а картинка {}x{} занимает {}",
                    destination.size_bytes(),
                    self.extent.width,
                    self.extent.height,
                    self.packed_size_bytes()
                ),
            });
        }
        // Нулевые buffer_row_length/buffer_image_height = плотные строки по ширине картинки.
        let region = vk::BufferImageCopy::default()
            .image_subresource(vk::ImageSubresourceLayers {
                aspect_mask: vk::ImageAspectFlags::COLOR,
                mip_level: 0,
                base_array_layer: 0,
                layer_count: 1,
            })
            .image_extent(vk::Extent3D {
                width: self.extent.width,
                height: self.extent.height,
                depth: 1,
            });
        // SAFETY: командный буфер в состоянии записи вне рендеринга (контракт вызывающего); картинка в
        // TRANSFER_SRC_OPTIMAL (контракт, `record_compute_write_to_transfer_read`), буфер вмещает w*h*4 байт
        // (проверено выше) и имеет usage TRANSFER_DST.
        unsafe {
            self.context.device().cmd_copy_image_to_buffer(
                command_buffer,
                self.image.handle(),
                Self::TRANSFER_SOURCE_LAYOUT,
                destination.buffer_handle(),
                &[region],
            );
        }
        destination.record_transfer_write_to_host_read(command_buffer);
        Ok(())
    }

    fn record_barrier(&self, command_buffer: vk::CommandBuffer, source: Scope, destination: Scope) {
        let barriers = [vk::ImageMemoryBarrier2::default()
            .src_stage_mask(source.stage)
            .src_access_mask(source.access)
            .dst_stage_mask(destination.stage)
            .dst_access_mask(destination.access)
            .old_layout(source.layout)
            .new_layout(destination.layout)
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .image(self.image.handle())
            .subresource_range(color_range())];
        let dependency = vk::DependencyInfo::default().image_memory_barriers(&barriers);
        // SAFETY: командный буфер в состоянии записи вне динамического рендеринга (контракт вызывающего);
        // картинка принадлежит этому устройству; barriers живут до конца вызова.
        unsafe { self.context.device().cmd_pipeline_barrier2(command_buffer, &dependency) };
    }

    /// Читает картинку на CPU (плотные строки RGBA) и ждёт завершения. Блокирующая операция вне кадрового цикла (тесты, дайджест стенда насыщения).
    ///
    /// Контракт: картинка уже записана compute-шейдером в более ранних сабмитах той же очереди и сейчас в
    /// раскладке [`Self::TRANSFER_SOURCE_LAYOUT`] (в ней её и оставляет читатель).
    pub fn read_back_blocking(&self) -> Result<Vec<u8>, GpuError> {
        let mut staging = HostReadbackBuffer::new(&self.context, self.packed_size_bytes())?;
        let run = crate::kit_test_support::run_one_shot_blocking_traced(&self.context, |command_buffer| {
            self.record_copy_to_host_readback(command_buffer, &staging)
        })?;
        staging.mark_written_by(&run.work);
        let sealed = staging.seal(&run.proof).map_err(|error| GpuError::InvalidState {
            what: error.to_string(),
        })?;
        Ok(sealed.bytes().to_vec())
    }
}

/// Область барьера: раскладка, стадия, доступ.
#[derive(Clone, Copy)]
struct Scope {
    layout: vk::ImageLayout,
    stage: vk::PipelineStageFlags2,
    access: vk::AccessFlags2,
}

fn color_range() -> vk::ImageSubresourceRange {
    vk::ImageSubresourceRange {
        aspect_mask: vk::ImageAspectFlags::COLOR,
        base_mip_level: 0,
        level_count: 1,
        base_array_layer: 0,
        layer_count: 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::test_support::gpu_context_or_skip;
    use crate::live_objects::LiveObjectKind;

    #[test]
    fn zero_or_oversized_extent_is_rejected_without_creating_objects() {
        let Some(context) = gpu_context_or_skip() else { return };
        let baseline = context.live_objects();
        for (width, height) in [(0, 4), (4, 0), (context.max_image_dimension_2d() + 1, 4)] {
            let error = Rgba8StorageImage::new(&context, vk::Extent2D { width, height }).unwrap_err();
            assert!(
                matches!(error, GpuError::InvalidArgument { .. }),
                "{width}x{height}: {error:?}"
            );
        }
        assert_eq!(context.live_objects(), baseline);
    }

    #[test]
    fn cleared_pixels_come_back_through_transfer_copy_in_rgba_byte_order() {
        let Some(context) = gpu_context_or_skip() else { return };
        let baseline = context.live_objects();
        {
            let image = Rgba8StorageImage::new(&context, vk::Extent2D { width: 3, height: 2 }).unwrap();
            assert_eq!(image.packed_size_bytes(), 24);
            assert!(matches!(
                image.storage_binding(),
                DescriptorResource::StorageImage { .. }
            ));
            context
                .run_one_shot(|command_buffer| {
                    image.record_prepare_for_compute_write(command_buffer);
                    let everything = [vk::MemoryBarrier2::default()
                        .src_stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)
                        .src_access_mask(vk::AccessFlags2::MEMORY_WRITE)
                        .dst_stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)
                        .dst_access_mask(vk::AccessFlags2::MEMORY_READ | vk::AccessFlags2::MEMORY_WRITE)];
                    let dependency = vk::DependencyInfo::default().memory_barriers(&everything);
                    // SAFETY: командный буфер в состоянии записи; массив барьеров живёт до конца вызова.
                    unsafe { context.device().cmd_pipeline_barrier2(command_buffer, &dependency) };
                    // Очищаем в GENERAL (имитация записи шейдером): R=1, G=0, B=0.2 (→ 51), A=1.
                    let color = vk::ClearColorValue {
                        float32: [1.0, 0.0, 0.2, 1.0],
                    };
                    // SAFETY: командный буфер в состоянии записи; картинка в GENERAL с usage TRANSFER_DST.
                    unsafe {
                        context.device().cmd_clear_color_image(
                            command_buffer,
                            image.handle(),
                            vk::ImageLayout::GENERAL,
                            &color,
                            &[color_range()],
                        );
                    }
                    // SAFETY: как выше — командный буфер в состоянии записи.
                    unsafe { context.device().cmd_pipeline_barrier2(command_buffer, &dependency) };
                    image.record_compute_write_to_transfer_read(command_buffer);
                    Ok(())
                })
                .unwrap();
            let bytes = image.read_back_blocking().unwrap();
            assert_eq!(bytes.len(), 24);
            for pixel in bytes.chunks_exact(4) {
                assert_eq!(pixel, [255, 0, 51, 255], "порядок байт R, G, B, A");
            }
            assert_eq!(context.live_objects().count(LiveObjectKind::Image), 1);
        }
        assert_eq!(
            context.live_objects(),
            baseline,
            "картинка, представление и память освобождены"
        );
        assert_eq!(
            context.validation_report().errors,
            0,
            "{:?}",
            context.validation_report()
        );
    }

    /// Читатель картинки после `record_compute_write_to_transfer_read` — `vkCmdBlitImage2` (показ в кодер), а не
    /// только копирование: барьер обязан охватывать стадию BLIT. Иначе sync-validation (FULL) видит гонку
    /// «чтение blit после записи/смены раскладки», а байты назначения не гарантированы.
    #[test]
    fn blit_reader_after_compute_write_barrier_is_synchronized_and_copies_pixels() {
        let Some(context) = gpu_context_or_skip() else { return };
        let extent = vk::Extent2D { width: 4, height: 3 };
        let source = Rgba8StorageImage::new(&context, extent).unwrap();
        let destination = Rgba8StorageImage::new(&context, extent).unwrap();
        let everything = [vk::MemoryBarrier2::default()
            .src_stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)
            .src_access_mask(vk::AccessFlags2::MEMORY_WRITE)
            .dst_stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)
            .dst_access_mask(vk::AccessFlags2::MEMORY_READ | vk::AccessFlags2::MEMORY_WRITE)];
        let dependency = vk::DependencyInfo::default().memory_barriers(&everything);
        context
            .run_one_shot(|command_buffer| {
                source.record_prepare_for_compute_write(command_buffer);
                // SAFETY: командный буфер в состоянии записи; массив барьеров живёт до конца вызова.
                unsafe { context.device().cmd_pipeline_barrier2(command_buffer, &dependency) };
                let color = vk::ClearColorValue {
                    float32: [0.0, 1.0, 0.0, 1.0],
                };
                // SAFETY: командный буфер в состоянии записи; картинка в GENERAL с usage TRANSFER_DST.
                unsafe {
                    context.device().cmd_clear_color_image(
                        command_buffer,
                        source.handle(),
                        vk::ImageLayout::GENERAL,
                        &color,
                        &[color_range()],
                    );
                    context.device().cmd_pipeline_barrier2(command_buffer, &dependency);
                }
                source.record_compute_write_to_transfer_read(command_buffer);
                // Назначение: готовим к записи blit-ом (любая предыдущая работа уже закончена барьером выше).
                destination.record_prepare_for_compute_write(command_buffer);
                let to_transfer_dst = [vk::ImageMemoryBarrier2::default()
                    .src_stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)
                    .src_access_mask(vk::AccessFlags2::MEMORY_WRITE)
                    .dst_stage_mask(vk::PipelineStageFlags2::BLIT)
                    .dst_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
                    .old_layout(vk::ImageLayout::GENERAL)
                    .new_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                    .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .image(destination.handle())
                    .subresource_range(color_range())];
                let to_transfer_dst_dependency = vk::DependencyInfo::default().image_memory_barriers(&to_transfer_dst);
                let layers = vk::ImageSubresourceLayers {
                    aspect_mask: vk::ImageAspectFlags::COLOR,
                    mip_level: 0,
                    base_array_layer: 0,
                    layer_count: 1,
                };
                let corner = vk::Offset3D { x: 4, y: 3, z: 1 };
                let region = [vk::ImageBlit2::default()
                    .src_subresource(layers)
                    .src_offsets([vk::Offset3D::default(), corner])
                    .dst_subresource(layers)
                    .dst_offsets([vk::Offset3D::default(), corner])];
                let blit = vk::BlitImageInfo2::default()
                    .src_image(source.handle())
                    .src_image_layout(Rgba8StorageImage::TRANSFER_SOURCE_LAYOUT)
                    .dst_image(destination.handle())
                    .dst_image_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                    .regions(&region)
                    .filter(vk::Filter::NEAREST);
                // SAFETY: командный буфер в состоянии записи вне рендеринга; обе картинки в нужных раскладках
                // (источник — TRANSFER_SRC после барьера под тестом, назначение — TRANSFER_DST); массивы живут
                // до конца вызова.
                unsafe {
                    context
                        .device()
                        .cmd_pipeline_barrier2(command_buffer, &to_transfer_dst_dependency);
                    context.device().cmd_blit_image2(command_buffer, &blit);
                }
                // Назначение — источник для чтения хостом: тот же барьер, что и у источника.
                let to_readable = [vk::ImageMemoryBarrier2::default()
                    .src_stage_mask(vk::PipelineStageFlags2::BLIT)
                    .src_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
                    .dst_stage_mask(vk::PipelineStageFlags2::COPY)
                    .dst_access_mask(vk::AccessFlags2::TRANSFER_READ)
                    .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                    .new_layout(Rgba8StorageImage::TRANSFER_SOURCE_LAYOUT)
                    .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .image(destination.handle())
                    .subresource_range(color_range())];
                let to_readable_dependency = vk::DependencyInfo::default().image_memory_barriers(&to_readable);
                // SAFETY: командный буфер в состоянии записи; массив барьеров живёт до конца вызова.
                unsafe {
                    context
                        .device()
                        .cmd_pipeline_barrier2(command_buffer, &to_readable_dependency);
                }
                Ok(())
            })
            .unwrap();
        let bytes = destination.read_back_blocking().unwrap();
        assert_eq!(bytes.len(), 4 * 3 * 4);
        for pixel in bytes.chunks_exact(4) {
            assert_eq!(pixel, [0, 255, 0, 255], "blit перенёс записанный цвет источника");
        }
        assert_eq!(
            context.validation_report().errors,
            0,
            "{:?}",
            context.validation_report()
        );
    }

    #[test]
    fn copy_into_wrong_sized_buffer_is_refused_before_any_command() {
        let Some(context) = gpu_context_or_skip() else { return };
        let image = Rgba8StorageImage::new(&context, vk::Extent2D { width: 2, height: 2 }).unwrap();
        let too_small = HostReadbackBuffer::new(&context, 8).unwrap();
        let error = context
            .run_one_shot(|command_buffer| image.record_copy_to_host_readback(command_buffer, &too_small))
            .unwrap_err();
        assert!(matches!(error, GpuError::InvalidArgument { .. }), "{error:?}");
        assert_eq!(too_small.size_bytes(), 8, "чужой буфер не тронут");
    }
}

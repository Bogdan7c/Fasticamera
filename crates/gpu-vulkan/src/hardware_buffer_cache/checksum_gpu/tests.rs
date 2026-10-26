//! Тесты запуска контрольных сумм на реальном GPU ПК (RADV) с полной validation.
//! Шейдеры, дескрипторы и барьеры те же, что на телефоне; не проверяется только импорт AHB.

use ash::vk;

use super::*;
use crate::context::test_support::gpu_context_or_skip;
use crate::hardware_buffer_cache::queue_handoff::{BufferAcquire, ImageAcquire, ReleaseTarget};
use crate::hardware_buffer_cache::raw_reference::{
    RawPixelFormat, RowGeometry, checksum_strided_words, checksum_u16_texels, fill_pattern,
};
use crate::live_objects::{OwnedImage, OwnedImageView};

/// Картинка `R16_UINT` в памяти GPU с загруженными байтами (как будто её записала камера).
struct UploadedImage {
    // Порядок полей = порядок уничтожения: представление → картинка → память.
    view: OwnedImageView,
    image: OwnedImage,
    _memory: OwnedDeviceMemory,
}

fn upload_r16_image(context: &GpuContext, width: u32, height: u32, texel_bytes: &[u8]) -> UploadedImage {
    let device = context.device();
    let image_info = vk::ImageCreateInfo::default()
        .image_type(vk::ImageType::TYPE_2D)
        .format(vk::Format::R16_UINT)
        .extent(vk::Extent3D {
            width,
            height,
            depth: 1,
        })
        .mip_levels(1)
        .array_layers(1)
        .samples(vk::SampleCountFlags::TYPE_1)
        .tiling(vk::ImageTiling::OPTIMAL)
        .usage(vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::TRANSFER_DST)
        .sharing_mode(vk::SharingMode::EXCLUSIVE)
        .initial_layout(vk::ImageLayout::UNDEFINED);
    // SAFETY: тестовые структуры живут до конца вызова.
    let image = OwnedImage::from_raw(context, unsafe { device.create_image(&image_info, None) }.unwrap());
    // SAFETY: картинка создана выше.
    let requirements = unsafe { device.get_image_memory_requirements(image.handle()) };
    let memory = context
        .allocate_memory(requirements, MemoryLocation::DeviceLocal, "тестовая картинка R16")
        .unwrap();
    // SAFETY: картинка и память одного устройства.
    unsafe { device.bind_image_memory(image.handle(), memory.handle(), 0) }.unwrap();
    let view_info = vk::ImageViewCreateInfo::default()
        .image(image.handle())
        .view_type(vk::ImageViewType::TYPE_2D)
        .format(vk::Format::R16_UINT)
        .subresource_range(vk::ImageSubresourceRange {
            aspect_mask: vk::ImageAspectFlags::COLOR,
            base_mip_level: 0,
            level_count: 1,
            base_array_layer: 0,
            layer_count: 1,
        });
    // SAFETY: картинка привязана к памяти.
    let view = OwnedImageView::from_raw(context, unsafe { device.create_image_view(&view_info, None) }.unwrap());

    // Загрузка: staging-буфер → картинка (раскладка TRANSFER_DST_OPTIMAL остаётся после копии).
    let staging = host_visible_buffer(context, texel_bytes, vk::BufferUsageFlags::TRANSFER_SRC);
    context
        .run_one_shot(|command_buffer| {
            let to_transfer = vk::ImageMemoryBarrier2::default()
                .src_stage_mask(vk::PipelineStageFlags2::NONE)
                .dst_stage_mask(vk::PipelineStageFlags2::COPY)
                .dst_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
                .old_layout(vk::ImageLayout::UNDEFINED)
                .new_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                .image(image.handle())
                .subresource_range(vk::ImageSubresourceRange {
                    aspect_mask: vk::ImageAspectFlags::COLOR,
                    base_mip_level: 0,
                    level_count: 1,
                    base_array_layer: 0,
                    layer_count: 1,
                });
            let barriers = [to_transfer];
            let region = vk::BufferImageCopy::default()
                .image_subresource(vk::ImageSubresourceLayers {
                    aspect_mask: vk::ImageAspectFlags::COLOR,
                    mip_level: 0,
                    base_array_layer: 0,
                    layer_count: 1,
                })
                .image_extent(vk::Extent3D {
                    width,
                    height,
                    depth: 1,
                });
            // SAFETY: командный буфер в состоянии recording (контракт run_one_shot).
            unsafe {
                device.cmd_pipeline_barrier2(
                    command_buffer,
                    &vk::DependencyInfo::default().image_memory_barriers(&barriers),
                );
                device.cmd_copy_buffer_to_image(
                    command_buffer,
                    staging.buffer.handle(),
                    image.handle(),
                    vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                    &[region],
                );
            }
            Ok(())
        })
        .unwrap();
    UploadedImage {
        view,
        image,
        _memory: memory,
    }
}

/// Host-visible буфер с копией `bytes`.
struct HostBuffer {
    buffer: OwnedBuffer,
    _memory: OwnedDeviceMemory,
}

fn host_visible_buffer(context: &GpuContext, bytes: &[u8], usage: vk::BufferUsageFlags) -> HostBuffer {
    let device = context.device();
    let info = vk::BufferCreateInfo::default()
        .size(bytes.len() as u64)
        .usage(usage)
        .sharing_mode(vk::SharingMode::EXCLUSIVE);
    // SAFETY: тестовая структура живёт до конца вызова.
    let buffer = OwnedBuffer::from_raw(context, unsafe { device.create_buffer(&info, None) }.unwrap());
    // SAFETY: буфер создан выше.
    let requirements = unsafe { device.get_buffer_memory_requirements(buffer.handle()) };
    let memory = context
        .allocate_memory(requirements, MemoryLocation::HostVisibleCoherent, "тестовый буфер")
        .unwrap();
    // SAFETY: буфер и память одного устройства; память host-visible, отображаем ровно bytes.len().
    unsafe {
        device.bind_buffer_memory(buffer.handle(), memory.handle(), 0).unwrap();
        let address = device
            .map_memory(memory.handle(), 0, bytes.len() as u64, vk::MemoryMapFlags::empty())
            .unwrap();
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), address.cast::<u8>(), bytes.len());
        device.unmap_memory(memory.handle());
    }
    HostBuffer {
        buffer,
        _memory: memory,
    }
}

fn raw16_geometry(width: u32, height: u32) -> RowGeometry {
    RowGeometry {
        rows: height,
        row_valid_bytes: u64::from(width) * 2,
        row_stride_bytes: u64::from(width) * 2,
    }
}

fn pattern_bytes(format: RawPixelFormat, geometry: &RowGeometry) -> Vec<u8> {
    let mut bytes = vec![0_u8; (geometry.row_stride_bytes * u64::from(geometry.rows)) as usize];
    assert!(fill_pattern(&mut bytes, format, geometry));
    bytes
}

/// Источник сразу после загрузки: картинка в `TRANSFER_DST_OPTIMAL`.
fn image_source(uploaded: &UploadedImage, width: u32, height: u32) -> ChecksumSource {
    image_source_in_layout(uploaded, width, height, vk::ImageLayout::TRANSFER_DST_OPTIMAL)
}

fn image_source_in_layout(
    uploaded: &UploadedImage,
    width: u32,
    height: u32,
    current_layout: vk::ImageLayout,
) -> ChecksumSource {
    ChecksumSource::SampledImage {
        view: uploaded.view.handle(),
        image: uploaded.image.handle(),
        width,
        height,
        acquire: ImageAcquire::FromLayout(current_layout),
        release: ReleaseTarget::KeepOwned,
    }
}

fn assert_clean(context: &GpuContext) {
    context.wait_idle().unwrap();
    let report = context.validation_report();
    assert_eq!(report.errors, 0, "validation: {report:?}");
}

#[test]
fn gpu_image_checksum_equals_cpu_reference_for_odd_sizes() {
    let Some(context) = gpu_context_or_skip() else { return };
    let runner = GpuChecksumRunner::new(&context, ChecksumRunnerConfig::default()).unwrap();
    // Нечётные размеры и один большой кадр (несколько итераций grid-stride цикла шейдера).
    for (width, height) in [(1, 1), (97, 61), (1024, 768)] {
        let geometry = raw16_geometry(width, height);
        let bytes = pattern_bytes(RawPixelFormat::Raw16, &geometry);
        let uploaded = upload_r16_image(&context, width, height, &bytes);
        let gpu = runner.run(&image_source(&uploaded, width, height)).unwrap();
        let cpu = checksum_u16_texels(&bytes, width, &geometry).unwrap();
        assert_eq!(gpu, cpu, "{width}x{height}");
    }
    assert_clean(&context);
}

#[test]
fn gpu_image_checksum_changes_when_a_single_texel_changes() {
    let Some(context) = gpu_context_or_skip() else { return };
    let runner = GpuChecksumRunner::new(&context, ChecksumRunnerConfig::default()).unwrap();
    let (width, height) = (33, 17);
    let geometry = raw16_geometry(width, height);
    let bytes = pattern_bytes(RawPixelFormat::Raw16, &geometry);
    let original = runner
        .run(&image_source(
            &upload_r16_image(&context, width, height, &bytes),
            width,
            height,
        ))
        .unwrap();

    let mut corrupted = bytes.clone();
    let last = corrupted.len() - 1;
    corrupted[last] ^= 0x80; // старший бит последнего текселя
    let changed = runner
        .run(&image_source(
            &upload_r16_image(&context, width, height, &corrupted),
            width,
            height,
        ))
        .unwrap();
    assert_ne!(changed.sum, original.sum);
    assert_ne!(changed.mix, original.mix);
    assert_eq!(changed, checksum_u16_texels(&corrupted, width, &geometry).unwrap());
    assert_clean(&context);
}

#[test]
fn gpu_buffer_checksum_equals_cpu_reference_and_skips_row_padding() {
    let Some(context) = gpu_context_or_skip() else { return };
    let runner = GpuChecksumRunner::new(&context, ChecksumRunnerConfig::default()).unwrap();
    // RAW10: ширина 100 → 125 полезных байт, шаг 160 (паддинг 35 байт забит сентинелом).
    let (width, rows) = (100_u32, 77_u32);
    let valid = RawPixelFormat::Raw10.valid_row_bytes(width);
    let geometry = RowGeometry {
        rows,
        row_valid_bytes: valid,
        row_stride_bytes: 160,
    };
    let bytes = pattern_bytes(RawPixelFormat::Raw10, &geometry);
    let host = host_visible_buffer(&context, &bytes, vk::BufferUsageFlags::STORAGE_BUFFER);
    let source_with_stride = |stride_words| ChecksumSource::PackedBuffer {
        buffer: host.buffer.handle(),
        size: bytes.len() as u64,
        row_words: (valid / 4) as u32,
        stride_words,
        rows,
        acquire: BufferAcquire::AlreadyOwned,
        release: ReleaseTarget::KeepOwned,
    };
    let gpu = runner.run(&source_with_stride(160 / 4)).unwrap();
    assert_eq!(gpu, checksum_strided_words(&bytes, &geometry).unwrap());

    // Неверный шаг строки (как если бы забыли паддинг) обязан дать другой результат.
    assert_ne!(runner.run(&source_with_stride((valid / 4) as u32)).unwrap(), gpu);
    assert_clean(&context);
}

#[test]
fn invalid_sources_are_rejected_before_any_gpu_work() {
    let Some(context) = gpu_context_or_skip() else { return };
    let runner = GpuChecksumRunner::new(&context, ChecksumRunnerConfig::default()).unwrap();
    let baseline = context.live_objects();
    let host = host_visible_buffer(&context, &[0_u8; 64], vk::BufferUsageFlags::STORAGE_BUFFER);
    let live_with_buffer = context.live_objects();
    let buffer_source = |row_words, stride_words, rows| ChecksumSource::PackedBuffer {
        buffer: host.buffer.handle(),
        size: 64,
        row_words,
        stride_words,
        rows,
        acquire: BufferAcquire::AlreadyOwned,
        release: ReleaseTarget::KeepOwned,
    };
    for source in [
        buffer_source(0, 4, 4), // нет слов в строке (деление на ноль в шейдере)
        buffer_source(4, 4, 0), // нет строк
        buffer_source(8, 4, 2), // шаг меньше строки
        buffer_source(4, 4, 5), // 5 строк × 16 байт = 80 > 64
    ] {
        let error = runner.run(&source).unwrap_err();
        assert!(matches!(error, GpuError::InvalidArgument { .. }), "{error:?}");
    }
    assert_eq!(
        context.live_objects(),
        live_with_buffer,
        "отказ не должен создавать объекты GPU"
    );
    drop(host);
    assert_eq!(context.live_objects(), baseline);
}

#[test]
fn zero_workgroup_count_is_invalid_configuration() {
    let Some(context) = gpu_context_or_skip() else { return };
    let error = GpuChecksumRunner::new(&context, ChecksumRunnerConfig { workgroup_count: 0 })
        .err()
        .unwrap();
    assert!(matches!(error, GpuError::InvalidArgument { .. }), "{error:?}");
}

#[test]
fn repeated_runs_do_not_leak_gpu_objects() {
    let Some(context) = gpu_context_or_skip() else { return };
    let baseline = context.live_objects();
    {
        let runner = GpuChecksumRunner::new(&context, ChecksumRunnerConfig::default()).unwrap();
        let geometry = raw16_geometry(16, 8);
        let bytes = pattern_bytes(RawPixelFormat::Raw16, &geometry);
        let uploaded = upload_r16_image(&context, 16, 8, &bytes);
        let live_during = context.live_objects();
        // Первый запуск переводит картинку в SHADER_READ_ONLY_OPTIMAL, последующие стартуют уже из неё.
        let mut layout = vk::ImageLayout::TRANSFER_DST_OPTIMAL;
        for _ in 0..20 {
            runner.run(&image_source_in_layout(&uploaded, 16, 8, layout)).unwrap();
            layout = vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL;
            assert_eq!(
                context.live_objects(),
                live_during,
                "каждый запуск обязан вернуть объекты"
            );
        }
    }
    assert_eq!(context.live_objects(), baseline);
    assert_clean(&context);
}

//! Обработка одного кадра: описание AHB → импорт в Vulkan → compute-сумма → сверка с CPU →
//! освобождение `AImage`.
//!
//! КЛЮЧЕВОЙ ИНВАРИАНТ: `AImage` возвращается камере только ПОСЛЕ того, как GPU закончил чтение.
//! Здесь это обеспечено дважды: (1) мы дожидаемся `VkFence` отправки до чтения результата;
//! (2) освобождаем через `AImage_deleteAsync` с release-fence, экспортированным из семафора,
//! который сигналит та же отправка (либо `AImage_delete` после `vkQueueWaitIdle`).

use std::os::fd::OwnedFd;

use ash::vk;

use super::acquired_image::{AcquiredImage, describe_hardware_buffer};
use super::ahb_lock::LockedBuffer;
use super::registry::ReaderEntry;
use crate::camera_metadata::FORMAT_RAW16;
use crate::checksum::{RowLayout, row_layout, sum_strided_words, sum_u16_texels};
use crate::hardware_buffer_names::{ahb_format_name, usage_flag_names};
use crate::report::{AhbDescription, AhbVulkanProperties, ComputeResult, FrameReport, ImportAttempt};
use crate::vulkan::ahb_import::{
    AhbProperties, AhbRef, ImageGeometry, ImageKind, ImportedBuffer, ImportedImage, create_r16_uint_view,
    import_as_buffer, import_as_image, query_properties,
};
use crate::vulkan::compute::{GpuInput, SubmitSync, submit};
use crate::vulkan::context::VulkanContext;
use crate::vulkan::resources::FrameResources;
use crate::vulkan::sync_fd::{
    AcquireImport, create_exportable_semaphore, export_release_fence, import_acquire_fence, wait_fence_on_cpu,
};

/// Сколько ждём acquire-fence на CPU в fallback-режиме.
const CPU_FENCE_WAIT_MS: i32 = 2000;

/// Как освобождать `AImage` после обработки.
enum ReleasePlan {
    /// GPU читал буфер, release-fence экспортирован (`None` внутри — уже сигнален, fd = -1).
    AfterGpuWithFence(Option<OwnedFd>),
    /// GPU читал буфер, но fence экспортировать не удалось: ждём очередь целиком, потом `AImage_delete`.
    AfterQueueIdle { reason: String },
    /// GPU буфер не трогал — достаточно обычного удаления.
    NoGpuWork,
}

/// Путь импорта, на котором будет посчитана сумма.
enum ChosenImport {
    Image(ImportedImage),
    Buffer(ImportedBuffer),
}

/// Полный разбор одного кадра. Всегда возвращает отчёт (ошибки — внутри него).
pub fn process_frame(ctx: &VulkanContext, index: usize, mut image: AcquiredImage, reader: &ReaderEntry) -> FrameReport {
    let mut report = FrameReport {
        index,
        ..FrameReport::default()
    };
    // Очередь Vulkan одна: на время кадра берём её целиком.
    let queue_guard = ctx.lock_queue();
    let plan = {
        let mut resources = FrameResources::new(ctx);
        run_gpu_stages(ctx, &mut resources, &mut image, reader, &mut report)
        // Здесь `resources` уничтожается: Drop сначала делает queue_wait_idle.
    };
    drop(queue_guard);
    release_image(ctx, image, plan, &mut report);
    report
}

/// Всё между захватом и освобождением. Заполняет `report`, возвращает план освобождения.
fn run_gpu_stages(
    ctx: &VulkanContext,
    resources: &mut FrameResources<'_>,
    image: &mut AcquiredImage,
    reader: &ReaderEntry,
    report: &mut FrameReport,
) -> ReleasePlan {
    let ahb = match image.hardware_buffer() {
        Ok(buffer) => buffer,
        Err(error) => {
            report.error = Some(error);
            return ReleasePlan::NoGpuWork;
        }
    };
    // SAFETY: ahb получен от живого AImage, который мы держим до release_image.
    let desc = unsafe { describe_hardware_buffer(ahb) };
    let plane0_stride = image.plane0_row_stride_bytes();
    report.ahb = Some(AhbDescription {
        format: desc.format,
        format_name: ahb_format_name(desc.format),
        width: desc.width,
        height: desc.height,
        layers: desc.layers,
        stride: desc.stride,
        usage_hex: format!("0x{:X}", desc.usage),
        usage_flags: usage_flag_names(desc.usage),
        plane0_row_stride_bytes: plane0_stride,
    });

    // SAFETY: буфер принадлежит `image`, который живёт до release_image (после всех обращений к vk_ahb).
    let vk_ahb = unsafe { AhbRef::from_raw(ahb.cast()) };
    let properties = match query_properties(ctx, vk_ahb) {
        Ok(properties) => properties,
        Err(error) => {
            report.error = Some(error);
            return ReleasePlan::NoGpuWork;
        }
    };
    report.ahb_vulkan = Some(AhbVulkanProperties {
        allocation_size: properties.allocation_size,
        memory_type_bits: properties.memory_type_bits,
        format: format!("{:?}", properties.format),
        external_format: properties.external_format,
        format_features_hex: format!("0x{:X}", properties.format_features.as_raw()),
    });

    let wait_semaphore = prepare_acquire_sync(ctx, resources, image.take_acquire_fence(), report);

    let geometry = ImageGeometry {
        width: desc.width,
        height: desc.height,
        layers: desc.layers,
    };
    let chosen = try_import_paths(ctx, vk_ahb, &properties, &geometry, reader.format, report);
    let Some(chosen) = chosen else {
        report.error = Some("ни один путь импорта не подходит для compute-проверки".to_owned());
        return ReleasePlan::NoGpuWork;
    };

    let layout = match row_layout(
        reader.format,
        desc.width as usize,
        desc.height as usize,
        plane0_stride,
        desc.stride,
        usize::try_from(properties.allocation_size).ok(),
    ) {
        Ok(layout) => layout,
        Err(error) => {
            report.error = Some(error);
            return ReleasePlan::NoGpuWork;
        }
    };
    run_checksum_stage(
        ctx,
        resources,
        ahb,
        &properties,
        chosen,
        &layout,
        wait_semaphore,
        report,
    )
}

/// Подготавливает ожидание acquire-fence: импорт в семафор либо CPU-poll. Заполняет `report.acquire_fence`.
fn prepare_acquire_sync(
    ctx: &VulkanContext,
    resources: &mut FrameResources<'_>,
    fence: Option<OwnedFd>,
    report: &mut FrameReport,
) -> Option<vk::Semaphore> {
    let Some(fence) = fence else {
        report.acquire_fence = "none".to_owned();
        return None;
    };
    match import_acquire_fence(ctx, resources, fence) {
        AcquireImport::Imported(semaphore) => {
            report.acquire_fence = "imported_as_vk_semaphore".to_owned();
            Some(semaphore)
        }
        AcquireImport::Failed { fence, reason } => {
            // Без импорта GPU не умеет ждать камеру сам — ждём на CPU, прежде чем что-либо читать.
            report.acquire_fence = match wait_fence_on_cpu(&fence, CPU_FENCE_WAIT_MS) {
                Ok(()) => format!("cpu_poll_fallback(import_failed: {reason})"),
                Err(wait_error) => format!("cpu_poll_failed(import_failed: {reason}; {wait_error})"),
            };
            None
        }
    }
}

/// Форматы-кандидаты для VkImage: заявленный драйвером либо типичные для RAW.
fn candidate_formats(properties: &AhbProperties) -> Vec<vk::Format> {
    if properties.format == vk::Format::UNDEFINED {
        vec![vk::Format::R16_UINT, vk::Format::R16_UNORM, vk::Format::R8_UINT]
    } else {
        vec![properties.format]
    }
}

/// Пробует все пути импорта, каждый фиксирует в отчёте; возвращает первый пригодный для compute
/// (приоритет: образ R16_UINT, затем буфер).
fn try_import_paths(
    ctx: &VulkanContext,
    ahb: AhbRef,
    properties: &AhbProperties,
    geometry: &ImageGeometry,
    reader_format: i32,
    report: &mut FrameReport,
) -> Option<ChosenImport> {
    let mut chosen: Option<ChosenImport> = None;

    // (a) VkImage с обычным форматом — только для плотного RAW16. У упакованного RAW10 и
    // непрозрачного RAW_PRIVATE ширина×высота образа не совпадает с байтами буфера: образ
    // «4096×3072 R16» на аллокации RAW10 (15,7 МБ) читал бы за её пределами (факт сессии 00 —
    // драйвер такой bind не отвергает). Для них честный путь — только VkBuffer.
    let image_formats = if reader_format == FORMAT_RAW16 {
        candidate_formats(properties)
    } else {
        report.import_attempts.push(ImportAttempt {
            path: "image".into(),
            vk_format: None,
            ok: false,
            error: Some(format!(
                "пропущено: формат 0x{reader_format:X} упакован/непрозрачен, геометрия образа не равна буферу"
            )),
            compute_capable: false,
        });
        Vec::new()
    };
    for format in image_formats {
        // Шейдер sum_r16 читает именно r16ui, для него нужна фича extended storage formats.
        let compute_capable = format == vk::Format::R16_UINT && ctx.caps.storage_image_extended_formats;
        let outcome = import_as_image(ctx, ahb, properties, geometry, &ImageKind::Storage(format));
        report
            .import_attempts
            .push(attempt_report("image", Some(format), &outcome, compute_capable));
        if let Ok(imported) = outcome {
            if compute_capable && chosen.is_none() {
                chosen = Some(ChosenImport::Image(imported));
            } else {
                imported.destroy(ctx);
            }
        }
    }

    // (b) VkImage с externalFormat: проверяем только создание и привязку памяти.
    let external = if properties.external_format == 0 {
        Err("externalFormat == 0: драйвер не предложил внешний формат".to_owned())
    } else {
        import_as_image(ctx, ahb, properties, geometry, &ImageKind::ExternalFormat)
    };
    report
        .import_attempts
        .push(attempt_report("image_external_format", None, &external, false));
    if let Ok(imported) = external {
        imported.destroy(ctx);
    }

    // (c) VkBuffer на память AHB.
    let buffer = import_as_buffer(ctx, ahb, properties);
    let buffer_report = match &buffer {
        Ok(_) => ImportAttempt {
            path: "buffer".into(),
            vk_format: None,
            ok: true,
            error: None,
            compute_capable: true,
        },
        Err(error) => ImportAttempt {
            path: "buffer".into(),
            vk_format: None,
            ok: false,
            error: Some(error.clone()),
            compute_capable: true,
        },
    };
    report.import_attempts.push(buffer_report);
    if let Ok(imported) = buffer {
        if chosen.is_none() {
            chosen = Some(ChosenImport::Buffer(imported));
        } else {
            imported.destroy(ctx);
        }
    }
    chosen
}

/// Запись о попытке импорта образа.
fn attempt_report(
    path: &str,
    format: Option<vk::Format>,
    outcome: &Result<ImportedImage, String>,
    compute_capable: bool,
) -> ImportAttempt {
    ImportAttempt {
        path: path.to_owned(),
        vk_format: format.map(|format| format!("{format:?}")),
        ok: outcome.is_ok(),
        error: outcome.as_ref().err().cloned(),
        compute_capable,
    }
}

/// GPU-сумма + CPU-эталон + экспорт release-fence.
#[allow(clippy::too_many_arguments)]
fn run_checksum_stage(
    ctx: &VulkanContext,
    resources: &mut FrameResources<'_>,
    ahb: *mut ndk_sys::AHardwareBuffer,
    properties: &AhbProperties,
    chosen: ChosenImport,
    layout: &RowLayout,
    wait_semaphore: Option<vk::Semaphore>,
    report: &mut FrameReport,
) -> ReleasePlan {
    let (input, path_name) = match build_gpu_input(ctx, resources, properties, chosen, layout, report) {
        Ok(prepared) => prepared,
        Err(error) => {
            report.error = Some(error);
            return ReleasePlan::NoGpuWork;
        }
    };

    // Семафор для release-fence. Если создать нельзя — продолжаем без него (fallback ниже).
    let (signal_semaphore, signal_error) = match create_exportable_semaphore(ctx, resources) {
        Ok(semaphore) => (Some(semaphore), None),
        Err(error) => (None, Some(error)),
    };
    let sync = SubmitSync {
        wait: wait_semaphore,
        signal: signal_semaphore,
    };
    let job = match submit(ctx, resources, &input, &sync) {
        Ok(job) => job,
        Err(error) => {
            report.error = Some(error);
            // Отправка могла частично пройти — на всякий случай ждём очередь перед удалением.
            return ReleasePlan::AfterQueueIdle {
                reason: "submit_failed".to_owned(),
            };
        }
    };

    // Экспорт ПОСЛЕ submit: у семафора уже есть отложенный сигнал.
    let release_fence = match (signal_semaphore, signal_error) {
        (Some(semaphore), _) => export_release_fence(ctx, semaphore).map_err(|error| format!("export_failed: {error}")),
        (None, Some(error)) => Err(format!("export_unavailable: {error}")),
        (None, None) => Err("export_unavailable".to_owned()),
    };

    let run = match job.wait_and_read(ctx) {
        Ok(run) => run,
        Err(error) => {
            report.error = Some(error);
            return ReleasePlan::AfterQueueIdle {
                reason: "gpu_wait_failed".to_owned(),
            };
        }
    };

    let (cpu_checksum, cpu_error) = match cpu_reference_checksum(ahb, properties, path_name, layout) {
        Ok(sum) => (Some(sum), None),
        Err(error) => (None, Some(error)),
    };
    report.compute = Some(ComputeResult {
        path: path_name.to_owned(),
        gpu_checksum: run.checksum,
        cpu_checksum,
        matches: cpu_checksum.map(|cpu| cpu == run.checksum),
        gpu_time_us: run.time_us,
        gpu_time_source: run.time_source.to_owned(),
        cpu_checksum_error: cpu_error,
    });

    match release_fence {
        Ok(fence) => ReleasePlan::AfterGpuWithFence(fence),
        Err(reason) => ReleasePlan::AfterQueueIdle { reason },
    }
}

/// Превращает выбранный импорт в вход шейдера (создаёт view, проверяет геометрию буфера).
/// Объекты передаются в корзину кадра.
fn build_gpu_input(
    ctx: &VulkanContext,
    resources: &mut FrameResources<'_>,
    properties: &AhbProperties,
    chosen: ChosenImport,
    layout: &RowLayout,
    report: &FrameReport,
) -> Result<(GpuInput, &'static str), String> {
    match chosen {
        ChosenImport::Image(imported) => {
            let view = create_r16_uint_view(ctx, imported.image);
            resources.adopt_image(imported.image);
            resources.adopt_memory(imported.memory);
            let view = view?;
            resources.adopt_image_view(view);
            let (width, height) = report
                .ahb
                .as_ref()
                .map(|ahb| (ahb.width, ahb.height))
                .ok_or("нет описания AHB")?;
            Ok((
                GpuInput::Image {
                    view,
                    image: imported.image,
                    width,
                    height,
                },
                "image_r16_uint",
            ))
        }
        ChosenImport::Buffer(imported) => {
            resources.adopt_buffer(imported.buffer);
            resources.adopt_memory(imported.memory);
            let row_words = layout.row_valid_bytes / 4;
            if layout.row_stride_bytes % 4 != 0 || row_words == 0 {
                return Err(format!(
                    "буферный путь требует stride кратный 4 и ≥1 слова в строке: stride={}, valid={}",
                    layout.row_stride_bytes, layout.row_valid_bytes
                ));
            }
            if layout.required_bytes() as u64 > properties.allocation_size {
                return Err(format!(
                    "геометрия ({} байт) не помещается в аллокацию ({})",
                    layout.required_bytes(),
                    properties.allocation_size
                ));
            }
            Ok((
                GpuInput::Buffer {
                    buffer: imported.buffer,
                    size: imported.size,
                    row_words: u32::try_from(row_words).map_err(|_| "row_words не влезает в u32")?,
                    stride_words: u32::try_from(layout.row_stride_bytes / 4)
                        .map_err(|_| "stride_words не влезает в u32")?,
                    rows: u32::try_from(layout.rows).map_err(|_| "rows не влезает в u32")?,
                },
                "buffer",
            ))
        }
    }
}

/// CPU-сумма тех же данных через `AHardwareBuffer_lock` (уважает stride).
fn cpu_reference_checksum(
    ahb: *mut ndk_sys::AHardwareBuffer,
    properties: &AhbProperties,
    path_name: &str,
    layout: &RowLayout,
) -> Result<u32, String> {
    let needed = layout.required_bytes();
    if needed as u64 > properties.allocation_size {
        return Err(format!(
            "геометрия ({needed} байт) больше аллокации ({})",
            properties.allocation_size
        ));
    }
    // SAFETY: ahb принадлежит живому AImage; GPU уже закончил (мы дождались fence).
    let locked = unsafe { LockedBuffer::lock_for_cpu_read(ahb) }?;
    // SAFETY: проверили выше, что `needed` ≤ allocationSize аллокации.
    let bytes = unsafe { locked.bytes(needed) };
    let result = if path_name == "image_r16_uint" {
        let width = layout.row_valid_bytes / 2;
        sum_u16_texels(bytes, width, layout.rows, layout.row_stride_bytes)
    } else {
        sum_strided_words(bytes, layout.rows, layout.row_valid_bytes, layout.row_stride_bytes)
    };
    result.map_err(|error| error.to_string())
}

/// Освобождение `AImage` по плану; результат — в `report.release`.
fn release_image(ctx: &VulkanContext, image: AcquiredImage, plan: ReleasePlan, report: &mut FrameReport) {
    match plan {
        ReleasePlan::AfterGpuWithFence(fence) => {
            report.release = if fence.is_some() {
                "sync_fd_via_delete_async".to_owned()
            } else {
                "sync_fd_via_delete_async(fd=-1: GPU уже закончил)".to_owned()
            };
            image.release_after_gpu(fence);
        }
        ReleasePlan::AfterQueueIdle { reason } => {
            let _queue_guard = ctx.lock_queue();
            // SAFETY: очередь захвачена; простая синхронная операция.
            let idle = unsafe { ctx.device.queue_wait_idle(ctx.queue) };
            report.release = match idle {
                Ok(()) => format!("delete_after_queue_wait_idle({reason})"),
                Err(code) => format!("delete_after_queue_wait_idle_FAILED({reason}; {code:?})"),
            };
            image.release_now();
        }
        ReleasePlan::NoGpuWork => {
            report.release = "plain_delete(gpu_not_used)".to_owned();
            image.release_now();
        }
    }
}

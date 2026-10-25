//! Запуск compute-шейдера контрольной суммы над импортированным кадром.
//!
//! Работа разделена на `submit` (записать и отправить в очередь) и `GpuJob::wait_and_read`
//! (дождаться и прочитать результат): между ними вызывающий экспортирует release-fence из
//! сигнального семафора, пока GPU ещё может работать — так проверяется реальный сценарий.

use std::time::{Duration, Instant};

use ash::vk;

use super::context::{ComputePipeline, VulkanContext};
use super::resources::FrameResources;
use super::vk_step;
use crate::gpu_timing::ticks_to_microseconds;

/// Число рабочих групп. Шейдеры обходят данные «решёткой», поэтому число групп
/// не зависит от размера кадра (8192×6144 тоже укладывается).
const WORKGROUP_COUNT: u32 = 1024;
/// Сколько ждём GPU, прежде чем объявить зависание.
const GPU_TIMEOUT: Duration = Duration::from_secs(5);
/// Размер результата — одно u32.
const RESULT_BYTES: u64 = 4;

/// Что именно читает шейдер.
pub enum GpuInput {
    /// `VkImage` R16_UINT (путь `sum_r16.comp`).
    Image {
        view: vk::ImageView,
        image: vk::Image,
        width: u32,
        height: u32,
    },
    /// `VkBuffer` на память AHB (путь `sum_words.comp`).
    Buffer {
        buffer: vk::Buffer,
        size: u64,
        row_words: u32,
        stride_words: u32,
        rows: u32,
    },
}

/// Отправленная, но ещё не дождавшаяся работа.
pub struct GpuJob {
    fence: vk::Fence,
    query_pool: Option<vk::QueryPool>,
    result: *const u32,
    submitted_at: Instant,
}

/// Результат завершённой работы.
pub struct GpuRun {
    pub checksum: u32,
    pub time_us: f64,
    /// `timestamp_query` или `host_clock`.
    pub time_source: &'static str,
}

/// Создаёт host-visible буфер результата (4 байта, обнулён) и возвращает (buffer, указатель на данные).
fn create_result_buffer(
    ctx: &VulkanContext,
    resources: &mut FrameResources<'_>,
) -> Result<(vk::Buffer, *mut u32), String> {
    let buffer_info = vk::BufferCreateInfo::default()
        .size(RESULT_BYTES)
        .usage(vk::BufferUsageFlags::STORAGE_BUFFER)
        .sharing_mode(vk::SharingMode::EXCLUSIVE);
    // SAFETY: параметры валидны, device жив.
    let buffer = vk_step(
        unsafe { ctx.device.create_buffer(&buffer_info, None) },
        "vkCreateBuffer(result)",
    )?;
    resources.adopt_buffer(buffer);
    // SAFETY: buffer только что создан.
    let requirements = unsafe { ctx.device.get_buffer_memory_requirements(buffer) };
    let memory_type = ctx
        .find_memory_type(
            requirements.memory_type_bits,
            vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
        )
        .ok_or("нет HOST_VISIBLE|HOST_COHERENT памяти для буфера результата")?;
    let allocate_info = vk::MemoryAllocateInfo::default()
        .allocation_size(requirements.size)
        .memory_type_index(memory_type);
    // SAFETY: allocate_info валиден.
    let memory = vk_step(
        unsafe { ctx.device.allocate_memory(&allocate_info, None) },
        "vkAllocateMemory(result)",
    )?;
    resources.adopt_memory(memory);
    // SAFETY: buffer и memory на одном device, память подходит под требования буфера.
    vk_step(
        unsafe { ctx.device.bind_buffer_memory(buffer, memory, 0) },
        "vkBindBufferMemory(result)",
    )?;
    // SAFETY: память HOST_VISIBLE и не отображена; регион целиком.
    let mapped = vk_step(
        unsafe {
            ctx.device
                .map_memory(memory, 0, vk::WHOLE_SIZE, vk::MemoryMapFlags::empty())
        },
        "vkMapMemory(result)",
    )?;
    resources.adopt_mapped_memory(memory);
    let result_ptr = mapped.cast::<u32>();
    // SAFETY: отображено минимум RESULT_BYTES (≥4) байт; обнуляем накопитель до запуска GPU.
    unsafe { result_ptr.write_volatile(0) };
    Ok((buffer, result_ptr))
}

/// Пул дескрипторов + набор, привязанный к входу и буферу результата.
fn bind_descriptors(
    ctx: &VulkanContext,
    resources: &mut FrameResources<'_>,
    pipeline: &ComputePipeline,
    input: &GpuInput,
    result_buffer: vk::Buffer,
) -> Result<vk::DescriptorSet, String> {
    let pool_sizes: Vec<vk::DescriptorPoolSize> = match input {
        GpuInput::Image { .. } => vec![
            vk::DescriptorPoolSize::default()
                .ty(vk::DescriptorType::STORAGE_IMAGE)
                .descriptor_count(1),
            vk::DescriptorPoolSize::default()
                .ty(vk::DescriptorType::STORAGE_BUFFER)
                .descriptor_count(1),
        ],
        GpuInput::Buffer { .. } => {
            vec![
                vk::DescriptorPoolSize::default()
                    .ty(vk::DescriptorType::STORAGE_BUFFER)
                    .descriptor_count(2),
            ]
        }
    };
    let pool_info = vk::DescriptorPoolCreateInfo::default()
        .max_sets(1)
        .pool_sizes(&pool_sizes);
    // SAFETY: pool_info валиден.
    let pool = vk_step(
        unsafe { ctx.device.create_descriptor_pool(&pool_info, None) },
        "vkCreateDescriptorPool",
    )?;
    resources.adopt_descriptor_pool(pool);
    let layouts = [pipeline.descriptor_set_layout];
    let allocate_info = vk::DescriptorSetAllocateInfo::default()
        .descriptor_pool(pool)
        .set_layouts(&layouts);
    // SAFETY: пул создан выше, layout принадлежит контексту.
    let sets = vk_step(
        unsafe { ctx.device.allocate_descriptor_sets(&allocate_info) },
        "vkAllocateDescriptorSets",
    )?;
    let set = *sets.first().ok_or("vkAllocateDescriptorSets: пустой результат")?;

    let image_infos = [match input {
        GpuInput::Image { view, .. } => vk::DescriptorImageInfo::default()
            .image_view(*view)
            .image_layout(vk::ImageLayout::GENERAL),
        GpuInput::Buffer { .. } => vk::DescriptorImageInfo::default(),
    }];
    let input_buffer_infos = [match input {
        GpuInput::Buffer { buffer, size, .. } => vk::DescriptorBufferInfo::default()
            .buffer(*buffer)
            .offset(0)
            .range(*size),
        GpuInput::Image { .. } => vk::DescriptorBufferInfo::default(),
    }];
    let result_infos = [vk::DescriptorBufferInfo::default()
        .buffer(result_buffer)
        .offset(0)
        .range(RESULT_BYTES)];
    let input_write = match input {
        GpuInput::Image { .. } => vk::WriteDescriptorSet::default()
            .dst_set(set)
            .dst_binding(0)
            .descriptor_type(vk::DescriptorType::STORAGE_IMAGE)
            .image_info(&image_infos),
        GpuInput::Buffer { .. } => vk::WriteDescriptorSet::default()
            .dst_set(set)
            .dst_binding(0)
            .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
            .buffer_info(&input_buffer_infos),
    };
    let result_write = vk::WriteDescriptorSet::default()
        .dst_set(set)
        .dst_binding(1)
        .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
        .buffer_info(&result_infos);
    // SAFETY: наборы информации живут до конца вызова, set не используется GPU.
    unsafe { ctx.device.update_descriptor_sets(&[input_write, result_write], &[]) };
    Ok(set)
}

/// Push-константы шейдера в виде байт (u32 little-endian подряд).
fn push_constant_bytes(input: &GpuInput) -> Vec<u8> {
    let words: Vec<u32> = match input {
        GpuInput::Image { width, height, .. } => vec![*width, *height],
        GpuInput::Buffer {
            row_words,
            stride_words,
            rows,
            ..
        } => vec![*row_words, *stride_words, *rows],
    };
    words.iter().flat_map(|word| word.to_ne_bytes()).collect()
}

/// Барьер «забрать ресурс у внешнего владельца (камера/gralloc)» и обратно «вернуть».
/// Образ переводится UNDEFINED→GENERAL (содержимое AHB при передаче от FOREIGN сохраняется).
fn record_ownership_barrier(
    ctx: &VulkanContext,
    cmd: vk::CommandBuffer,
    input: &GpuInput,
    direction: OwnershipDirection,
) {
    let (src_family, dst_family, src_access, dst_access, src_stage, dst_stage, old_layout) = match direction {
        OwnershipDirection::AcquireFromForeign => (
            vk::QUEUE_FAMILY_FOREIGN_EXT,
            ctx.queue_family_index,
            vk::AccessFlags::empty(),
            vk::AccessFlags::SHADER_READ,
            vk::PipelineStageFlags::TOP_OF_PIPE,
            vk::PipelineStageFlags::COMPUTE_SHADER,
            vk::ImageLayout::UNDEFINED,
        ),
        OwnershipDirection::ReleaseToForeign => (
            ctx.queue_family_index,
            vk::QUEUE_FAMILY_FOREIGN_EXT,
            vk::AccessFlags::SHADER_READ,
            vk::AccessFlags::empty(),
            vk::PipelineStageFlags::COMPUTE_SHADER,
            vk::PipelineStageFlags::BOTTOM_OF_PIPE,
            vk::ImageLayout::GENERAL,
        ),
    };
    let image_barriers: Vec<vk::ImageMemoryBarrier<'_>> = match input {
        GpuInput::Image { image, .. } => vec![
            vk::ImageMemoryBarrier::default()
                .src_access_mask(src_access)
                .dst_access_mask(dst_access)
                .old_layout(old_layout)
                .new_layout(vk::ImageLayout::GENERAL)
                .src_queue_family_index(src_family)
                .dst_queue_family_index(dst_family)
                .image(*image)
                .subresource_range(vk::ImageSubresourceRange {
                    aspect_mask: vk::ImageAspectFlags::COLOR,
                    base_mip_level: 0,
                    level_count: 1,
                    base_array_layer: 0,
                    layer_count: 1,
                }),
        ],
        GpuInput::Buffer { .. } => Vec::new(),
    };
    let buffer_barriers: Vec<vk::BufferMemoryBarrier<'_>> = match input {
        GpuInput::Buffer { buffer, .. } => vec![
            vk::BufferMemoryBarrier::default()
                .src_access_mask(src_access)
                .dst_access_mask(dst_access)
                .src_queue_family_index(src_family)
                .dst_queue_family_index(dst_family)
                .buffer(*buffer)
                .offset(0)
                .size(vk::WHOLE_SIZE),
        ],
        GpuInput::Image { .. } => Vec::new(),
    };
    // SAFETY: cmd в состоянии recording; массивы барьеров живут до конца вызова.
    unsafe {
        ctx.device.cmd_pipeline_barrier(
            cmd,
            src_stage,
            dst_stage,
            vk::DependencyFlags::empty(),
            &[],
            &buffer_barriers,
            &image_barriers,
        );
    }
}

#[derive(Clone, Copy)]
enum OwnershipDirection {
    AcquireFromForeign,
    ReleaseToForeign,
}

/// Записывает командный буфер: acquire → timestamp → dispatch → timestamp → release.
fn record_commands(
    ctx: &VulkanContext,
    cmd: vk::CommandBuffer,
    pipeline: &ComputePipeline,
    set: vk::DescriptorSet,
    input: &GpuInput,
    query_pool: Option<vk::QueryPool>,
) -> Result<(), String> {
    let begin_info = vk::CommandBufferBeginInfo::default().flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
    // SAFETY: cmd только что выделен и не записывался.
    vk_step(
        unsafe { ctx.device.begin_command_buffer(cmd, &begin_info) },
        "vkBeginCommandBuffer",
    )?;
    if let Some(pool) = query_pool {
        // SAFETY: cmd в состоянии recording; пул на 2 запроса.
        unsafe { ctx.device.cmd_reset_query_pool(cmd, pool, 0, 2) };
    }
    record_ownership_barrier(ctx, cmd, input, OwnershipDirection::AcquireFromForeign);
    let push_bytes = push_constant_bytes(input);
    // SAFETY: cmd в состоянии recording; pipeline/set/layout принадлежат этому device,
    // push-константы по размеру совпадают с layout.
    unsafe {
        if let Some(pool) = query_pool {
            ctx.device
                .cmd_write_timestamp(cmd, vk::PipelineStageFlags::COMPUTE_SHADER, pool, 0);
        }
        ctx.device
            .cmd_bind_pipeline(cmd, vk::PipelineBindPoint::COMPUTE, pipeline.pipeline);
        ctx.device.cmd_bind_descriptor_sets(
            cmd,
            vk::PipelineBindPoint::COMPUTE,
            pipeline.pipeline_layout,
            0,
            &[set],
            &[],
        );
        ctx.device.cmd_push_constants(
            cmd,
            pipeline.pipeline_layout,
            vk::ShaderStageFlags::COMPUTE,
            0,
            &push_bytes,
        );
        ctx.device.cmd_dispatch(cmd, WORKGROUP_COUNT, 1, 1);
        if let Some(pool) = query_pool {
            ctx.device
                .cmd_write_timestamp(cmd, vk::PipelineStageFlags::BOTTOM_OF_PIPE, pool, 1);
        }
    }
    record_ownership_barrier(ctx, cmd, input, OwnershipDirection::ReleaseToForeign);
    // SAFETY: cmd в состоянии recording.
    vk_step(unsafe { ctx.device.end_command_buffer(cmd) }, "vkEndCommandBuffer")
}

/// Семафоры отправки: что ждём и что сигналим.
pub struct SubmitSync {
    /// Семафор из импортированного acquire-fence (GPU не начнёт читать, пока камера не закончила писать).
    pub wait: Option<vk::Semaphore>,
    /// Семафор, экспортируемый потом в release-fence.
    pub signal: Option<vk::Semaphore>,
}

/// Готовит всё и отправляет работу в очередь. Вызывать под `ctx.lock_queue()`.
pub fn submit(
    ctx: &VulkanContext,
    resources: &mut FrameResources<'_>,
    input: &GpuInput,
    sync: &SubmitSync,
) -> Result<GpuJob, String> {
    let (result_buffer, result_ptr) = create_result_buffer(ctx, resources)?;
    let pipeline = match input {
        GpuInput::Image { .. } => &ctx.image_pipeline,
        GpuInput::Buffer { .. } => &ctx.buffer_pipeline,
    };
    let set = bind_descriptors(ctx, resources, pipeline, input, result_buffer)?;

    let query_pool = if ctx.caps.timestamp_valid_bits > 0 {
        let info = vk::QueryPoolCreateInfo::default()
            .query_type(vk::QueryType::TIMESTAMP)
            .query_count(2);
        // SAFETY: info валиден.
        let pool = vk_step(
            unsafe { ctx.device.create_query_pool(&info, None) },
            "vkCreateQueryPool",
        )?;
        resources.adopt_query_pool(pool);
        Some(pool)
    } else {
        None
    };

    let allocate_info = vk::CommandBufferAllocateInfo::default()
        .command_pool(ctx.command_pool)
        .level(vk::CommandBufferLevel::PRIMARY)
        .command_buffer_count(1);
    // SAFETY: command_pool принадлежит контексту; вызов под lock_queue (пул не потокобезопасен).
    let buffers = vk_step(
        unsafe { ctx.device.allocate_command_buffers(&allocate_info) },
        "vkAllocateCommandBuffers",
    )?;
    let cmd = *buffers.first().ok_or("vkAllocateCommandBuffers: пустой результат")?;
    resources.adopt_command_buffer(cmd);
    record_commands(ctx, cmd, pipeline, set, input, query_pool)?;

    // SAFETY: обычное создание fence.
    let fence = vk_step(
        unsafe { ctx.device.create_fence(&vk::FenceCreateInfo::default(), None) },
        "vkCreateFence",
    )?;
    resources.adopt_fence(fence);

    let wait_semaphores: Vec<vk::Semaphore> = sync.wait.into_iter().collect();
    let wait_stages = vec![vk::PipelineStageFlags::COMPUTE_SHADER; wait_semaphores.len()];
    let signal_semaphores: Vec<vk::Semaphore> = sync.signal.into_iter().collect();
    let command_buffers = [cmd];
    let submit_info = vk::SubmitInfo::default()
        .wait_semaphores(&wait_semaphores)
        .wait_dst_stage_mask(&wait_stages)
        .command_buffers(&command_buffers)
        .signal_semaphores(&signal_semaphores);
    let submitted_at = Instant::now();
    // SAFETY: все объекты принадлежат device; очередь защищена lock_queue у вызывающего.
    vk_step(
        unsafe { ctx.device.queue_submit(ctx.queue, &[submit_info], fence) },
        "vkQueueSubmit",
    )?;
    Ok(GpuJob {
        fence,
        query_pool,
        result: result_ptr,
        submitted_at,
    })
}

impl GpuJob {
    /// Ждёт завершения GPU и читает сумму и время.
    pub fn wait_and_read(self, ctx: &VulkanContext) -> Result<GpuRun, String> {
        let timeout_ns = u64::try_from(GPU_TIMEOUT.as_nanos()).unwrap_or(u64::MAX);
        // SAFETY: fence принадлежит device и отправлена в submit.
        vk_step(
            unsafe { ctx.device.wait_for_fences(&[self.fence], true, timeout_ns) },
            "vkWaitForFences",
        )?;
        let host_elapsed = self.submitted_at.elapsed();
        // SAFETY: указатель на отображённую память результата; после wait_for_fences запись GPU видна хосту.
        let checksum = unsafe { self.result.read_volatile() };
        let (time_us, time_source) = match self.query_pool.map(|pool| self.read_timestamps(ctx, pool)) {
            Some(Ok(microseconds)) => (microseconds, "timestamp_query"),
            Some(Err(error)) => {
                log::warn!("timestamp: {error}; используем host_clock");
                (host_elapsed.as_secs_f64() * 1e6, "host_clock")
            }
            None => (host_elapsed.as_secs_f64() * 1e6, "host_clock"),
        };
        Ok(GpuRun {
            checksum,
            time_us,
            time_source,
        })
    }

    fn read_timestamps(&self, ctx: &VulkanContext, pool: vk::QueryPool) -> Result<f64, String> {
        let mut ticks = [0_u64; 2];
        // SAFETY: пул из submit, оба запроса записаны завершившейся работой.
        vk_step(
            unsafe {
                ctx.device.get_query_pool_results(
                    pool,
                    0,
                    &mut ticks,
                    vk::QueryResultFlags::TYPE_64 | vk::QueryResultFlags::WAIT,
                )
            },
            "vkGetQueryPoolResults",
        )?;
        Ok(ticks_to_microseconds(
            ticks[0],
            ticks[1],
            ctx.caps.timestamp_valid_bits,
            ctx.caps.timestamp_period_ns,
        ))
    }
}

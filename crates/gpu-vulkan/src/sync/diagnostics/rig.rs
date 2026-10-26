//! Стенд диагностики: два compute-пайплайна, общий набор дескрипторов, командные буферы и метки.
//!
//! Один pipeline layout обслуживает обе работы: binding 0 — буфер A, binding 1 — буфер
//! результата, push-константы `{iterations, seed}` (работе B они не нужны, но layout общий).

use ash::vk;

use super::host_buffer::HostBuffer;
use super::timestamps::{TIMESTAMP_COUNT, TimestampPool, slot};
use crate::context::GpuContext;
use crate::error::{GpuError, run_step};
use crate::live_objects::{
    OwnedCommandPool, OwnedDescriptorPool, OwnedDescriptorSetLayout, OwnedPipeline, OwnedPipelineLayout,
};
use crate::shaders::blobs;

/// Размер push-констант: два `u32` (`iterations`, `seed`).
const PUSH_CONSTANT_BYTES: u32 = 8;

/// Командные буферы стенда: по одному на каждую отправку.
#[derive(Debug, Clone, Copy)]
pub(super) struct RigCommandBuffers {
    pub(super) work_a: vk::CommandBuffer,
    pub(super) work_b: vk::CommandBuffer,
    pub(super) work_b_without_wait: vk::CommandBuffer,
}

/// Все объекты диагностики. Поля уничтожаются сверху вниз: пайплайны → layout'ы → пулы → буферы.
pub(super) struct DiagnosticRig {
    busy_pipeline: OwnedPipeline,
    copy_pipeline: OwnedPipeline,
    pipeline_layout: OwnedPipelineLayout,
    _descriptor_pool: OwnedDescriptorPool,
    descriptor_set: vk::DescriptorSet,
    _set_layout: OwnedDescriptorSetLayout,
    _command_pool: OwnedCommandPool,
    pub(super) command_buffers: RigCommandBuffers,
    timestamps: TimestampPool,
    pub(super) work_a_buffer: HostBuffer,
    pub(super) result_buffer: HostBuffer,
    context: GpuContext,
}

impl DiagnosticRig {
    pub(super) fn new(context: &GpuContext) -> Result<Self, GpuError> {
        let device = context.device();

        let bindings: Vec<_> = (0..2_u32)
            .map(|binding| {
                vk::DescriptorSetLayoutBinding::default()
                    .binding(binding)
                    .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                    .descriptor_count(1)
                    .stage_flags(vk::ShaderStageFlags::COMPUTE)
            })
            .collect();
        let set_layout_info = vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings);
        // SAFETY: устройство валидно; структуры локальные.
        let set_layout = OwnedDescriptorSetLayout::from_raw(
            context,
            run_step(
                unsafe { device.create_descriptor_set_layout(&set_layout_info, None) },
                "vkCreateDescriptorSetLayout",
            )?,
        );
        let set_layouts = [set_layout.handle()];
        let push_ranges = [vk::PushConstantRange::default()
            .stage_flags(vk::ShaderStageFlags::COMPUTE)
            .offset(0)
            .size(PUSH_CONSTANT_BYTES)];
        let layout_info = vk::PipelineLayoutCreateInfo::default()
            .set_layouts(&set_layouts)
            .push_constant_ranges(&push_ranges);
        // SAFETY: как выше.
        let pipeline_layout = OwnedPipelineLayout::from_raw(
            context,
            run_step(
                unsafe { device.create_pipeline_layout(&layout_info, None) },
                "vkCreatePipelineLayout",
            )?,
        );

        let busy_pipeline = Self::create_pipeline(context, &pipeline_layout, blobs::FENCE_BUSY_COMP)?;
        let copy_pipeline = Self::create_pipeline(context, &pipeline_layout, blobs::FENCE_COPY_COMP)?;

        let pool_sizes = [vk::DescriptorPoolSize::default()
            .ty(vk::DescriptorType::STORAGE_BUFFER)
            .descriptor_count(2)];
        let descriptor_pool_info = vk::DescriptorPoolCreateInfo::default()
            .max_sets(1)
            .pool_sizes(&pool_sizes);
        // SAFETY: как выше.
        let descriptor_pool = OwnedDescriptorPool::from_raw(
            context,
            run_step(
                unsafe { device.create_descriptor_pool(&descriptor_pool_info, None) },
                "vkCreateDescriptorPool",
            )?,
        );
        let allocate_info = vk::DescriptorSetAllocateInfo::default()
            .descriptor_pool(descriptor_pool.handle())
            .set_layouts(&set_layouts);
        // SAFETY: пул и layout созданы этим устройством.
        let descriptor_set = run_step(
            unsafe { device.allocate_descriptor_sets(&allocate_info) },
            "vkAllocateDescriptorSets",
        )?
        .first()
        .copied()
        .ok_or(GpuError::InvalidState {
            what: "vkAllocateDescriptorSets не вернул набор".to_owned(),
        })?;

        let work_a_buffer = HostBuffer::new(context)?;
        let result_buffer = HostBuffer::new(context)?;
        let buffer_infos = [
            [vk::DescriptorBufferInfo::default()
                .buffer(work_a_buffer.handle())
                .range(work_a_buffer.size())],
            [vk::DescriptorBufferInfo::default()
                .buffer(result_buffer.handle())
                .range(result_buffer.size())],
        ];
        let writes: Vec<_> = buffer_infos
            .iter()
            .enumerate()
            .map(|(binding, info)| {
                vk::WriteDescriptorSet::default()
                    .dst_set(descriptor_set)
                    .dst_binding(binding as u32)
                    .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                    .buffer_info(info)
            })
            .collect();
        // SAFETY: набор не используется GPU; буферы живы.
        unsafe { device.update_descriptor_sets(&writes, &[]) };

        let command_pool_info = vk::CommandPoolCreateInfo::default().queue_family_index(context.queue_family_index());
        // SAFETY: как выше.
        let command_pool = OwnedCommandPool::from_raw(
            context,
            run_step(
                unsafe { device.create_command_pool(&command_pool_info, None) },
                "vkCreateCommandPool",
            )?,
        );
        let buffers_info = vk::CommandBufferAllocateInfo::default()
            .command_pool(command_pool.handle())
            .level(vk::CommandBufferLevel::PRIMARY)
            .command_buffer_count(3);
        // SAFETY: пул только что создан; буферы освободятся вместе с ним.
        let buffers = run_step(
            unsafe { device.allocate_command_buffers(&buffers_info) },
            "vkAllocateCommandBuffers",
        )?;
        let [work_a, work_b, work_b_without_wait] = buffers[..] else {
            return Err(GpuError::InvalidState {
                what: "vkAllocateCommandBuffers вернул не три буфера".to_owned(),
            });
        };

        Ok(Self {
            busy_pipeline,
            copy_pipeline,
            pipeline_layout,
            _descriptor_pool: descriptor_pool,
            descriptor_set,
            _set_layout: set_layout,
            _command_pool: command_pool,
            command_buffers: RigCommandBuffers {
                work_a,
                work_b,
                work_b_without_wait,
            },
            timestamps: TimestampPool::new(context)?,
            work_a_buffer,
            result_buffer,
            context: context.clone(),
        })
    }

    fn create_pipeline(
        context: &GpuContext,
        layout: &OwnedPipelineLayout,
        spirv: &[u8],
    ) -> Result<OwnedPipeline, GpuError> {
        let module = context.create_shader_module(spirv)?;
        let stage = vk::PipelineShaderStageCreateInfo::default()
            .stage(vk::ShaderStageFlags::COMPUTE)
            .module(module.handle())
            .name(c"main");
        let create_info = vk::ComputePipelineCreateInfo::default()
            .stage(stage)
            .layout(layout.handle());
        // SAFETY: модуль и layout живы; кэш не используется.
        let pipelines = unsafe {
            context
                .device()
                .create_compute_pipelines(vk::PipelineCache::null(), &[create_info], None)
        }
        .map_err(|(created, result)| {
            // Частично созданные пайплайны при ошибке нужно уничтожить самим.
            for pipeline in created {
                if pipeline != vk::Pipeline::null() {
                    // SAFETY: пайплайн создан этим устройством и нигде не использовался.
                    unsafe { context.device().destroy_pipeline(pipeline, None) };
                }
            }
            GpuError::Vulkan {
                step: "vkCreateComputePipelines",
                result,
            }
        })?;
        let pipeline = pipelines.first().copied().ok_or(GpuError::InvalidState {
            what: "vkCreateComputePipelines не вернул пайплайн".to_owned(),
        })?;
        // Модуль после создания пайплайна не нужен: он уничтожится при выходе из функции.
        Ok(OwnedPipeline::from_raw(context, pipeline))
    }

    /// Записывает работу A: метка, долгий цикл, метка. Результат цикла — в `work_a_buffer`.
    pub(super) fn record_work_a(&self, iterations: u32, seed: u32) -> Result<(), GpuError> {
        let command_buffer = self.command_buffers.work_a;
        self.begin(command_buffer)?;
        let device = self.context.device();
        // SAFETY: буфер в состоянии записи; пул запросов жив. Сброс — до первой записи меток.
        unsafe {
            device.cmd_reset_query_pool(command_buffer, self.timestamps.handle(), 0, 2);
            device.cmd_write_timestamp2(
                command_buffer,
                vk::PipelineStageFlags2::COMPUTE_SHADER,
                self.timestamps.handle(),
                slot::WORK_A_BEGIN,
            );
        }
        self.record_dispatch(command_buffer, &self.busy_pipeline, [iterations, seed]);
        // SAFETY: как выше.
        unsafe {
            device.cmd_write_timestamp2(
                command_buffer,
                vk::PipelineStageFlags2::COMPUTE_SHADER,
                self.timestamps.handle(),
                slot::WORK_A_END,
            );
        }
        self.end(command_buffer)
    }

    /// Записывает работу B: метка, копирование результата A, метка.
    pub(super) fn record_work_b(&self) -> Result<(), GpuError> {
        let command_buffer = self.command_buffers.work_b;
        self.begin(command_buffer)?;
        let device = self.context.device();
        // SAFETY: буфер в состоянии записи; сбрасываем только свои запросы (2 и 3).
        unsafe {
            device.cmd_reset_query_pool(command_buffer, self.timestamps.handle(), slot::WORK_B_BEGIN, 2);
            device.cmd_write_timestamp2(
                command_buffer,
                vk::PipelineStageFlags2::COMPUTE_SHADER,
                self.timestamps.handle(),
                slot::WORK_B_BEGIN,
            );
        }
        self.record_dispatch(command_buffer, &self.copy_pipeline, [0, 0]);
        // SAFETY: как выше.
        unsafe {
            device.cmd_write_timestamp2(
                command_buffer,
                vk::PipelineStageFlags2::COMPUTE_SHADER,
                self.timestamps.handle(),
                slot::WORK_B_END,
            );
        }
        self.end(command_buffer)
    }

    /// Работа B для фазы «fence уже сигнален»: то же копирование, без меток.
    pub(super) fn record_work_b_without_timestamps(&self) -> Result<(), GpuError> {
        let command_buffer = self.command_buffers.work_b_without_wait;
        self.begin(command_buffer)?;
        self.record_dispatch(command_buffer, &self.copy_pipeline, [0, 0]);
        self.end(command_buffer)
    }

    fn record_dispatch(&self, command_buffer: vk::CommandBuffer, pipeline: &OwnedPipeline, push_values: [u32; 2]) {
        let device = self.context.device();
        let mut push_bytes = [0_u8; PUSH_CONSTANT_BYTES as usize];
        for (chunk, value) in push_bytes.chunks_exact_mut(4).zip(push_values) {
            chunk.copy_from_slice(&value.to_ne_bytes());
        }
        // SAFETY: буфер в состоянии записи; пайплайн, layout и набор живы; push-диапазон
        // (8 байт) совпадает с объявленным в layout.
        unsafe {
            device.cmd_bind_pipeline(command_buffer, vk::PipelineBindPoint::COMPUTE, pipeline.handle());
            device.cmd_bind_descriptor_sets(
                command_buffer,
                vk::PipelineBindPoint::COMPUTE,
                self.pipeline_layout.handle(),
                0,
                &[self.descriptor_set],
                &[],
            );
            device.cmd_push_constants(
                command_buffer,
                self.pipeline_layout.handle(),
                vk::ShaderStageFlags::COMPUTE,
                0,
                &push_bytes,
            );
            device.cmd_dispatch(command_buffer, 1, 1, 1);
        }
    }

    fn begin(&self, command_buffer: vk::CommandBuffer) -> Result<(), GpuError> {
        let begin_info = vk::CommandBufferBeginInfo::default().flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
        // SAFETY: буфер в исходном состоянии и принадлежит только этому стенду.
        run_step(
            unsafe { self.context.device().begin_command_buffer(command_buffer, &begin_info) },
            "vkBeginCommandBuffer",
        )
    }

    fn end(&self, command_buffer: vk::CommandBuffer) -> Result<(), GpuError> {
        // SAFETY: запись шла в этом же потоке.
        run_step(
            unsafe { self.context.device().end_command_buffer(command_buffer) },
            "vkEndCommandBuffer",
        )
    }

    /// Метки времени всех четырёх точек (после завершения работ).
    pub(super) fn read_timestamp_ticks(&self) -> Result<[u64; TIMESTAMP_COUNT as usize], GpuError> {
        self.timestamps.read_ticks()
    }
}

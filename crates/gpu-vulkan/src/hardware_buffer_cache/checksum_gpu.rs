//! Запуск compute-шейдеров контрольных сумм (`ahb_sum_r16.comp`, `ahb_sum_words.comp`) над
//! картинкой `R16_UINT` или буфером и чтение результата на CPU.
//!
//! Модуль общий для ПК и телефона: ему всё равно, откуда память ресурса. На телефоне это импорт
//! `AHardwareBuffer`, на ПК — обычные картинка/буфер в тестах, поэтому сам шейдер, дескрипторы,
//! push-константы и барьеры проверяются на RADV с validation, а на телефоне остаётся проверить
//! только импорт и передачу владения FOREIGN.
//!
//! Результат — пара [`ChecksumPair`] (`sum`, `mix`), совпадающая с CPU-эталоном
//! [`super::raw_reference`] до бита.

use ash::vk;

use super::queue_handoff::{
    BufferAcquire, ImageAcquire, ReleaseTarget, record_buffer_acquire, record_buffer_release, record_image_acquire,
    record_image_release,
};
use super::raw_reference::ChecksumPair;
use crate::context::{GpuContext, MemoryLocation};
use crate::error::{GpuError, run_step};
use crate::live_objects::{
    OwnedBuffer, OwnedDescriptorPool, OwnedDescriptorSetLayout, OwnedDeviceMemory, OwnedPipeline, OwnedPipelineLayout,
};
use crate::shaders::blobs;

/// Параметры запуска. Значения по умолчанию объяснены у полей.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChecksumRunnerConfig {
    /// Число рабочих групп. Шейдеры обходят данные «решёткой» (grid-stride), поэтому число групп
    /// не зависит от размера кадра: 1024 групп по 256 нитей покрывают любую видеокарту, а кадр
    /// 8192×6144 всё равно обрабатывается за несколько итераций цикла.
    pub workgroup_count: u32,
}

impl Default for ChecksumRunnerConfig {
    fn default() -> Self {
        Self { workgroup_count: 1024 }
    }
}

/// Что читает шейдер и как ресурс попадает в очередь и уходит обратно.
#[derive(Debug, Clone, Copy)]
pub enum ChecksumSource {
    /// Картинка `R16_UINT` (путь `ahb_sum_r16.comp`), читается как sampled image через
    /// `texelFetch` (без сэмплера и фильтрации): камера отдаёт AHB только с usage
    /// `GPU_SAMPLED_IMAGE`, поэтому STORAGE-usage недопустим (00 C4).
    SampledImage {
        /// Представление `R16_UINT` с usage `SAMPLED`.
        view: vk::ImageView,
        /// Сама картинка (для барьеров).
        image: vk::Image,
        /// Ширина, текселей.
        width: u32,
        /// Высота, текселей.
        height: u32,
        /// Как забрать картинку перед чтением.
        acquire: ImageAcquire,
        /// Куда отдать после чтения.
        release: ReleaseTarget,
    },
    /// Буфер со словами (путь `ahb_sum_words.comp`), в том числе упакованный RAW10.
    PackedBuffer {
        /// Буфер с usage `STORAGE_BUFFER`.
        buffer: vk::Buffer,
        /// Размер буфера, байт (`range` дескриптора).
        size: u64,
        /// Полезных слов в строке.
        row_words: u32,
        /// Шаг строки в словах.
        stride_words: u32,
        /// Число строк.
        rows: u32,
        /// Как забрать буфер перед чтением.
        acquire: BufferAcquire,
        /// Куда отдать после чтения.
        release: ReleaseTarget,
    },
}

/// Размер буфера результата: два `u32` (`sum`, `mix`).
const RESULT_BYTES: u64 = 8;
/// Число 32-битных слов в push-константах картинки (`width`, `height`).
const IMAGE_PUSH_WORDS: usize = 2;
/// Число слов в push-константах буфера (`row_words`, `stride_words`, `rows`).
const BUFFER_PUSH_WORDS: usize = 3;

/// Один compute-пайплайн с его раскладками. Порядок полей = порядок уничтожения.
struct ComputePipelineSet {
    pipeline: OwnedPipeline,
    pipeline_layout: OwnedPipelineLayout,
    descriptor_set_layout: OwnedDescriptorSetLayout,
}

/// Запускатель контрольных сумм. Создаёт пайплайны один раз; `run` можно звать многократно.
pub struct GpuChecksumRunner {
    image_pipeline: ComputePipelineSet,
    buffer_pipeline: ComputePipelineSet,
    config: ChecksumRunnerConfig,
    context: GpuContext,
}

impl GpuChecksumRunner {
    /// Создаёт пайплайны. Путь картинки читает `R16_UINT` как sampled image, поэтому
    /// `shaderStorageImageExtendedFormats` не нужна.
    pub fn new(context: &GpuContext, config: ChecksumRunnerConfig) -> Result<Self, GpuError> {
        if config.workgroup_count == 0 {
            return Err(GpuError::InvalidArgument {
                what: "workgroup_count = 0".to_owned(),
            });
        }
        let image_pipeline = Self::create_pipeline_set(
            context,
            blobs::AHB_SUM_R16_COMP,
            vk::DescriptorType::SAMPLED_IMAGE,
            IMAGE_PUSH_WORDS,
        )?;
        let buffer_pipeline = Self::create_pipeline_set(
            context,
            blobs::AHB_SUM_WORDS_COMP,
            vk::DescriptorType::STORAGE_BUFFER,
            BUFFER_PUSH_WORDS,
        )?;
        Ok(Self {
            image_pipeline,
            buffer_pipeline,
            config,
            context: context.clone(),
        })
    }

    /// Пайплайн с раскладкой: binding 0 — вход (`input_type`), binding 1 — буфер результата.
    fn create_pipeline_set(
        context: &GpuContext,
        spirv: &[u8],
        input_type: vk::DescriptorType,
        push_words: usize,
    ) -> Result<ComputePipelineSet, GpuError> {
        let device = context.device();
        let bindings = [
            vk::DescriptorSetLayoutBinding::default()
                .binding(0)
                .descriptor_type(input_type)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::COMPUTE),
            vk::DescriptorSetLayoutBinding::default()
                .binding(1)
                .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::COMPUTE),
        ];
        let set_layout_info = vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings);
        // SAFETY: устройство валидно; структуры живут до конца вызова.
        let descriptor_set_layout = OwnedDescriptorSetLayout::from_raw(
            context,
            run_step(
                unsafe { device.create_descriptor_set_layout(&set_layout_info, None) },
                "vkCreateDescriptorSetLayout",
            )?,
        );

        let set_layouts = [descriptor_set_layout.handle()];
        let push_ranges = [vk::PushConstantRange::default()
            .stage_flags(vk::ShaderStageFlags::COMPUTE)
            .offset(0)
            .size((push_words * std::mem::size_of::<u32>()) as u32)];
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

        let module = context.create_shader_module(spirv)?;
        let stage = vk::PipelineShaderStageCreateInfo::default()
            .stage(vk::ShaderStageFlags::COMPUTE)
            .module(module.handle())
            .name(c"main");
        let pipeline_info = vk::ComputePipelineCreateInfo::default()
            .stage(stage)
            .layout(pipeline_layout.handle());
        // SAFETY: модуль и раскладка живы до конца вызова.
        let pipelines = unsafe { device.create_compute_pipelines(context.pipeline_cache(), &[pipeline_info], None) }
            .map_err(|(_, result)| GpuError::Vulkan {
                step: "vkCreateComputePipelines",
                result,
            })?;
        let pipeline = pipelines.first().copied().ok_or(GpuError::InvalidState {
            what: "vkCreateComputePipelines не вернул пайплайн".to_owned(),
        })?;
        Ok(ComputePipelineSet {
            pipeline: OwnedPipeline::from_raw(context, pipeline),
            pipeline_layout,
            descriptor_set_layout,
        })
    }

    /// Считает контрольные суммы источника на GPU и возвращает их. Блокирует до конца работы
    /// (таймаут — из конфигурации контекста).
    pub fn run(&self, source: &ChecksumSource) -> Result<ChecksumPair, GpuError> {
        Self::validate_source(source)?;

        // Буфер результата в host-visible памяти; обнуляем накопители до запуска GPU.
        let result_buffer = self.create_result_buffer()?;
        let mapped = MappedResult::map_and_zero(&self.context, &result_buffer)?;

        let pipelines = match source {
            ChecksumSource::SampledImage { .. } => &self.image_pipeline,
            ChecksumSource::PackedBuffer { .. } => &self.buffer_pipeline,
        };
        let (descriptor_pool, descriptor_set) =
            self.create_descriptor_set(pipelines, source, result_buffer.buffer.handle())?;

        let outcome = self.context.run_one_shot(|command_buffer| {
            self.record_work(
                command_buffer,
                pipelines,
                descriptor_set,
                source,
                result_buffer.buffer.handle(),
            );
            Ok(())
        });
        match outcome {
            // run_one_shot дождался fence: GPU закончил, пул и буфер можно освобождать.
            Ok(()) => Ok(mapped.read()),
            Err(error) => {
                // Состояние GPU неизвестно (таймаут/потеря устройства): уничтожать то, что он мог
                // ещё использовать, нельзя — намеренная утечка, видна в live_objects().
                drop(mapped);
                std::mem::forget(descriptor_pool);
                std::mem::forget(result_buffer);
                Err(error)
            }
        }
    }

    /// Отказывает до обращения к GPU, если источник заведомо неверен (деление на ноль в шейдере и т.п.).
    fn validate_source(source: &ChecksumSource) -> Result<(), GpuError> {
        let invalid = match *source {
            ChecksumSource::SampledImage { width, height, .. } => width == 0 || height == 0,
            ChecksumSource::PackedBuffer {
                row_words,
                stride_words,
                rows,
                size,
                ..
            } => {
                row_words == 0
                    || rows == 0
                    || stride_words < row_words
                    // Все прочитанные слова должны лежать внутри буфера.
                    || (u64::from(rows - 1) * u64::from(stride_words) + u64::from(row_words)) * 4 > size
            }
        };
        if invalid {
            return Err(GpuError::InvalidArgument {
                what: format!("недопустимый источник контрольной суммы: {source:?}"),
            });
        }
        Ok(())
    }

    fn create_result_buffer(&self) -> Result<ResultBuffer, GpuError> {
        let device = self.context.device();
        let buffer_info = vk::BufferCreateInfo::default()
            .size(RESULT_BYTES)
            .usage(vk::BufferUsageFlags::STORAGE_BUFFER)
            .sharing_mode(vk::SharingMode::EXCLUSIVE);
        // SAFETY: устройство валидно; buffer_info — локальная структура.
        let buffer = OwnedBuffer::from_raw(
            &self.context,
            run_step(
                unsafe { device.create_buffer(&buffer_info, None) },
                "vkCreateBuffer(result)",
            )?,
        );
        // SAFETY: буфер только что создан этим устройством.
        let requirements = unsafe { device.get_buffer_memory_requirements(buffer.handle()) };
        let memory = self.context.allocate_memory(
            requirements,
            MemoryLocation::HostVisibleCoherent,
            "буфер результата контрольной суммы",
        )?;
        // SAFETY: буфер и память принадлежат одному устройству; память подобрана по требованиям буфера.
        run_step(
            unsafe { device.bind_buffer_memory(buffer.handle(), memory.handle(), 0) },
            "vkBindBufferMemory(result)",
        )?;
        Ok(ResultBuffer { buffer, memory })
    }

    /// Пул (на один набор) и набор дескрипторов: вход + буфер результата.
    fn create_descriptor_set(
        &self,
        pipelines: &ComputePipelineSet,
        source: &ChecksumSource,
        result_buffer: vk::Buffer,
    ) -> Result<(OwnedDescriptorPool, vk::DescriptorSet), GpuError> {
        let device = self.context.device();
        let input_type = match source {
            ChecksumSource::SampledImage { .. } => vk::DescriptorType::SAMPLED_IMAGE,
            ChecksumSource::PackedBuffer { .. } => vk::DescriptorType::STORAGE_BUFFER,
        };
        let pool_sizes = [
            vk::DescriptorPoolSize::default().ty(input_type).descriptor_count(1),
            vk::DescriptorPoolSize::default()
                .ty(vk::DescriptorType::STORAGE_BUFFER)
                .descriptor_count(1),
        ];
        // Если вход тоже буфер, тип совпадает — описываем одним размером на 2 дескриптора.
        let merged_sizes = [vk::DescriptorPoolSize::default()
            .ty(vk::DescriptorType::STORAGE_BUFFER)
            .descriptor_count(2)];
        let sizes: &[vk::DescriptorPoolSize] = if input_type == vk::DescriptorType::STORAGE_BUFFER {
            &merged_sizes
        } else {
            &pool_sizes
        };
        let pool_info = vk::DescriptorPoolCreateInfo::default().max_sets(1).pool_sizes(sizes);
        // SAFETY: устройство валидно; структуры локальные.
        let pool = OwnedDescriptorPool::from_raw(
            &self.context,
            run_step(
                unsafe { device.create_descriptor_pool(&pool_info, None) },
                "vkCreateDescriptorPool",
            )?,
        );
        let set_layouts = [pipelines.descriptor_set_layout.handle()];
        let allocate_info = vk::DescriptorSetAllocateInfo::default()
            .descriptor_pool(pool.handle())
            .set_layouts(&set_layouts);
        // SAFETY: пул и раскладка принадлежат этому устройству.
        let sets = run_step(
            unsafe { device.allocate_descriptor_sets(&allocate_info) },
            "vkAllocateDescriptorSets",
        )?;
        let set = sets.first().copied().ok_or(GpuError::InvalidState {
            what: "vkAllocateDescriptorSets не вернул набор".to_owned(),
        })?;

        let image_infos;
        let input_buffer_infos;
        let input_write = match *source {
            ChecksumSource::SampledImage { view, .. } => {
                image_infos = [vk::DescriptorImageInfo::default()
                    .image_view(view)
                    .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
                vk::WriteDescriptorSet::default()
                    .dst_set(set)
                    .dst_binding(0)
                    .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                    .image_info(&image_infos)
            }
            ChecksumSource::PackedBuffer { buffer, size, .. } => {
                input_buffer_infos = [vk::DescriptorBufferInfo::default().buffer(buffer).offset(0).range(size)];
                vk::WriteDescriptorSet::default()
                    .dst_set(set)
                    .dst_binding(0)
                    .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                    .buffer_info(&input_buffer_infos)
            }
        };
        let result_infos = [vk::DescriptorBufferInfo::default()
            .buffer(result_buffer)
            .offset(0)
            .range(RESULT_BYTES)];
        let result_write = vk::WriteDescriptorSet::default()
            .dst_set(set)
            .dst_binding(1)
            .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
            .buffer_info(&result_infos);
        // SAFETY: набор свободен (GPU его ещё не использует); структуры живут до конца вызова.
        unsafe { device.update_descriptor_sets(&[input_write, result_write], &[]) };
        Ok((pool, set))
    }

    /// Записывает: acquire → dispatch → барьер результата к host → release.
    fn record_work(
        &self,
        command_buffer: vk::CommandBuffer,
        pipelines: &ComputePipelineSet,
        descriptor_set: vk::DescriptorSet,
        source: &ChecksumSource,
        result_buffer: vk::Buffer,
    ) {
        let device = self.context.device();
        let family = self.context.queue_family_index();
        match *source {
            ChecksumSource::SampledImage { image, acquire, .. } => {
                record_image_acquire(device, command_buffer, image, acquire, family);
            }
            ChecksumSource::PackedBuffer { buffer, acquire, .. } => {
                record_buffer_acquire(device, command_buffer, buffer, acquire, family);
            }
        }

        let push_words: Vec<u32> = match *source {
            ChecksumSource::SampledImage { width, height, .. } => vec![width, height],
            ChecksumSource::PackedBuffer {
                row_words,
                stride_words,
                rows,
                ..
            } => vec![row_words, stride_words, rows],
        };
        let push_bytes: Vec<u8> = push_words.iter().flat_map(|word| word.to_ne_bytes()).collect();
        // SAFETY: командный буфер в состоянии recording; пайплайн, раскладка и набор принадлежат
        // этому устройству; размер push-констант равен диапазону раскладки.
        unsafe {
            device.cmd_bind_pipeline(
                command_buffer,
                vk::PipelineBindPoint::COMPUTE,
                pipelines.pipeline.handle(),
            );
            device.cmd_bind_descriptor_sets(
                command_buffer,
                vk::PipelineBindPoint::COMPUTE,
                pipelines.pipeline_layout.handle(),
                0,
                &[descriptor_set],
                &[],
            );
            device.cmd_push_constants(
                command_buffer,
                pipelines.pipeline_layout.handle(),
                vk::ShaderStageFlags::COMPUTE,
                0,
                &push_bytes,
            );
            device.cmd_dispatch(command_buffer, self.config.workgroup_count, 1, 1);
        }

        // Атомарные записи шейдера → видимость для чтения на CPU после fence.
        let result_barrier = vk::BufferMemoryBarrier2::default()
            .src_stage_mask(vk::PipelineStageFlags2::COMPUTE_SHADER)
            .src_access_mask(vk::AccessFlags2::SHADER_STORAGE_WRITE)
            .dst_stage_mask(vk::PipelineStageFlags2::HOST)
            .dst_access_mask(vk::AccessFlags2::HOST_READ)
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .buffer(result_buffer)
            .offset(0)
            .size(vk::WHOLE_SIZE);
        let barriers = [result_barrier];
        let dependency = vk::DependencyInfo::default().buffer_memory_barriers(&barriers);
        // SAFETY: командный буфер в состоянии recording.
        unsafe { device.cmd_pipeline_barrier2(command_buffer, &dependency) };

        match *source {
            ChecksumSource::SampledImage { image, release, .. } => {
                record_image_release(device, command_buffer, image, release, family);
            }
            ChecksumSource::PackedBuffer { buffer, release, .. } => {
                record_buffer_release(device, command_buffer, buffer, release, family);
            }
        }
    }
}

/// Буфер результата с памятью. Буфер объявлен раньше памяти → уничтожается раньше.
struct ResultBuffer {
    buffer: OwnedBuffer,
    memory: OwnedDeviceMemory,
}

/// Отображённая память результата: размапливается в `Drop`.
struct MappedResult<'buffer> {
    context: GpuContext,
    memory: vk::DeviceMemory,
    address: *const u32,
    _owner: &'buffer ResultBuffer,
}

impl<'buffer> MappedResult<'buffer> {
    /// Отображает память и обнуляет оба накопителя.
    fn map_and_zero(context: &GpuContext, result: &'buffer ResultBuffer) -> Result<Self, GpuError> {
        // SAFETY: память host-visible, создана этим устройством и сейчас не отображена.
        let address = run_step(
            unsafe {
                context
                    .device()
                    .map_memory(result.memory.handle(), 0, RESULT_BYTES, vk::MemoryMapFlags::empty())
            },
            "vkMapMemory(result)",
        )?;
        let address = address.cast::<u32>();
        // SAFETY: отображено RESULT_BYTES = 8 байт, выровнено (map_memory даёт выравнивание не меньше
        // minMemoryMapAlignment ≥ 64); GPU ещё не запущен.
        unsafe {
            address.write_volatile(0);
            address.add(1).write_volatile(0);
        }
        Ok(Self {
            context: context.clone(),
            memory: result.memory.handle(),
            address,
            _owner: result,
        })
    }

    /// Читает накопители. Вызывать после завершения GPU.
    fn read(&self) -> ChecksumPair {
        // SAFETY: память отображена на RESULT_BYTES байт, GPU закончил (fence), память когерентна.
        unsafe {
            ChecksumPair {
                sum: self.address.read_volatile(),
                mix: self.address.add(1).read_volatile(),
            }
        }
    }
}

impl Drop for MappedResult<'_> {
    fn drop(&mut self) {
        // SAFETY: память была отображена в `map_and_zero` и жива (`_owner` держит буфер результата).
        unsafe { self.context.device().unmap_memory(self.memory) };
    }
}

#[cfg(test)]
mod tests;

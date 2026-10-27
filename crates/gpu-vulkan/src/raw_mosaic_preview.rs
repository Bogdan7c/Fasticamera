//! `RawMosaicPreview` — отладочный предпросмотр RAW-кадра на экране (сессия 05).
//!
//! Это НЕ ISP: без баланса белого, демозаики и гаммы. Кадр показывается «как есть»: ячейка CFA 2×2
//! превращается в один цвет (R, среднее двух G, B) после вычитания чёрного и деления на размах.
//! Цель — доказать, что кадр камеры дошёл до экрана без копий и стоит правильной стороной.
//! Настоящий ISP (`isp-vulkan`) придёт в сессиях 06+ и заменит этот модуль на экране.
//!
//! # Что внутри
//! * два фрагментных пайплайна (по одному на кодировку): RAW16 читается как sampled `R16_UINT`
//!   (`texelFetch`), RAW10 — как `readonly buffer` слов с распаковкой в шейдере;
//! * по одному набору дескрипторов НА СЛОТ кольца кадров: набор переписывается перед записью кадра
//!   слота, а слот снова выдаётся только после fence слота — GPU набор уже не читает;
//! * барьеры передачи владения с очередью `FOREIGN` — только если источник отмечен как камерный
//!   (`ImageAcquire::FromForeignQueue` и т.п.); на ПК источники `KeepOwned`.
//!
//! Запись идёт в ПЕРЕДАННЫЙ командный буфер и цель (раскладка цели — `COLOR_ATTACHMENT_OPTIMAL`,
//! её переводит владелец цели). Барьеры владения записываются ВНЕ динамического рендеринга
//! (до `cmd_begin_rendering` и после `cmd_end_rendering`).

mod compute_source;
pub mod params;
pub mod reference;
#[cfg(any(test, feature = "test-support"))]
pub mod synthetic;

use ash::vk;

pub use self::params::{CfaChannel, CfaOrder, MosaicPreviewParams, MosaicSource, QuarterTurns, ValidArea};
use crate::context::GpuContext;
use crate::error::{GpuError, run_step};
use crate::hardware_buffer_cache::queue_handoff::{
    ConsumerStage, record_buffer_acquire_for_stage, record_buffer_release_for_stage, record_image_acquire_for_stage,
    record_image_release_for_stage,
};
use crate::live_objects::{OwnedDescriptorPool, OwnedDescriptorSetLayout, OwnedPipeline, OwnedPipelineLayout};
use crate::shaders::blobs;
use crate::test_pattern::{PatternTarget, SurfaceTransform};

/// Число 32-битных слов push-констант (см. `MosaicParameters` в `raw_mosaic_params.glsl`).
const PUSH_CONSTANT_WORDS: usize = 15;
/// Размер push-констант, байт.
const PUSH_CONSTANT_BYTES: u32 = (PUSH_CONSTANT_WORDS * std::mem::size_of::<u32>()) as u32;

/// Нужно ли забрать источник у внешнего владельца (камеры) перед чтением в ЭТОМ кадре.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceAcquire {
    /// Первое чтение нового кадра: записать barrier приёма владения (по `acquire` источника).
    BeforeRead,
    /// Источник уже наш (перерисовка показанного кадра): никаких барьеров владения и смены раскладки,
    /// чтение после чтения. Повторный приём из `UNDEFINED` гонялся бы с чтением соседнего сабмита.
    AlreadyAcquired,
}

/// Передача владения источниками внешнему владельцу в одном кадре.
///
/// Владение принимается РОВНО ОДИН раз (в сабмите, который первым читает кадр) и возвращается РОВНО
/// ОДИН раз (в сабмите, который заменяет кадр): барьер возврата входит в сабмит, fence которого потом
/// экспортируется, поэтому fence покрывает и его.
#[derive(Debug, Clone, Copy)]
pub struct FrameHandoff {
    /// Принимать ли рисуемый источник.
    pub acquire_source: SourceAcquire,
    /// Источник прежнего показанного кадра, который этот сабмит возвращает внешнему владельцу.
    pub release_previous: Option<MosaicSource>,
}

impl FrameHandoff {
    /// Первое чтение нового кадра: приём владения, возвращать нечего.
    pub const fn first_read() -> Self {
        Self {
            acquire_source: SourceAcquire::BeforeRead,
            release_previous: None,
        }
    }

    /// Перерисовка уже принятого кадра: барьеров владения нет.
    pub const fn redraw() -> Self {
        Self {
            acquire_source: SourceAcquire::AlreadyAcquired,
            release_previous: None,
        }
    }
}

/// Всё, что нужно записать один кадр предпросмотра.
#[derive(Debug, Clone, Copy)]
pub struct PreviewFrame<'a> {
    /// Откуда читать кадр.
    pub source: &'a MosaicSource,
    /// Параметры кадра.
    pub params: &'a MosaicPreviewParams,
    /// Поворот дисплея.
    pub transform: SurfaceTransform,
    /// Слот кольца кадров (`DisplayFrame::ring_slot`).
    pub frame_slot: usize,
    /// Передача владения внешним ресурсам в этом кадре.
    pub handoff: FrameHandoff,
}

/// Записывает приём источника у внешнего владельца (камеры) перед чтением стадией `stage`.
pub(crate) fn record_acquire(
    device: &ash::Device,
    command_buffer: vk::CommandBuffer,
    queue_family: u32,
    source: &MosaicSource,
    stage: ConsumerStage,
) {
    match *source {
        MosaicSource::SampledR16 { image, acquire, .. } => {
            record_image_acquire_for_stage(device, command_buffer, image, acquire, queue_family, stage)
        }
        MosaicSource::PackedRaw10 { buffer, acquire, .. } => {
            record_buffer_acquire_for_stage(device, command_buffer, buffer, acquire, queue_family, stage)
        }
    }
}

/// Записывает возврат источника внешнему владельцу (камере); читала его стадия `stage`.
pub(crate) fn record_release(
    device: &ash::Device,
    command_buffer: vk::CommandBuffer,
    queue_family: u32,
    source: &MosaicSource,
    stage: ConsumerStage,
) {
    match *source {
        MosaicSource::SampledR16 { image, release, .. } => {
            record_image_release_for_stage(device, command_buffer, image, release, queue_family, stage)
        }
        MosaicSource::PackedRaw10 { buffer, release, .. } => {
            record_buffer_release_for_stage(device, command_buffer, buffer, release, queue_family, stage)
        }
    }
}

/// Всё для одной кодировки: пайплайн, раскладки и наборы дескрипторов по слотам.
/// Порядок полей = порядок уничтожения (пайплайн → layout → раскладка набора → пул с наборами).
struct EncodingPath {
    pipeline: OwnedPipeline,
    pipeline_layout: OwnedPipelineLayout,
    _set_layout: OwnedDescriptorSetLayout,
    _pool: OwnedDescriptorPool,
    /// По одному набору на слот кольца кадров.
    sets: Vec<vk::DescriptorSet>,
}

/// Отладочный предпросмотр RAW-мозаики под один формат цели.
pub struct RawMosaicPreview {
    raw16: EncodingPath,
    raw10: EncodingPath,
    slot_count: usize,
    context: GpuContext,
}

impl RawMosaicPreview {
    /// Создаёт пайплайны под формат цели и наборы дескрипторов на `frame_slots` слотов кольца.
    pub fn new(context: &GpuContext, color_format: vk::Format, frame_slots: usize) -> Result<Self, GpuError> {
        if frame_slots == 0 {
            return Err(GpuError::InvalidArgument {
                what: "RawMosaicPreview: число слотов кадров = 0".to_owned(),
            });
        }
        let raw16 = EncodingPath::new(
            context,
            color_format,
            blobs::RAW_MOSAIC_PREVIEW_RAW16_FRAG,
            vk::DescriptorType::SAMPLED_IMAGE,
            frame_slots,
        )?;
        let raw10 = EncodingPath::new(
            context,
            color_format,
            blobs::RAW_MOSAIC_PREVIEW_RAW10_FRAG,
            vk::DescriptorType::STORAGE_BUFFER,
            frame_slots,
        )?;
        Ok(Self {
            raw16,
            raw10,
            slot_count: frame_slots,
            context: context.clone(),
        })
    }

    /// Число слотов, на которые рассчитаны наборы дескрипторов.
    pub fn frame_slots(&self) -> usize {
        self.slot_count
    }

    /// Записывает отрисовку кадра камеры в `command_buffer`.
    ///
    /// `frame.frame_slot` — номер слота кольца кадров (`DisplayFrame::ring_slot`): набор дескрипторов этого
    /// слота переписывается здесь, поэтому слот обязан быть свободен от GPU (fence слота сигнален).
    /// Все проверки идут ДО записи команд: при ошибке буфер остаётся нетронутым.
    pub fn record(
        &mut self,
        command_buffer: vk::CommandBuffer,
        target: PatternTarget,
        frame: &PreviewFrame<'_>,
    ) -> Result<(), GpuError> {
        let PreviewFrame {
            source,
            params,
            transform,
            frame_slot,
            handoff,
        } = *frame;
        params::validate(source, params)?;
        let display_turns = transform.quarter_turns()?;
        Self::validate_target(target)?;
        if frame_slot >= self.slot_count {
            return Err(GpuError::InvalidArgument {
                what: format!("слот кадра {frame_slot} вне 0..{}", self.slot_count),
            });
        }
        let path = match source {
            MosaicSource::SampledR16 { .. } => &self.raw16,
            MosaicSource::PackedRaw10 { .. } => &self.raw10,
        };
        let descriptor_set = path.sets[frame_slot];
        self.write_descriptor(descriptor_set, source);

        let push_bytes = push_constants(target, source, params, display_turns);
        let device = self.context.device();
        let queue_family = self.context.queue_family_index();
        // Владение передаём ВНЕ рендеринга (барьеры внутри него недопустимы): сначала возвращаем прежний
        // кадр (первая область барьера включает все более ранние команды очереди, то есть чтение его
        // прошлыми сабмитами), затем забираем новый — и только в первом сабмите, читающем новый кадр.
        if let Some(previous) = &handoff.release_previous {
            record_release(device, command_buffer, queue_family, previous, ConsumerStage::Fragment);
        }
        if handoff.acquire_source == SourceAcquire::BeforeRead {
            record_acquire(device, command_buffer, queue_family, source, ConsumerStage::Fragment);
        }
        // SAFETY: командный буфер в состоянии записи (контракт вызывающего); пайплайн, layout и набор
        // принадлежат self; push-константы укладываются в объявленный диапазон FRAGMENT.
        unsafe {
            begin_rendering(device, command_buffer, target);
            device.cmd_bind_pipeline(command_buffer, vk::PipelineBindPoint::GRAPHICS, path.pipeline.handle());
            device.cmd_bind_descriptor_sets(
                command_buffer,
                vk::PipelineBindPoint::GRAPHICS,
                path.pipeline_layout.handle(),
                0,
                &[descriptor_set],
                &[],
            );
            device.cmd_push_constants(
                command_buffer,
                path.pipeline_layout.handle(),
                vk::ShaderStageFlags::FRAGMENT,
                0,
                &push_bytes,
            );
            device.cmd_draw(command_buffer, 3, 1, 0, 0);
            device.cmd_end_rendering(command_buffer);
        }
        // Возврат нового кадра камере здесь НЕ записывается: он будет в сабмите, который заменит кадр.
        Ok(())
    }

    /// Записывает чёрный кадр (потока нет): очистка цели без шейдера. `release_previous` — прежний показанный
    /// кадр, который этот сабмит возвращает внешнему владельцу.
    pub fn record_black(
        &self,
        command_buffer: vk::CommandBuffer,
        target: PatternTarget,
        release_previous: Option<MosaicSource>,
    ) -> Result<(), GpuError> {
        Self::validate_target(target)?;
        let device = self.context.device();
        if let Some(previous) = &release_previous {
            record_release(
                device,
                command_buffer,
                self.context.queue_family_index(),
                previous,
                ConsumerStage::Fragment,
            );
        }
        // SAFETY: командный буфер в состоянии записи; рендеринг только очищает вложение.
        unsafe {
            begin_rendering(device, command_buffer, target);
            device.cmd_end_rendering(command_buffer);
        }
        Ok(())
    }

    /// Возвращает источник внешнему владельцу отдельной блокирующей отправкой (ограничена таймаутом
    /// контекста). Для остановки показа: кадр на экране остаётся нашим, пока его не заменил другой.
    pub fn release_source_blocking(context: &GpuContext, source: &MosaicSource) -> Result<(), GpuError> {
        let queue_family = context.queue_family_index();
        context.run_one_shot(|command_buffer| {
            record_release(
                context.device(),
                command_buffer,
                queue_family,
                source,
                ConsumerStage::Fragment,
            );
            Ok(())
        })
    }

    fn validate_target(target: PatternTarget) -> Result<(), GpuError> {
        if target.extent.width == 0 || target.extent.height == 0 {
            return Err(GpuError::InvalidArgument {
                what: "нулевой размер цели предпросмотра RAW".to_owned(),
            });
        }
        Ok(())
    }

    /// Переписывает дескриптор набора слота на ресурс источника.
    fn write_descriptor(&self, set: vk::DescriptorSet, source: &MosaicSource) {
        let image_infos;
        let buffer_infos;
        let write = match *source {
            MosaicSource::SampledR16 { view, .. } => {
                image_infos = [vk::DescriptorImageInfo::default()
                    .image_view(view)
                    .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
                vk::WriteDescriptorSet::default()
                    .dst_set(set)
                    .dst_binding(0)
                    .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                    .image_info(&image_infos)
            }
            MosaicSource::PackedRaw10 { buffer, size_bytes, .. } => {
                buffer_infos = [vk::DescriptorBufferInfo::default()
                    .buffer(buffer)
                    .offset(0)
                    .range(size_bytes)];
                vk::WriteDescriptorSet::default()
                    .dst_set(set)
                    .dst_binding(0)
                    .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                    .buffer_info(&buffer_infos)
            }
        };
        // SAFETY: набор принадлежит слоту, свободному от GPU (контракт `record`); структуры живы до конца вызова.
        unsafe { self.context.device().update_descriptor_sets(&[write], &[]) };
    }
}

/// Начинает динамический рендеринг с очисткой цели в чёрный (вне кадра камеры — letterbox тоже чёрный).
///
/// # Safety
/// `command_buffer` в состоянии записи, вне другого рендеринга; цель в `COLOR_ATTACHMENT_OPTIMAL`.
unsafe fn begin_rendering(device: &ash::Device, command_buffer: vk::CommandBuffer, target: PatternTarget) {
    let color_attachments = [vk::RenderingAttachmentInfo::default()
        .image_view(target.view)
        .image_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
        .load_op(vk::AttachmentLoadOp::CLEAR)
        .store_op(vk::AttachmentStoreOp::STORE)
        .clear_value(vk::ClearValue {
            color: vk::ClearColorValue {
                float32: [0.0, 0.0, 0.0, 1.0],
            },
        })];
    let render_area = vk::Rect2D {
        offset: vk::Offset2D { x: 0, y: 0 },
        extent: target.extent,
    };
    let rendering_info = vk::RenderingInfo::default()
        .render_area(render_area)
        .layer_count(1)
        .color_attachments(&color_attachments);
    let viewport = vk::Viewport {
        x: 0.0,
        y: 0.0,
        width: target.extent.width as f32,
        height: target.extent.height as f32,
        min_depth: 0.0,
        max_depth: 1.0,
    };
    // SAFETY: см. контракт функции.
    unsafe {
        device.cmd_begin_rendering(command_buffer, &rendering_info);
        device.cmd_set_viewport(command_buffer, 0, &[viewport]);
        device.cmd_set_scissor(command_buffer, 0, &[render_area]);
    }
}

/// Байты push-констант в порядке полей `MosaicParameters`.
fn push_constants(
    target: PatternTarget,
    source: &MosaicSource,
    params: &MosaicPreviewParams,
    display_turns: u32,
) -> Vec<u8> {
    let row_stride_bytes = match *source {
        MosaicSource::PackedRaw10 { row_stride_bytes, .. } => row_stride_bytes,
        MosaicSource::SampledR16 { .. } => 0,
    };
    let area = params.valid_area;
    let words: [u32; PUSH_CONSTANT_WORDS] = [
        target.extent.width,
        target.extent.height,
        display_turns,
        params.sensor_rotation.count(),
        area.left,
        area.top,
        area.width,
        area.height,
        params.cfa.shader_code(),
        params.white_level,
        params.black_levels[0],
        params.black_levels[1],
        params.black_levels[2],
        params.black_levels[3],
        row_stride_bytes,
    ];
    words.iter().flat_map(|word| word.to_ne_bytes()).collect()
}

impl EncodingPath {
    fn new(
        context: &GpuContext,
        color_format: vk::Format,
        fragment_spirv: &[u8],
        input_type: vk::DescriptorType,
        frame_slots: usize,
    ) -> Result<Self, GpuError> {
        let device = context.device();
        let bindings = [vk::DescriptorSetLayoutBinding::default()
            .binding(0)
            .descriptor_type(input_type)
            .descriptor_count(1)
            .stage_flags(vk::ShaderStageFlags::FRAGMENT)];
        let set_layout_info = vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings);
        // SAFETY: устройство валидно; структуры живут до конца вызова.
        let set_layout = OwnedDescriptorSetLayout::from_raw(
            context,
            run_step(
                unsafe { device.create_descriptor_set_layout(&set_layout_info, None) },
                "vkCreateDescriptorSetLayout(предпросмотр RAW)",
            )?,
        );
        let set_layouts = [set_layout.handle()];
        let push_ranges = [vk::PushConstantRange::default()
            .stage_flags(vk::ShaderStageFlags::FRAGMENT)
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
                "vkCreatePipelineLayout(предпросмотр RAW)",
            )?,
        );
        let pipeline = create_pipeline(
            context,
            color_format,
            fragment_spirv,
            &pipeline_layout,
            "vkCreateGraphicsPipelines(предпросмотр RAW)",
        )?;

        let slot_count = u32::try_from(frame_slots).map_err(|_| GpuError::InvalidArgument {
            what: "число слотов кадров не помещается в u32".to_owned(),
        })?;
        let pool_sizes = [vk::DescriptorPoolSize::default()
            .ty(input_type)
            .descriptor_count(slot_count)];
        let pool_info = vk::DescriptorPoolCreateInfo::default()
            .max_sets(slot_count)
            .pool_sizes(&pool_sizes);
        // SAFETY: устройство валидно; структуры локальные.
        let pool = OwnedDescriptorPool::from_raw(
            context,
            run_step(
                unsafe { device.create_descriptor_pool(&pool_info, None) },
                "vkCreateDescriptorPool(предпросмотр RAW)",
            )?,
        );
        let all_layouts = vec![set_layout.handle(); frame_slots];
        let allocate_info = vk::DescriptorSetAllocateInfo::default()
            .descriptor_pool(pool.handle())
            .set_layouts(&all_layouts);
        // SAFETY: пул и раскладка принадлежат этому устройству; наборы освободятся вместе с пулом.
        let sets = run_step(
            unsafe { device.allocate_descriptor_sets(&allocate_info) },
            "vkAllocateDescriptorSets(предпросмотр RAW)",
        )?;
        Ok(Self {
            pipeline,
            pipeline_layout,
            _set_layout: set_layout,
            _pool: pool,
            sets,
        })
    }
}

/// Графический пайплайн (dynamic rendering, полноэкранный треугольник) под формат цели.
/// Общий с компоновщиком картинок (`crate::compositor`): тот же треугольник, другой фрагментный шейдер.
pub(crate) fn create_pipeline(
    context: &GpuContext,
    color_format: vk::Format,
    fragment_spirv: &[u8],
    layout: &OwnedPipelineLayout,
    step: &'static str,
) -> Result<OwnedPipeline, GpuError> {
    let device = context.device();
    let vertex_module = context.create_shader_module(blobs::FULLSCREEN_TRIANGLE_VERT)?;
    let fragment_module = context.create_shader_module(fragment_spirv)?;
    let stages = [
        vk::PipelineShaderStageCreateInfo::default()
            .stage(vk::ShaderStageFlags::VERTEX)
            .module(vertex_module.handle())
            .name(c"main"),
        vk::PipelineShaderStageCreateInfo::default()
            .stage(vk::ShaderStageFlags::FRAGMENT)
            .module(fragment_module.handle())
            .name(c"main"),
    ];
    let vertex_input = vk::PipelineVertexInputStateCreateInfo::default();
    let input_assembly =
        vk::PipelineInputAssemblyStateCreateInfo::default().topology(vk::PrimitiveTopology::TRIANGLE_LIST);
    let viewport_state = vk::PipelineViewportStateCreateInfo::default()
        .viewport_count(1)
        .scissor_count(1);
    let rasterization = vk::PipelineRasterizationStateCreateInfo::default()
        .polygon_mode(vk::PolygonMode::FILL)
        .cull_mode(vk::CullModeFlags::NONE)
        .front_face(vk::FrontFace::COUNTER_CLOCKWISE)
        .line_width(1.0);
    let multisample =
        vk::PipelineMultisampleStateCreateInfo::default().rasterization_samples(vk::SampleCountFlags::TYPE_1);
    let blend_attachments = [vk::PipelineColorBlendAttachmentState::default()
        .color_write_mask(vk::ColorComponentFlags::RGBA)
        .blend_enable(false)];
    let color_blend = vk::PipelineColorBlendStateCreateInfo::default().attachments(&blend_attachments);
    let dynamic_states = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
    let dynamic_state = vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dynamic_states);
    let color_formats = [color_format];
    let mut rendering_info = vk::PipelineRenderingCreateInfo::default().color_attachment_formats(&color_formats);
    let pipeline_info = vk::GraphicsPipelineCreateInfo::default()
        .stages(&stages)
        .vertex_input_state(&vertex_input)
        .input_assembly_state(&input_assembly)
        .viewport_state(&viewport_state)
        .rasterization_state(&rasterization)
        .multisample_state(&multisample)
        .color_blend_state(&color_blend)
        .dynamic_state(&dynamic_state)
        .layout(layout.handle())
        .push_next(&mut rendering_info);
    // SAFETY: все структуры и модули живут до конца вызова; кэша пайплайнов нет.
    let pipelines = unsafe { device.create_graphics_pipelines(vk::PipelineCache::null(), &[pipeline_info], None) }
        .map_err(|(_, result)| GpuError::Vulkan { step, result })?;
    let handle = pipelines.first().copied().ok_or(GpuError::InvalidState {
        what: "vkCreateGraphicsPipelines не вернул пайплайн".to_owned(),
    })?;
    Ok(OwnedPipeline::from_raw(context, handle))
}

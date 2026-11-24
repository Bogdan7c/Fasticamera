//! `TestPattern` — графический пайплайн (dynamic rendering) «градиент + метка кадра».
//!
//! Назначение: эталонная картинка для паритетных тестов и smoke на телефоне. Цвет пикселя
//! считается из целых координат и номера кадра; CPU-функции [`reference`] дают те же значения
//! бит-в-бит (шейдер — `shaders/test_pattern.frag`).
//!
//! Точность по форматам цели (см. [`reference::ColorDepth`]):
//! * UNORM8 (`R8G8B8A8_UNORM`, `B8G8R8A8_UNORM`): значение `v/255` возвращается ровно в `v` —
//!   readback сравнивается побитно, допуска нет;
//! * UNORM10 (`A2B10G10R10_UNORM_PACK32`): значения `v/1023`, также без допуска, но сравнивать
//!   нужно «уровни» (`expected_levels`), а не 8-битные байты.
//!
//! Поворот `SurfaceTransform` передаётся push-константой: картинка встаёт прямо после того,
//! как дисплей применит `preTransform`.
//!
//! Запись идёт в ПЕРЕДАННЫЙ командный буфер и ПЕРЕДАННУЮ цель; переходы раскладки картинки
//! (в `COLOR_ATTACHMENT_OPTIMAL` до и обратно после) — ответственность владельца цели.

pub mod reference;
pub mod transform;

use ash::vk;

pub use self::reference::{ColorDepth, expected_levels, expected_pixel, expected_pixel_with_transform};
pub use self::transform::SurfaceTransform;
use crate::context::GpuContext;
use crate::error::{GpuError, run_step};
use crate::live_objects::{OwnedPipeline, OwnedPipelineLayout};
use crate::shaders::blobs;

/// Размер push-констант: пять `u32` (ширина, высота, кадр, четверти оборота, levels).
const PUSH_CONSTANT_BYTES: u32 = 5 * 4;

/// Цель отрисовки: готовое представление картинки и её физический размер.
#[derive(Debug, Clone, Copy)]
pub struct PatternTarget {
    /// Представление картинки (формат — тот же, с которым создан `TestPattern`).
    pub view: vk::ImageView,
    /// Физический размер картинки (для swapchain с поворотом — уже с учётом обмена сторон).
    pub extent: vk::Extent2D,
}

/// Пайплайн тестового паттерна под один формат цели.
pub struct TestPattern {
    // Порядок полей: пайплайн уничтожается раньше своего layout.
    pipeline: OwnedPipeline,
    layout: OwnedPipelineLayout,
    color_depth: ColorDepth,
    color_format: vk::Format,
    context: GpuContext,
}

impl TestPattern {
    /// Создаёт пайплайн для формата цели. Неподдерживаемый формат — `Unsupported`.
    pub fn new(context: &GpuContext, color_format: vk::Format) -> Result<Self, GpuError> {
        let color_depth = ColorDepth::for_format(color_format)?;
        let device = context.device();

        let vertex_module = context.create_shader_module(blobs::FULLSCREEN_TRIANGLE_VERT)?;
        let fragment_module = context.create_shader_module(blobs::TEST_PATTERN_FRAG)?;

        let push_ranges = [vk::PushConstantRange::default()
            .stage_flags(vk::ShaderStageFlags::FRAGMENT)
            .offset(0)
            .size(PUSH_CONSTANT_BYTES)];
        let layout_info = vk::PipelineLayoutCreateInfo::default().push_constant_ranges(&push_ranges);
        // SAFETY: устройство валидно; layout_info ссылается на живой массив.
        let layout = OwnedPipelineLayout::from_raw(
            context,
            run_step(
                unsafe { device.create_pipeline_layout(&layout_info, None) },
                "vkCreatePipelineLayout",
            )?,
        );

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
        // SAFETY: все структуры и модули живут до конца вызова; кэш контекста синхронизирован драйвером.
        let pipelines = unsafe { device.create_graphics_pipelines(context.pipeline_cache(), &[pipeline_info], None) }
            .map_err(|(_, result)| GpuError::Vulkan {
            step: "vkCreateGraphicsPipelines",
            result,
        })?;
        let pipeline_handle = pipelines.first().copied().ok_or(GpuError::InvalidState {
            what: "vkCreateGraphicsPipelines не вернул пайплайн".to_owned(),
        })?;
        let pipeline = OwnedPipeline::from_raw(context, pipeline_handle);

        // Шейдерные модули после создания пайплайна не нужны — освободятся на выходе.
        Ok(Self {
            pipeline,
            layout,
            color_depth,
            color_format,
            context: context.clone(),
        })
    }

    /// Формат цели, под который создан пайплайн.
    pub fn color_format(&self) -> vk::Format {
        self.color_format
    }

    /// Разрядность канала цели.
    pub fn color_depth(&self) -> ColorDepth {
        self.color_depth
    }

    /// Записывает отрисовку паттерна в `command_buffer`. Картинка цели должна быть в раскладке
    /// `COLOR_ATTACHMENT_OPTIMAL`. Состояние самого `TestPattern` не меняется.
    pub fn record(
        &self,
        command_buffer: vk::CommandBuffer,
        target: PatternTarget,
        frame_index: u32,
        transform: SurfaceTransform,
    ) -> Result<(), GpuError> {
        // Все проверки ДО записи любых команд: при ошибке буфер остаётся нетронутым.
        let quarter_turns = transform.quarter_turns()?;
        if target.extent.width == 0 || target.extent.height == 0 {
            return Err(GpuError::InvalidArgument {
                what: "нулевой размер цели тестового паттерна".to_owned(),
            });
        }
        let device = self.context.device();

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
        let push_words = [
            target.extent.width,
            target.extent.height,
            frame_index,
            quarter_turns,
            self.color_depth.levels(),
        ];
        let push_bytes: Vec<u8> = push_words.iter().flat_map(|word| word.to_ne_bytes()).collect();

        // SAFETY: command_buffer в состоянии записи (контракт вызывающего); пайплайн/layout живы
        // (принадлежат self); push-константы укладываются в объявленный диапазон FRAGMENT 0..20.
        unsafe {
            device.cmd_begin_rendering(command_buffer, &rendering_info);
            device.cmd_bind_pipeline(command_buffer, vk::PipelineBindPoint::GRAPHICS, self.pipeline.handle());
            device.cmd_set_viewport(command_buffer, 0, &[viewport]);
            device.cmd_set_scissor(command_buffer, 0, &[render_area]);
            device.cmd_push_constants(
                command_buffer,
                self.layout.handle(),
                vk::ShaderStageFlags::FRAGMENT,
                0,
                &push_bytes,
            );
            device.cmd_draw(command_buffer, 3, 1, 0, 0);
            device.cmd_end_rendering(command_buffer);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;

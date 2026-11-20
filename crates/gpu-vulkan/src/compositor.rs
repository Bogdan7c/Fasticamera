//! [`ImageCompositor`] — кладёт готовую картинку (выход ISP видоискателя) на цель показа.
//!
//! Графический проход в стиле `raw_mosaic_preview`: динамический рендеринг, полноэкранный треугольник,
//! очистка цели в цвет фона. Фрагментный шейдер (`shaders/image_compositor.frag`) переводит физические
//! координаты цели в логические (прямые) по повороту дисплея и берёт линейную выборку источника внутри
//! прямоугольника назначения. Значения пишутся как есть: ISP уже закодировал их кривой экрана, а
//! форматы swapchain у нас UNORM (никогда не `_SRGB`), поэтому повторного кодирования нет.
//!
//! Инварианты:
//! * набор дескрипторов — по одному на слот кадра; переписывается в `record`, поэтому слот обязан быть
//!   свободен от GPU (fence слота сигнален) — как у `RawMosaicPreview`. Это ПРОВЕРЯЕТСЯ: после `record`
//!   слот помечен «привязан»; следующий `record` того же слота с ДРУГИМ источником — `InvalidState`,
//!   пока вызывающий не подтвердит fence слота ([`ImageCompositor::release_slot_after_fence`]). Тот же
//!   источник — без обновления набора, поэтому безопасен всегда (и дважды в одном командном буфере);
//! * источник при записи команд уже в `SHADER_READ_ONLY_OPTIMAL` и его запись сделана видимой
//!   фрагментному шейдеру (`StorageImage::record_compute_write_to_fragment_read`) — барьеры
//!   источника вне рендеринга записывает его владелец;
//! * цель — в `COLOR_ATTACHMENT_OPTIMAL` (переводит владелец цели);
//! * все проверки — до записи команд.

use ash::vk;

use crate::compute::BindingKind;
use crate::compute::descriptors::DescriptorRing;
use crate::compute::{BindingWrite, DescriptorResource};
use crate::context::GpuContext;
use crate::error::{GpuError, run_step};
use crate::live_objects::{OwnedPipeline, OwnedPipelineLayout};
use crate::raw_mosaic_preview::create_pipeline;
use crate::shaders::blobs;
use crate::source_window::SourceWindow;
use crate::storage_image::SampledImageView;
use crate::test_pattern::{PatternTarget, SurfaceTransform};

/// Размер push-констант: 4 × u32 + 4 × vec4 (см. `CompositeParameters` в шейдере).
const PUSH_CONSTANT_BYTES: u32 = 80;

/// Цвет фона вне прямоугольника назначения (letterbox), линейные значения UNORM 0..1.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BackgroundColor(pub [f32; 4]);

impl BackgroundColor {
    /// Чёрный непрозрачный — фон видоискателя.
    pub const BLACK: Self = Self([0.0, 0.0, 0.0, 1.0]);
}

/// Прямоугольник в ЛОГИЧЕСКИХ (прямых, как видит пользователь) пикселях цели.
/// Может выходить за цель — лишнее отсекается.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LogicalRect {
    /// Левый край.
    pub x: u32,
    /// Верхний край.
    pub y: u32,
    /// Ширина (> 0).
    pub width: u32,
    /// Высота (> 0).
    pub height: u32,
}

impl LogicalRect {
    /// Прямоугольник на всю логическую цель.
    pub fn covering(logical_extent: vk::Extent2D) -> Self {
        Self {
            x: 0,
            y: 0,
            width: logical_extent.width,
            height: logical_extent.height,
        }
    }
}

/// Что и куда положить в одном кадре.
#[derive(Debug, Clone, Copy)]
pub struct CompositeRequest {
    /// Источник (раскладка `SHADER_READ_ONLY_OPTIMAL`).
    pub source: SampledImageView,
    /// Поворот дисплея (`preTransform` swapchain): как у `RawMosaicPreview`, рисуем уже повёрнутым.
    pub transform: SurfaceTransform,
    /// Куда положить источник, в логических пикселях цели.
    pub destination: LogicalRect,
    /// Цвет вне прямоугольника.
    pub background: BackgroundColor,
    /// Слот кольца кадров (`DisplayFrame::ring_slot`): набор дескрипторов этого слота переписывается.
    pub frame_slot: usize,
}

/// Компоновщик под один формат цели. Порядок полей = порядок уничтожения: пайплайн → layout → наборы.
pub struct ImageCompositor {
    pipeline: OwnedPipeline,
    pipeline_layout: OwnedPipelineLayout,
    descriptors: DescriptorRing,
    context: GpuContext,
}

impl std::fmt::Debug for ImageCompositor {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "ImageCompositor({:?})", self.pipeline)
    }
}

impl ImageCompositor {
    /// Создаёт пайплайн под формат цели и наборы дескрипторов на `frame_slots` слотов.
    pub fn new(context: &GpuContext, target_format: vk::Format, frame_slots: usize) -> Result<Self, GpuError> {
        let descriptors = DescriptorRing::new(
            context,
            &[BindingKind::CombinedImageSampler],
            vk::ShaderStageFlags::FRAGMENT,
            frame_slots,
            "vkCreateDescriptorSetLayout/Pool(компоновщик)",
        )?;
        let set_layouts = [descriptors.set_layout()];
        let push_ranges = [vk::PushConstantRange::default()
            .stage_flags(vk::ShaderStageFlags::FRAGMENT)
            .offset(0)
            .size(PUSH_CONSTANT_BYTES)];
        let layout_info = vk::PipelineLayoutCreateInfo::default()
            .set_layouts(&set_layouts)
            .push_constant_ranges(&push_ranges);
        // SAFETY: устройство валидно; массивы живут до конца вызова.
        let pipeline_layout = OwnedPipelineLayout::from_raw(
            context,
            run_step(
                unsafe { context.device().create_pipeline_layout(&layout_info, None) },
                "vkCreatePipelineLayout(компоновщик)",
            )?,
        );
        let pipeline = create_pipeline(
            context,
            target_format,
            blobs::IMAGE_COMPOSITOR_FRAG,
            &pipeline_layout,
            "vkCreateGraphicsPipelines(компоновщик)",
        )?;
        Ok(Self {
            pipeline,
            pipeline_layout,
            descriptors,
            context: context.clone(),
        })
    }

    /// Число слотов наборов дескрипторов.
    pub fn frame_slots(&self) -> usize {
        self.descriptors.slot_count()
    }

    /// Вызывающий подтверждает: fence последнего сабмита, в который записана компоновка слота
    /// `frame_slot`, сигнален — набор слота снова можно переписывать (новый источник). Звать после
    /// ожидания fence слота кольца кадров, до `record` этого слота.
    pub fn release_slot_after_fence(&self, frame_slot: usize) -> Result<(), GpuError> {
        self.descriptors.release_after_fence(frame_slot)
    }

    /// Записывает компоновку в `command_buffer` и цель `target` (рендеринг начинается и заканчивается
    /// здесь; барьеры источника и цели — у их владельцев, вне рендеринга).
    ///
    /// Смена источника в слоте, привязанном после последнего [`Self::release_slot_after_fence`], —
    /// `InvalidState` до записи команд (см. инварианты модуля).
    pub fn record(
        &mut self,
        command_buffer: vk::CommandBuffer,
        target: PatternTarget,
        request: &CompositeRequest,
    ) -> Result<(), GpuError> {
        self.record_windowed(
            command_buffer,
            target,
            request,
            &SourceWindow::full(request.source.extent),
        )
    }

    /// То же, что [`Self::record`], но читает только окно `window` источника: содержимое в левом верхнем
    /// углу картинки ёмкости, зеркало и увеличение (см. [`SourceWindow`]). Прямоугольник назначения
    /// считает вызывающий по РАЗМЕРУ СОДЕРЖИМОГО, а не картинки.
    pub fn record_windowed(
        &mut self,
        command_buffer: vk::CommandBuffer,
        target: PatternTarget,
        request: &CompositeRequest,
        window: &SourceWindow,
    ) -> Result<(), GpuError> {
        window.validate_within(request.source.extent)?;
        let push_bytes = push_constants(target, request, window)?;
        self.descriptors.write(
            request.frame_slot,
            &[BindingWrite {
                binding: 0,
                resource: DescriptorResource::ReadOnlyImage {
                    view: request.source.view,
                },
            }],
        )?;
        let set = self.descriptors.bind_for_recording(request.frame_slot)?;
        let device = self.context.device();
        // SAFETY: командный буфер в состоянии записи вне другого рендеринга (контракт вызывающего); цель
        // в COLOR_ATTACHMENT_OPTIMAL; пайплайн, layout и набор принадлежат self; размер push-констант
        // равен объявленному диапазону FRAGMENT.
        unsafe {
            begin_rendering(device, command_buffer, target, request.background);
            device.cmd_bind_pipeline(command_buffer, vk::PipelineBindPoint::GRAPHICS, self.pipeline.handle());
            device.cmd_bind_descriptor_sets(
                command_buffer,
                vk::PipelineBindPoint::GRAPHICS,
                self.pipeline_layout.handle(),
                0,
                &[set],
                &[],
            );
            device.cmd_push_constants(
                command_buffer,
                self.pipeline_layout.handle(),
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

/// Проверяет запрос и собирает байты push-констант в порядке `CompositeParameters`.
fn push_constants(
    target: PatternTarget,
    request: &CompositeRequest,
    window: &SourceWindow,
) -> Result<Vec<u8>, GpuError> {
    let invalid = |what: String| GpuError::InvalidArgument { what };
    if target.extent.width == 0 || target.extent.height == 0 {
        return Err(invalid("нулевой размер цели компоновщика".to_owned()));
    }
    if request.source.extent.width == 0 || request.source.extent.height == 0 {
        return Err(invalid("нулевой размер источника компоновщика".to_owned()));
    }
    let destination = request.destination;
    if destination.width == 0 || destination.height == 0 {
        return Err(invalid(format!("пустой прямоугольник назначения {destination:?}")));
    }
    if request.background.0.iter().any(|channel| !channel.is_finite()) {
        return Err(invalid(format!("нечисловой цвет фона {:?}", request.background)));
    }
    let display_turns = request.transform.quarter_turns()?;
    let words = [target.extent.width, target.extent.height, display_turns, 0];
    let floats = [
        destination.x as f32,
        destination.y as f32,
        destination.width as f32,
        destination.height as f32,
        request.background.0[0],
        request.background.0[1],
        request.background.0[2],
        request.background.0[3],
    ];
    let mut bytes: Vec<u8> = words.iter().flat_map(|word| word.to_ne_bytes()).collect();
    bytes.extend(floats.iter().flat_map(|value| value.to_ne_bytes()));
    // Окно источника: origin.xy, step.xy, content.xy (+ выравнивание до vec4).
    let [origin_x, origin_y, step_x, step_y, content_x, content_y] = window.mapping().to_floats();
    bytes.extend(
        [origin_x, origin_y, step_x, step_y, content_x, content_y, 0.0, 0.0]
            .iter()
            .flat_map(|value| value.to_ne_bytes()),
    );
    Ok(bytes)
}

/// Начинает динамический рендеринг с очисткой цели в цвет фона.
///
/// # Safety
/// `command_buffer` в состоянии записи, вне другого рендеринга; цель в `COLOR_ATTACHMENT_OPTIMAL`.
unsafe fn begin_rendering(
    device: &ash::Device,
    command_buffer: vk::CommandBuffer,
    target: PatternTarget,
    background: BackgroundColor,
) {
    let color_attachments = [vk::RenderingAttachmentInfo::default()
        .image_view(target.view)
        .image_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
        .load_op(vk::AttachmentLoadOp::CLEAR)
        .store_op(vk::AttachmentStoreOp::STORE)
        .clear_value(vk::ClearValue {
            color: vk::ClearColorValue { float32: background.0 },
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

#[cfg(test)]
mod tests {
    use super::*;

    fn request(destination: LogicalRect, transform: SurfaceTransform) -> CompositeRequest {
        CompositeRequest {
            source: SampledImageView {
                view: vk::ImageView::null(),
                extent: vk::Extent2D { width: 8, height: 8 },
                format: vk::Format::R16G16B16A16_SFLOAT,
            },
            transform,
            destination,
            background: BackgroundColor::BLACK,
            frame_slot: 0,
        }
    }

    fn window() -> SourceWindow {
        SourceWindow::full(vk::Extent2D { width: 8, height: 8 })
    }

    fn target() -> PatternTarget {
        PatternTarget {
            view: vk::ImageView::null(),
            extent: vk::Extent2D { width: 24, height: 16 },
        }
    }

    #[test]
    fn push_constants_follow_shader_layout() {
        let destination = LogicalRect {
            x: 2,
            y: 3,
            width: 10,
            height: 12,
        };
        let bytes = push_constants(target(), &request(destination, SurfaceTransform::Rotate270), &window()).unwrap();
        assert_eq!(bytes.len(), PUSH_CONSTANT_BYTES as usize);
        let word = |index: usize| {
            u32::from_ne_bytes([
                bytes[index * 4],
                bytes[index * 4 + 1],
                bytes[index * 4 + 2],
                bytes[index * 4 + 3],
            ])
        };
        let float = |index: usize| f32::from_bits(word(index));
        assert_eq!((word(0), word(1), word(2)), (24, 16, 3));
        assert_eq!((float(4), float(5), float(6), float(7)), (2.0, 3.0, 10.0, 12.0));
        assert_eq!((float(8), float(11)), (0.0, 1.0), "чёрный непрозрачный фон");
        // Окно: источник 8×8 целиком — origin 0, step = размер, content = размер.
        assert_eq!((float(12), float(13), float(14), float(15)), (0.0, 0.0, 8.0, 8.0));
        assert_eq!((float(16), float(17)), (8.0, 8.0));
    }

    #[test]
    fn empty_rect_mirror_transform_and_nan_background_are_rejected() {
        let empty = LogicalRect {
            x: 0,
            y: 0,
            width: 0,
            height: 4,
        };
        assert!(push_constants(target(), &request(empty, SurfaceTransform::Identity), &window()).is_err());
        let full = LogicalRect::covering(vk::Extent2D { width: 24, height: 16 });
        let mirrored =
            push_constants(target(), &request(full, SurfaceTransform::HorizontalMirror), &window()).unwrap_err();
        assert!(matches!(mirrored, GpuError::Unsupported { .. }), "{mirrored:?}");
        let mut nan_background = request(full, SurfaceTransform::Identity);
        nan_background.background = BackgroundColor([f32::NAN, 0.0, 0.0, 1.0]);
        assert!(push_constants(target(), &nan_background, &window()).is_err());
    }
}

//! Общие помощники интеграционных тестов растрового набора (сессия 11): конвейер прямоугольников из
//! SSBO, конвейер выборки текстуры, цели, сравнение пикселей с допуском формата.

#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "общий модуль подключается разными тестовыми файлами и использует не всё; в тестах падение — способ сообщить о провале"
)]

use ash::vk;
use gpu_vulkan::color_target::{ColorTarget, ColorTargetDesc, ColorTargetFormat};
use gpu_vulkan::compute::{BindingKind, BindingWrite};
use gpu_vulkan::context::GpuContext;
use gpu_vulkan::host_buffer::{HostBuffer, HostBufferUsage};
use gpu_vulkan::raster::{BlendMode, RasterPipeline, RasterPipelineDesc};
use gpu_vulkan::shaders::blobs;

/// Один прямоугольник: что лежит в SSBO (`rect` + `color`, как `Instance` в `kit_raster_rects.vert`).
#[derive(Debug, Clone, Copy)]
pub struct RectInstance {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    pub color: [f32; 4],
}

impl RectInstance {
    pub fn new(x: u32, y: u32, width: u32, height: u32, color: [f32; 4]) -> Self {
        Self {
            x,
            y,
            width,
            height,
            color,
        }
    }

    /// Лежит ли пиксель внутри прямоугольника.
    pub fn contains(&self, pixel_x: u32, pixel_y: u32) -> bool {
        pixel_x >= self.x && pixel_x < self.x + self.width && pixel_y >= self.y && pixel_y < self.y + self.height
    }
}

pub const RED: [f32; 4] = [1.0, 0.0, 0.0, 1.0];
pub const GREEN: [f32; 4] = [0.0, 1.0, 0.0, 1.0];
pub const BLUE: [f32; 4] = [0.0, 0.0, 1.0, 1.0];
pub const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
pub const BLACK: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

/// Буфер экземпляров (host-visible storage только для чтения шейдером) с записанными прямоугольниками.
pub fn rect_buffer(context: &GpuContext, rects: &[RectInstance]) -> HostBuffer {
    let mut bytes = Vec::with_capacity(rects.len() * 32);
    for rect in rects {
        for value in [rect.x as f32, rect.y as f32, rect.width as f32, rect.height as f32] {
            bytes.extend_from_slice(&value.to_ne_bytes());
        }
        for value in rect.color {
            bytes.extend_from_slice(&value.to_ne_bytes());
        }
    }
    let mut buffer = HostBuffer::new(context, bytes.len() as u64, HostBufferUsage::ReadOnlyStorage).unwrap();
    buffer.write_bytes(0, &bytes).unwrap();
    buffer
}

/// Запись «буфер экземпляров → привязка 0».
pub fn instances_write(buffer: &HostBuffer) -> BindingWrite {
    BindingWrite {
        binding: 0,
        resource: buffer.binding(),
    }
}

/// Размер цели как push-константа `vec2`.
pub fn target_size_push(extent: vk::Extent2D) -> Vec<u8> {
    [extent.width as f32, extent.height as f32]
        .iter()
        .flat_map(|value| value.to_ne_bytes())
        .collect()
}

/// Конвейер «N прямоугольников за вызов»: SSBO в привязке 0, push-константы — размер цели.
pub fn rect_pipeline(
    context: &GpuContext,
    color_format: vk::Format,
    blend: BlendMode,
    frame_slots: usize,
) -> RasterPipeline {
    RasterPipeline::new(
        context,
        &RasterPipelineDesc {
            label: "kit-raster-rects",
            vertex_spirv: blobs::KIT_RASTER_RECTS_VERT,
            fragment_spirv: blobs::KIT_RASTER_RECTS_FRAG,
            bindings: &[BindingKind::ReadOnlyStorageBuffer],
            push_constant_bytes: 8,
            specialization: &[],
            color_format,
            blend,
            frame_slots,
        },
    )
    .unwrap()
}

/// Конвейер выборки текстуры на весь экран: сэмплер в привязке 0, push-константы — размер цели.
pub fn sample_pipeline(context: &GpuContext, color_format: vk::Format) -> RasterPipeline {
    RasterPipeline::new(
        context,
        &RasterPipelineDesc {
            label: "kit-raster-sample",
            vertex_spirv: blobs::FULLSCREEN_TRIANGLE_VERT,
            fragment_spirv: blobs::KIT_RASTER_SAMPLE_FRAG,
            bindings: &[BindingKind::CombinedImageSampler],
            push_constant_bytes: 8,
            specialization: &[],
            color_format,
            blend: BlendMode::Opaque,
            frame_slots: 1,
        },
    )
    .unwrap()
}

/// Цветовая цель.
pub fn color_target(context: &GpuContext, width: u32, height: u32, format: ColorTargetFormat) -> ColorTarget {
    ColorTarget::new(
        context,
        &ColorTargetDesc {
            extent: vk::Extent2D { width, height },
            format,
            label: "raster-test",
        },
    )
    .unwrap()
}

/// Допуск сравнения канала: 1 LSB формата (+ запас на округление). Альфа A2B10G10R10 — 2 бита (шаг 1/3).
pub fn tolerance(format: ColorTargetFormat, channel: usize) -> f32 {
    match format {
        ColorTargetFormat::Rgba8Unorm => 1.01 / 255.0,
        ColorTargetFormat::A2B10G10R10Unorm if channel == 3 => 1.0 / 3.0,
        ColorTargetFormat::A2B10G10R10Unorm => 1.01 / 1023.0,
        ColorTargetFormat::Rgba16Float => 1.0e-3,
    }
}

/// Пиксель `(x, y)` построчного readback шириной `width`.
pub fn pixel_at(pixels: &[[f32; 4]], width: u32, x: u32, y: u32) -> [f32; 4] {
    pixels[(y * width + x) as usize]
}

/// Сравнивает пиксель с ожидаемым в пределах допуска формата.
pub fn assert_pixel_near(actual: [f32; 4], expected: [f32; 4], format: ColorTargetFormat, what: &str) {
    for channel in 0..4 {
        let allowed = tolerance(format, channel);
        assert!(
            (actual[channel] - expected[channel]).abs() <= allowed,
            "{what}: канал {channel}: получили {actual:?}, ожидали {expected:?} (допуск {allowed})"
        );
    }
}

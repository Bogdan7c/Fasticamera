//! Конвейеры рендера UI: фигуры, глифы, стекло (в цель показа) и две ступени пирамиды (в её формат).

use gpu_vulkan::compute::BindingKind;
use gpu_vulkan::context::GpuContext;
use gpu_vulkan::error::GpuError;
use gpu_vulkan::raster::{BlendMode, RasterPipeline, RasterPipelineDesc};

use crate::config::{MAX_PYRAMID_LEVELS, UiRendererConfig};
use crate::pack::{BLUR_PUSH_BYTES, FRAME_PUSH_BYTES};
use crate::shaders::blobs;

/// Привязки конвейера стекла: группы, члены, фон полного разрешения, затем уровни пирамиды 1..=6.
pub(crate) const GLASS_FIRST_PYRAMID_BINDING: u32 = 3;

/// Все конвейеры рендера.
pub(crate) struct UiPipelines {
    pub shape: RasterPipeline,
    pub glyph: RasterPipeline,
    pub glass: RasterPipeline,
    /// Первая ступень пирамиды: фон → уровень 1. Набор на слот кадра (фон меняется).
    pub blur_from_backdrop: RasterPipeline,
    /// Остальные ступени: уровень k−1 → k. Набор на (слот кадра, ступень): `frame_slot * stages + stage`.
    pub blur_from_level: RasterPipeline,
    /// Сколько ступеней «уровень → уровень» (уровней − 1, не меньше 1, чтобы кольцо не было пустым).
    pub level_stages: usize,
}

/// Привязки стекла по порядку шейдера.
fn glass_bindings() -> Vec<BindingKind> {
    let mut bindings = vec![
        BindingKind::ReadOnlyStorageBuffer,
        BindingKind::ReadOnlyStorageBuffer,
        BindingKind::CombinedImageSampler,
    ];
    bindings.extend((0..MAX_PYRAMID_LEVELS).map(|_| BindingKind::CombinedImageSampler));
    bindings
}

impl UiPipelines {
    /// Создаёт конвейеры под настройки.
    pub(crate) fn new(context: &GpuContext, config: &UiRendererConfig) -> Result<Self, GpuError> {
        let ui_pipeline =
            |label: &'static str, vertex: &'static [u8], fragment: &'static [u8], bindings: &[BindingKind]| {
                RasterPipeline::new(
                    context,
                    &RasterPipelineDesc {
                        label,
                        vertex_spirv: vertex,
                        fragment_spirv: fragment,
                        bindings,
                        push_constant_bytes: FRAME_PUSH_BYTES,
                        specialization: &[],
                        color_format: config.target_format,
                        blend: BlendMode::PremultipliedAlpha,
                        frame_slots: config.frame_slots,
                    },
                )
            };
        let shape = ui_pipeline(
            "ui-shape",
            blobs::UI_SHAPE_VERT,
            blobs::UI_SHAPE_FRAG,
            &[BindingKind::ReadOnlyStorageBuffer],
        )?;
        let glyph = ui_pipeline(
            "ui-glyph",
            blobs::UI_GLYPH_VERT,
            blobs::UI_GLYPH_FRAG,
            &[BindingKind::ReadOnlyStorageBuffer, BindingKind::CombinedImageSampler],
        )?;
        let glass = ui_pipeline(
            "ui-glass",
            blobs::UI_GLASS_VERT,
            blobs::UI_GLASS_FRAG,
            &glass_bindings(),
        )?;

        let pyramid_format = config.pyramid.format.color_target_format().vk_format();
        let level_stages = (config.pyramid.levels as usize).saturating_sub(1).max(1);
        let blur_pipeline = |label: &'static str, fragment: &'static [u8], frame_slots: usize| {
            RasterPipeline::new(
                context,
                &RasterPipelineDesc {
                    label,
                    vertex_spirv: blobs::UI_FULLSCREEN_VERT,
                    fragment_spirv: fragment,
                    bindings: &[BindingKind::CombinedImageSampler],
                    push_constant_bytes: BLUR_PUSH_BYTES,
                    specialization: &[],
                    color_format: pyramid_format,
                    blend: BlendMode::Opaque,
                    frame_slots,
                },
            )
        };
        let blur_from_backdrop = blur_pipeline(
            "ui-blur-backdrop",
            blobs::UI_BLUR_DOWN_BACKDROP_FRAG,
            config.frame_slots,
        )?;
        let blur_from_level = blur_pipeline(
            "ui-blur-level",
            blobs::UI_BLUR_DOWN_LEVEL_FRAG,
            config.frame_slots * level_stages,
        )?;
        Ok(Self {
            shape,
            glyph,
            glass,
            blur_from_backdrop,
            blur_from_level,
            level_stages,
        })
    }
}

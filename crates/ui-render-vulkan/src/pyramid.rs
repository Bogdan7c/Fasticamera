//! Пирамида размытия фона: dual Kawase «вниз», фрагментными проходами в `ColorTarget`-ы.
//!
//! # Устройство
//! Уровень k (1..=K) хранится в системе координат ЭКРАНА: его тексель покрывает 2^k логических px, размер
//! `ceil(логический размер / 2^k)`. Первая ступень читает фон через размещение композитора (вне
//! прямоугольника — чёрный), остальные — предыдущий уровень. Так стекло берёт размытый фон по экранной точке,
//! не зная, как фон был размещён.
//!
//! Ступень: пиксель цели = (центр ×4 + четыре диагональные выборки на тексель источника) / 8; каждая выборка —
//! билинейная по 2×2 исходных текселя. Ступени читают вершинный (средняя яркость группы) и фрагментный шейдеры стекла: барьеры ступеней покрывают обе
//! стадии (`PYRAMID_READER_STAGES`); исходный фон (картинка ISP) читает только фрагментный шейдер ступени 1.
//! Проходов «вверх» нет: уровни читаются билинейно и смешиваются по дробной части `lod` (см. `blur_lod` в шейдере);
//! это дешевле на ступень-другую и на тестовых сценах без заметных полос (решение владельца ждёт 11a — глаза).
//!
//! # Ключ
//! Пирамида зависит от картинки фона (поколение + номер кадра), её размещения и размера экрана. Ключ — всё это;
//! пока он не менялся, ступени переиспользуются (путь Recompose: UI меняется, фон тот же).

use ash::vk;
use gpu_vulkan::backdrop::{BackdropFrameNumber, BackdropSource};
use gpu_vulkan::color_target::{ColorTarget, ColorTargetDesc, ColorTargetPreviousUse, ContentOnPrepare};
use gpu_vulkan::compute::{BindingWrite, DescriptorResource};
use gpu_vulkan::context::GpuContext;
use gpu_vulkan::error::GpuError;
use gpu_vulkan::hardware_buffer_cache::queue_handoff::ConsumerStage;
use gpu_vulkan::raster::{AttachmentLoad, DrawCall, RasterPass};
use gpu_vulkan::storage_image::StorageImageGeneration;
use ui_draw::color::DisplayColor;
use ui_draw::geometry::PxRect;

use crate::config::BackdropPyramidConfig;
use crate::pack::BlurPush;
use crate::pipelines::UiPipelines;

/// Кто читает ступени пирамиды: фрагментный шейдер стекла (размытый фон) и вершинный (средняя яркость группы).
/// Все барьеры пирамиды обязаны покрывать обе стадии, иначе вершинная выборка гонится с перестройкой (RAW/WAR).
pub(crate) const PYRAMID_READER_STAGES: ConsumerStage = ConsumerStage::VertexAndFragment;

/// Что определяет содержимое ступеней.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PyramidKey {
    Live {
        generation: StorageImageGeneration,
        frame: BackdropFrameNumber,
        /// Размещение фона побитно (f32 не `Eq`; изменение хоть на бит = перестройка).
        destination_bits: [u32; 4],
        /// Окно чтения фона побитно (origin, step, содержимое): удержанный кадр с меняющимся увеличением
        /// или зеркало — другой фон, ступени переиспользовать нельзя.
        window_bits: [u32; 6],
    },
    Absent {
        fill_bits: [u32; 4],
    },
}

impl PyramidKey {
    pub(crate) fn live(source: &BackdropSource, destination: &PxRect) -> Self {
        Self::Live {
            generation: source.image_generation(),
            frame: source.frame(),
            destination_bits: rect_bits(destination),
            window_bits: source.window().mapping().to_floats().map(f32::to_bits),
        }
    }

    pub(crate) fn absent(fill: DisplayColor) -> Self {
        Self::Absent {
            fill_bits: [fill.r.to_bits(), fill.g.to_bits(), fill.b.to_bits(), fill.a.to_bits()],
        }
    }
}

fn rect_bits(rect: &PxRect) -> [u32; 4] {
    [
        rect.origin.x.to_bits(),
        rect.origin.y.to_bits(),
        rect.size.width.to_bits(),
        rect.size.height.to_bits(),
    ]
}

/// Источник перестройки.
#[derive(Clone, Copy)]
pub(crate) enum RebuildSource<'a> {
    Live {
        source: &'a BackdropSource,
        destination: PxRect,
    },
    Absent {
        fill: DisplayColor,
    },
}

/// Набор ступеней под один логический размер.
pub(crate) struct Pyramid {
    pub targets: Vec<ColorTarget>,
    pub logical: (u32, u32),
}

impl Pyramid {
    /// Создаёт ступени 1..=levels для логического размера.
    pub(crate) fn new(
        context: &GpuContext,
        logical: (u32, u32),
        config: &BackdropPyramidConfig,
    ) -> Result<Self, GpuError> {
        let mut targets = Vec::with_capacity(config.levels as usize);
        for level in 1..=config.levels {
            let divisor = 1_u32 << level;
            targets.push(ColorTarget::new(
                context,
                &ColorTargetDesc {
                    extent: vk::Extent2D {
                        width: logical.0.div_ceil(divisor).max(1),
                        height: logical.1.div_ceil(divisor).max(1),
                    },
                    format: config.format.color_target_format(),
                    label: "ui-backdrop-pyramid",
                },
            )?);
        }
        Ok(Self { targets, logical })
    }

    /// Представление уровня (1-based) для привязки; уровни глубже построенных повторяют самый глубокий.
    pub(crate) fn level_binding(&self, level: usize) -> DescriptorResource {
        let index = level.saturating_sub(1).min(self.targets.len() - 1);
        DescriptorResource::ReadOnlyImage {
            view: self.targets[index].sampled_view().view,
        }
    }

    /// Записывает все ступени. Вне динамического рендеринга. `previous_use` — как ступени использовали последним.
    pub(crate) fn record_rebuild(
        &self,
        context: &GpuContext,
        command_buffer: vk::CommandBuffer,
        pipelines: &mut UiPipelines,
        frame_slot: usize,
        source: RebuildSource<'_>,
        previous_use: ColorTargetPreviousUse,
    ) -> Result<(), GpuError> {
        if let RebuildSource::Live { source, .. } = source {
            // Барьер «запись ISP → чтение фрагментным шейдером»: только при перестройке (ключ сменился).
            source.record_read_barrier(command_buffer, ConsumerStage::Fragment);
        }
        for (index, target) in self.targets.iter().enumerate() {
            let level = index + 1;
            target.record_prepare_for_render(command_buffer, previous_use, ContentOnPrepare::Discard)?;
            let load = match source {
                RebuildSource::Absent { fill } => AttachmentLoad::Clear([fill.r, fill.g, fill.b, 1.0]),
                RebuildSource::Live { .. } => AttachmentLoad::Clear([0.0, 0.0, 0.0, 1.0]),
            };
            let mut pass = RasterPass::begin(context, command_buffer, target.attachment(), load)?;
            if let RebuildSource::Live { source, destination } = source {
                // Ступень k: цель — тексель 2^k px, источник — тексель 2^(k−1) px, выборки на полтексела цели.
                let source_texel_px = (1_u32 << (level - 1)) as f32;
                let push = BlurPush {
                    destination: [
                        destination.origin.x,
                        destination.origin.y,
                        destination.size.width,
                        destination.size.height,
                    ],
                    tap_offset_px: source_texel_px,
                    target_texel_px: source_texel_px * 2.0,
                    source_texel_px,
                    backdrop_window: source.window().mapping(),
                };
                let full_screen = DrawCall {
                    vertices_per_instance: 3,
                    instance_count: 1,
                    scissor: None,
                };
                if level == 1 {
                    pipelines.blur_from_backdrop.bind(
                        frame_slot,
                        &[BindingWrite {
                            binding: 0,
                            resource: DescriptorResource::ReadOnlyImage {
                                view: source.view().view,
                            },
                        }],
                    )?;
                    pass.draw(&pipelines.blur_from_backdrop, frame_slot, &push.to_bytes(), full_screen)?;
                } else {
                    let stage_slot = frame_slot * pipelines.level_stages + (level - 2);
                    pipelines.blur_from_level.bind(
                        stage_slot,
                        &[BindingWrite {
                            binding: 0,
                            resource: self.level_binding(level - 1),
                        }],
                    )?;
                    pass.draw(&pipelines.blur_from_level, stage_slot, &push.to_bytes(), full_screen)?;
                }
            }
            pass.end();
            target.record_render_to_sampled_read(command_buffer, PYRAMID_READER_STAGES);
        }
        Ok(())
    }
}

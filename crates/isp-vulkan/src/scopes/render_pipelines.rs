//! Пайплайны второго прохода показа: продуктовый (следы и вектороскоп), гистограммы и диаграммы CIE (сессия 14s).
//!
//! Все три имеют ОДИН интерфейс (привязки [`RENDER_BINDINGS`], блок параметров `scope_render_parameters.glsl`), различается
//! только SPIR-V: столбики гистограммы — вариант `SCOPE_RENDER_HISTOGRAM`, диаграмма CIE — вариант `SCOPE_RENDER_CIE` того же тела показа. Выбор пайплайна по способу
//! рисования следа — единственное место, где проходы знают, что столбики гистограммы идут отдельным шейдером; остальной
//! код [`super::ColorScopes`] зовёт [`RenderPipelines::for_paint`].

use gpu_vulkan::compute::{BindingKind, ComputePipeline, ComputePipelineDesc, LocalSize};
use gpu_vulkan::context::GpuContext;
use scope_model::TracePaint;

use super::accumulators::local_size_specialization;
use super::error::ScopeError;
use crate::shaders::blobs;

/// Привязки шейдера показа: картинка прибора, параметры показа, промежуточный слой первого прохода (только чтение).
pub(super) const RENDER_BINDINGS: [BindingKind; 3] = [
    BindingKind::StorageImageWrite,
    BindingKind::ReadOnlyStorageBuffer,
    BindingKind::ReadOnlyStorageBuffer,
];

/// Пайплайны показа.
pub(super) struct RenderPipelines {
    /// Следы по плотности и цвет по положению (RGB waveform, Luma, parade, вектороскоп).
    product: ComputePipeline,
    /// Столбики гистограммы.
    histogram: ComputePipeline,
    /// Диаграмма CIE: цвет цветности, заливка локуса под следом (вариант `SCOPE_RENDER_CIE`).
    cie: ComputePipeline,
}

impl RenderPipelines {
    /// Создаёт оба пайплайна. `product_spirv` — шейдер показа следов (тесты подставляют свой с тем же интерфейсом).
    pub(super) fn new(
        context: &GpuContext,
        product_spirv: &'static [u8],
        local_size: LocalSize,
        frame_slots: usize,
    ) -> Result<Self, ScopeError> {
        let create = |label: &'static str, spirv: &'static [u8]| {
            ComputePipeline::new(
                context,
                &ComputePipelineDesc {
                    label,
                    spirv,
                    bindings: &RENDER_BINDINGS,
                    push_constant_bytes: 0,
                    specialization: &local_size_specialization(local_size),
                    frame_slots,
                },
            )
        };
        Ok(Self {
            product: create("scope-render", product_spirv)?,
            histogram: create("scope-render-histogram", blobs::SCOPE_RENDER_HISTOGRAM_COMP)?,
            cie: create("scope-render-cie", blobs::SCOPE_RENDER_CIE_COMP)?,
        })
    }

    /// Пайплайн показа для способа рисования `paint`.
    pub(super) fn for_paint(&self, paint: TracePaint) -> &ComputePipeline {
        match paint {
            TracePaint::HistogramOverlaid | TracePaint::HistogramSections => &self.histogram,
            TracePaint::Cie => &self.cie,
            TracePaint::PlaneSections | TracePaint::PlanesOverlaid | TracePaint::ColorByPosition => &self.product,
        }
    }

    /// То же, изменяемо (привязки слота).
    pub(super) fn for_paint_mut(&mut self, paint: TracePaint) -> &mut ComputePipeline {
        match paint {
            TracePaint::HistogramOverlaid | TracePaint::HistogramSections => &mut self.histogram,
            TracePaint::Cie => &mut self.cie,
            TracePaint::PlaneSections | TracePaint::PlanesOverlaid | TracePaint::ColorByPosition => &mut self.product,
        }
    }

    /// Fence слота сигнален: наборы слота обоих пайплайнов снова можно переписывать.
    pub(super) fn release_slot_after_fence(&self, frame_slot: usize) -> Result<(), ScopeError> {
        self.product.release_slot_after_fence(frame_slot)?;
        self.histogram.release_slot_after_fence(frame_slot)?;
        self.cie.release_slot_after_fence(frame_slot)?;
        Ok(())
    }
}

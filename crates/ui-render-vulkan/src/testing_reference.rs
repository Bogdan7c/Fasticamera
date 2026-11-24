//! Рендер с замороженными шейдерами-эталонами (feature `test-support`).
//!
//! Шейдеры стекла и фигур получили быстрые пути (пропуск работы, результат которой умножается на точный ноль).
//! Чтобы доказать, что картинка не изменилась ни на один бит, тест рисует сцену двумя рендерами: боевым
//! ([`crate::UiRenderer::new`]) и этим, на копиях шейдеров до оптимизаций (`shaders/reference_*`). Остальное
//! (вершинные шейдеры, раскладка данных, пирамида, глифы, картинки) у обоих рендеров общее.

use gpu_vulkan::context::GpuContext;

use crate::config::UiRendererConfig;
use crate::error::UiRenderError;
use crate::pipelines::UiShaderSet;
use crate::renderer::UiRenderer;

/// Рендер UI, у которого стекло и фигуры рисуются замороженными шейдерами коммита 896865d (без быстрых путей).
/// В боевую сборку не попадает.
pub fn frozen_reference_renderer(context: &GpuContext, config: &UiRendererConfig) -> Result<UiRenderer, UiRenderError> {
    UiRenderer::with_shader_set(context, config, UiShaderSet::FrozenReference)
}

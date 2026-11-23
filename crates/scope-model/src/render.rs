//! Правило показа: счётчики → картинка прибора RGBA (f32, прямая альфа, цвета экрана).
//!
//! UI картинку не пересчитывает: цвета уже в кодировке экрана. Фон прозрачный (альфа 0), след — с альфой,
//! чтобы стекло и превью просвечивали (решение 185). ТОЛЬКО СЧЁТЧИКИ сверяются с GPU бит в бит; здесь
//! есть `log2` и деление, поэтому картинка сверяется с допуском.
//!
//! Пиксель → ячейка целочисленно: `column = px · columns / width`, `row = py · rows / height`. Если у
//! прибора несколько плоскостей (parade), картинка делится на столько равных секций по ширине, каждая —
//! на весь диапазон столбцов своей плоскости.
//!
//! Яркость `min(1, log2(1 + count) / log2(1 + reference))`, альфа следа `яркость · trace_opacity`. Сетка
//! рисуется ПОД следом: прямая альфа «over», `a = a_t + a_g (1 − a_t)`.

use scope_contract::ScopeImageSize;

use crate::graticule::GraticulePrimitive;
use crate::grid::ScopeCounters;
use crate::render_settings::{RenderSettings, RenderSettingsError};
use crate::resolved::ResolvedScope;
use crate::rule::ScopeRule;

/// Ошибка показа.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum RenderError {
    /// Счётчики от другого прибора: форма не совпала с формой правила.
    #[error("форма счётчиков не совпала с формой прибора")]
    ShapeMismatch,
    /// Настройки показа недопустимы.
    #[error(transparent)]
    Settings(#[from] RenderSettingsError),
}

/// Готовая картинка прибора: RGBA f32, построчно сверху вниз, прямая альфа.
#[derive(Debug, Clone, PartialEq)]
pub struct ScopePicture {
    size: ScopeImageSize,
    pixels: Vec<[f32; 4]>,
}

impl ScopePicture {
    /// Размер.
    pub fn size(&self) -> ScopeImageSize {
        self.size
    }

    /// Все пиксели построчно.
    pub fn pixels(&self) -> &[[f32; 4]] {
        &self.pixels
    }

    /// Пиксель `(x, y)`; `None` вне картинки.
    pub fn pixel(&self, x: u32, y: u32) -> Option<[f32; 4]> {
        if x >= self.size.width() || y >= self.size.height() {
            return None;
        }
        self.pixels
            .get(y as usize * self.size.width() as usize + x as usize)
            .copied()
    }
}

/// Яркость следа по счётчику: `min(1, log2(1 + count) / log2(1 + reference))`.
pub fn trace_intensity(count: u32, saturation_count: u32) -> f32 {
    let numerator = (1.0 + count as f32).log2();
    let denominator = (1.0 + saturation_count as f32).log2();
    (numerator / denominator).min(1.0)
}

/// Картинка прибора из счётчиков.
pub fn render(
    counters: &ScopeCounters,
    scope: &ResolvedScope,
    settings: &RenderSettings,
    size: ScopeImageSize,
) -> Result<ScopePicture, RenderError> {
    settings.validate()?;
    if counters.shape() != scope.shape() {
        return Err(RenderError::ShapeMismatch);
    }
    let shape = counters.shape();
    let colors = scope.trace_colors(settings);
    let graticule = scope.graticule(settings);
    let picture_px = [size.width() as f32, size.height() as f32];
    let width = u64::from(size.width());
    let planes = u64::from(shape.planes());
    let mut pixels = Vec::with_capacity(size.width() as usize * size.height() as usize);
    for y in 0..size.height() {
        let row = (u64::from(y) * u64::from(shape.rows()) / u64::from(size.height())) as u32;
        for x in 0..size.width() {
            // Секция плоскости и столбец внутри неё — целочисленно.
            let plane = u64::from(x) * planes / width;
            let column = ((u64::from(x) * planes - plane * width) * u64::from(shape.columns()) / width) as u32;
            let count = counters.count(plane as u32, row, column).unwrap_or(0);
            let color = colors
                .get(plane as usize)
                .copied()
                .unwrap_or(settings.vectorscope_color);
            let trace_alpha = if count == 0 {
                0.0
            } else {
                trace_intensity(count, settings.trace_saturation_count) * settings.trace_opacity
            };
            let center = [x as f32 + 0.5, y as f32 + 0.5];
            let graticule_alpha = graticule_alpha(&graticule, center, picture_px, settings);
            pixels.push(composite_over(
                color,
                trace_alpha,
                settings.graticule_color,
                graticule_alpha,
            ));
        }
    }
    Ok(ScopePicture { size, pixels })
}

/// Наибольшее покрытие примитивами сетки, умноженное на непрозрачность сетки.
fn graticule_alpha(
    primitives: &[GraticulePrimitive],
    center: [f32; 2],
    picture_px: [f32; 2],
    settings: &RenderSettings,
) -> f32 {
    let coverage = primitives
        .iter()
        .map(|primitive| primitive.coverage(center, picture_px))
        .fold(0.0_f32, f32::max);
    coverage * settings.graticule_opacity
}

/// След поверх сетки (прямая альфа, «over»). Нулевая итоговая альфа даёт полностью прозрачный чёрный.
fn composite_over(trace: [f32; 3], trace_alpha: f32, grid: [f32; 3], grid_alpha: f32) -> [f32; 4] {
    let alpha = trace_alpha + grid_alpha * (1.0 - trace_alpha);
    if alpha <= 0.0 {
        return [0.0; 4];
    }
    let channel = |index: usize| (trace[index] * trace_alpha + grid[index] * grid_alpha * (1.0 - trace_alpha)) / alpha;
    [channel(0), channel(1), channel(2), alpha]
}

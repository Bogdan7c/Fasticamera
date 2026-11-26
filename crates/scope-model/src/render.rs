//! Правило показа: счётчики → картинка прибора RGBA (f32, прямая альфа, цвета экрана).
//!
//! UI картинку не пересчитывает: цвета уже в кодировке экрана. Фон прозрачный (альфа 0), след — с альфой,
//! чтобы стекло и превью просвечивали (решение 185). ТОЛЬКО СЧЁТЧИКИ сверяются с GPU бит в бит; здесь
//! есть `log2` и деление, поэтому картинка сверяется с допуском.
//!
//! Способ рисования следа решает правило прибора ([`ResolvedScope::trace_style`]): секции по плоскостям, плоскости
//! наложением или цвет по месту; пиксельные формулы каждого способа — в [`crate::trace_painter`]. Яркость следа —
//! `min(1, log2(1 + count) / log2(1 + порог))`, порог — доля отсчётов одного столбца (waveform, parade) или всего кадра (вектороскоп), см. [`TraceReference`], поэтому
//! [`render`] получает число отсчётов кадра. Альфа следа `яркость · trace_opacity`. Сетка рисуется ПОД следом:
//! прямая альфа «over», `a = a_t + a_g (1 − a_t)`.

use scope_contract::ScopeImageSize;

use crate::graticule::GraticulePrimitive;
use crate::grid::ScopeCounters;
use crate::render_settings::{RenderSettings, RenderSettingsError};
use crate::resolved::ResolvedScope;
use crate::rule::ScopeRule;
use crate::sample_count::SampleCount;
use crate::trace_painter::TracePainter;
use crate::trace_reference::TraceReference;

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

/// Картинка прибора из счётчиков. `samples` — число отсчётов кадра, из которого получены счётчики
/// ([`SampleCount::for_window`] по окну показанной картинки): от него зависит опорное число яркости следа.
pub fn render(
    counters: &ScopeCounters,
    scope: &ResolvedScope,
    settings: &RenderSettings,
    size: ScopeImageSize,
    samples: SampleCount,
) -> Result<ScopePicture, RenderError> {
    settings.validate()?;
    if counters.shape() != scope.shape() {
        return Err(RenderError::ShapeMismatch);
    }
    let style = scope.trace_style(settings);
    let reference = TraceReference::for_paint(style.paint(), settings, samples, counters.shape().columns());
    let painter = TracePainter::new(counters, &style, reference, settings.trace_opacity);
    let graticule = scope.graticule(settings);
    let picture_px = [size.width() as f32, size.height() as f32];
    let mut pixels = Vec::with_capacity(size.width() as usize * size.height() as usize);
    for y in 0..size.height() {
        for x in 0..size.width() {
            let trace = painter.pixel(x, y, size);
            let center = [x as f32 + 0.5, y as f32 + 0.5];
            let graticule_alpha = graticule_alpha(&graticule, center, picture_px, settings);
            pixels.push(composite_over(
                trace.color,
                trace.alpha,
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

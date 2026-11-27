//! Рисование гистограммы: эталон ветки `SCOPE_RENDER_HISTOGRAM` шейдера `scope_render_body.glsl` (решения 336, 337).
//!
//! Конвейер:
//! 1. счётчики по уровням → столбцы картинки тем же переносом по горизонтали, что у следов
//!    ([`crate::trace_density::resample_columns`], гауссово ядро `AxisResampling`, число попаданий сохраняется): плотность
//!    `D` столбца картинки — «сколько отсчётов легло на этот пиксель по ширине». Строка сетки одна, переноса по вертикали
//!    нет. Каждая плоскость занимает ВСЮ ширину (полосы PARADE лежат друг над другом, а не рядом);
//! 2. счёт на уровень `n = D · (пикселей на уровень)` и шкала [`HistogramHeight`] дают долю высоты полосы `0..=1`;
//! 3. пиксель закрашен заливкой, если лежит ниже верха столбика (верх мягкий, шириной в пиксель: без этого 1 ulp
//!    расхождения GPU и CPU переворачивал бы целый пиксель), и линией, если лежит у ломаной по верхушкам столбцов
//!    ([`crate::histogram_paint`]). Цвет и прозрачность — [`crate::histogram_paint::HistogramBlend`].
//!
//! Полосы: наложение (RGB) — одна полоса на всю высоту, три канала в одних пикселях; PARADE — по полосе на плоскость
//! (R сверху), Y — одна. Раскладка — [`HistogramStrips`]. Опора шкалы «самый высокий столбик» — ОДНА на картинку: наибольшая
//! плотность по всем столбцам и плоскостям, так что три полосы PARADE честно сравнимы по высоте друг с другом (канал с
//! меньшим пиком виден ниже). Свои прозрачности трассы (`trace_opacity`) гистограмма не берёт: у неё свои настройки
//! вида ([`HistogramLook`]).

use scope_contract::ScopeImageSize;

use crate::grid::ScopeCounters;
use crate::histogram_height::{HistogramCurve, HistogramHeight, HistogramReference};
use crate::histogram_paint::{HistogramBlend, edge_coverage};
use crate::histogram_strips::{HistogramLook, HistogramStrips};
use crate::sample_count::SampleCount;
use crate::trace_density::{resample_columns, slots_per_pixel};
use crate::trace_painter::TracePixel;
use crate::trace_resample::AxisResampling;
use crate::trace_style::TraceStyle;

/// Опора шкалы пикселя: известный счёт или «самый высокий столбик».
#[derive(Debug, Clone, Copy, PartialEq)]
enum BarReference {
    /// Опора в отсчётах на уровень известна до рисования (кратное среднего).
    Known(f32),
    /// Опора — самый высокий столбик картинки: хранится его плотность `D`, счётом он становится умножением на пиксели
    /// на уровень.
    TallestDensity(f32),
}

/// Столбики гистограммы одной картинки.
pub(crate) struct HistogramBars<'a> {
    style: &'a TraceStyle,
    size: ScopeImageSize,
    planes: u32,
    slots: u32,
    /// Плотность столбцов картинки по слотам: `[x][слот]`.
    density: Vec<f32>,
    /// Пикселей картинки на один уровень (все плоскости на всю ширину — число одно).
    pixels_per_bin: f32,
    curve: HistogramCurve,
    reference: BarReference,
    look: HistogramLook,
    strips: HistogramStrips,
}

impl<'a> HistogramBars<'a> {
    /// Столбики по счётчикам `counters` кадра с отсчётами `samples`. Стиль обязан быть одним из двух гистограммных
    /// (иначе столбиков нет — `pixel` вернёт «нет следа»).
    pub(crate) fn new(
        counters: &ScopeCounters,
        samples: SampleCount,
        style: &'a TraceStyle,
        size: ScopeImageSize,
        height: HistogramHeight,
        look: HistogramLook,
        softening_px: f32,
    ) -> Self {
        let shape = counters.shape();
        let paint = style.paint();
        let slots = slots_per_pixel(paint, shape.planes());
        // Строка сетки одна: первый проход переноса — вся плотность (`[строка 0][x][слот]` = `[x][слот]`).
        let density = resample_columns(counters, paint, size, softening_px);
        let pixels_per_bin = AxisResampling::new(shape.columns(), size.width(), softening_px).pixels_per_cell();
        let reference = match height.reference() {
            HistogramReference::TallestBar => {
                BarReference::TallestDensity(density.iter().copied().fold(0.0_f32, f32::max))
            }
            HistogramReference::MeanMultiple { .. } => {
                // Известна до рисования: `known_reference_count` отвечает для среднего всегда.
                BarReference::Known(
                    height
                        .known_reference_count(samples.total(), shape.columns())
                        .unwrap_or(0.0),
                )
            }
        };
        let strips = match style {
            TraceStyle::HistogramSections { .. } => HistogramStrips::new(shape.planes(), look.strip_gap),
            TraceStyle::HistogramOverlaid { .. }
            | TraceStyle::PlaneSections { .. }
            | TraceStyle::PlanesOverlaid { .. }
            | TraceStyle::ColorByPosition { .. }
            | TraceStyle::Cie(_) => HistogramStrips::new(1, look.strip_gap),
        };
        Self {
            style,
            size,
            planes: shape.planes(),
            slots,
            density,
            pixels_per_bin,
            curve: height.curve(),
            reference,
            look,
            strips,
        }
    }

    /// Плотность столбца `x` в слоте `slot` (вне картинки — 0).
    fn density_at(&self, x: u32, slot: u32) -> f32 {
        if x >= self.size.width() || slot >= self.slots {
            return 0.0;
        }
        self.density[x as usize * self.slots as usize + slot as usize]
    }

    /// Доля высоты столбика слота `slot` в столбце `x`.
    fn height_fraction(&self, x: u32, slot: u32) -> f32 {
        let count = self.density_at(x, slot) * self.pixels_per_bin;
        let reference = match self.reference {
            BarReference::Known(count) => count,
            BarReference::TallestDensity(density) => density * self.pixels_per_bin,
        };
        self.curve.fraction(count, reference)
    }

    /// Высоты верхушек столбцов `x − 1`, `x`, `x + 1` слота `slot` в пикселях полосы высотой `strip_px`; за краем картинки
    /// берётся крайний столбец (график продолжается ровно).
    fn heights_around(&self, x: u32, slot: u32, strip_px: f32) -> [f32; 3] {
        let last = self.size.width().saturating_sub(1);
        let left = x.saturating_sub(1);
        let right = (x + 1).min(last);
        [left, x, right].map(|column| self.height_fraction(column, slot) * strip_px)
    }

    /// Покрытие заливкой пикселя на расстоянии `distance_to_baseline` от базовой линии под верхушкой `top_px`: верх мягкий,
    /// шириной в один пиксель. ПОРЯДОК ОПЕРАЦИЙ f32 тот же, что в шейдере.
    /// Пустой столбец (высота 0) не закрашивается вовсе: базовая линия полосы может лечь не на границу пикселя, и
    /// без этой проверки мягкий верх нулевой высоты давал бы пятно на пикселе у базовой линии.
    fn fill_coverage(top_px: f32, distance_to_baseline: f32) -> f32 {
        if top_px <= 0.0 {
            return 0.0;
        }
        (top_px - distance_to_baseline + 0.5).clamp(0.0, 1.0)
    }

    /// След пикселя `(x, y)`.
    pub(crate) fn pixel(&self, x: u32, y: u32) -> TracePixel {
        let centre_y = y as f32 + 0.5;
        let Some(strip) = self.strips.locate(centre_y, self.size.height() as f32) else {
            return TracePixel::ABSENT;
        };
        let mut blend = HistogramBlend::new();
        let mut add_slot = |slot: u32, color: [f32; 3]| {
            let heights = self.heights_around(x, slot, strip.height_px);
            let fill = Self::fill_coverage(heights[1], strip.distance_to_baseline);
            let edge = edge_coverage([0.0, strip.distance_to_baseline], heights, self.look.edge_width_px);
            blend.add_channel(color, fill, edge);
        };
        match self.style {
            // Наложение: все плоскости в одной полосе.
            TraceStyle::HistogramOverlaid { primaries } => {
                for (slot, primary) in (0u32..).zip(primaries.iter().take(self.planes as usize)) {
                    add_slot(slot, *primary);
                }
            }
            // Полосы: полоса `index` рисует плоскость `index` её цветом.
            TraceStyle::HistogramSections { colors } => {
                add_slot(
                    strip.index,
                    colors.get(strip.index as usize).copied().unwrap_or([0.0; 3]),
                );
            }
            TraceStyle::PlaneSections { .. }
            | TraceStyle::PlanesOverlaid { .. }
            | TraceStyle::ColorByPosition { .. }
            | TraceStyle::Cie(_) => return TracePixel::ABSENT,
        }
        blend.finish(&self.look)
    }
}

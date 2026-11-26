//! Цвет и прозрачность следа одного пикселя картинки прибора — по способу рисования ([`TraceStyle`]).
//!
//! Это эталон веток шейдера `scope_render.comp`: каждая ветка здесь соответствует ветке там, формулы те же.
//! Пиксель → ячейка целочисленно (как у шейдера), яркость — [`TraceReference::intensity`] с опорным числом СТОЛБЦА
//! ([`ColumnReferences`]): знаменатель берётся по столбцу ячейки, а не один на кадр.
//!
//! * секции: картинка делится на `planes` равных секций по ширине; секция показывает свою плоскость на весь
//!   диапазон столбцов, цвет — цвет плоскости;
//! * наложение: `column = x · columns / width` на всю ширину; яркость каждой плоскости `i_c` по общей лог-шкале;
//!   цвет = `Σ i_c · P_c`, делённый на свой наибольший канал (яркость задаёт альфа, а не цвет); альфа следа =
//!   `max_c(i_c) · trace_opacity`; серый кадр (три плоскости равны) даёт белый след, если `Σ P_c = (1, 1, 1)`;
//! * по месту: плоскость одна, цвет — `PositionPalette::colour_at(column, row)`, альфа — по счётчику ячейки.

use scope_contract::ScopeImageSize;

use crate::column_references::ColumnReferences;
use crate::grid::{GridShape, ScopeCounters};
use crate::trace_style::TraceStyle;

/// След пикселя: цвет экрана и ПРЯМАЯ альфа (нулевая альфа — цвет не важен).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct TracePixel {
    pub color: [f32; 3],
    pub alpha: f32,
}

impl TracePixel {
    /// Нет следа.
    const ABSENT: Self = Self {
        color: [0.0; 3],
        alpha: 0.0,
    };
}

/// Рисует след пикселей одной картинки.
pub(crate) struct TracePainter<'a> {
    counters: &'a ScopeCounters,
    shape: GridShape,
    style: &'a TraceStyle,
    references: &'a ColumnReferences,
    trace_opacity: f32,
}

impl<'a> TracePainter<'a> {
    pub(crate) fn new(
        counters: &'a ScopeCounters,
        style: &'a TraceStyle,
        references: &'a ColumnReferences,
        trace_opacity: f32,
    ) -> Self {
        Self {
            counters,
            shape: counters.shape(),
            style,
            references,
            trace_opacity,
        }
    }

    /// След пикселя `(x, y)` картинки размера `size`.
    pub(crate) fn pixel(&self, x: u32, y: u32, size: ScopeImageSize) -> TracePixel {
        let width = u64::from(size.width());
        let row = (u64::from(y) * u64::from(self.shape.rows()) / u64::from(size.height())) as u32;
        match self.style {
            TraceStyle::PlaneSections { colors } => self.section_pixel(colors, x, row, width),
            TraceStyle::PlanesOverlaid { primaries } => {
                let column = self.column_over_full_width(x, width);
                self.overlaid_pixel(primaries, row, column)
            }
            TraceStyle::ColorByPosition { palette } => {
                let column = self.column_over_full_width(x, width);
                let count = self.counters.count(0, row, column).unwrap_or(0);
                if count == 0 {
                    return TracePixel::ABSENT;
                }
                TracePixel {
                    color: palette.colour_at(column, row),
                    alpha: self.intensity(column, count) * self.trace_opacity,
                }
            }
        }
    }

    /// Яркость следа ячейки столбца `column` со счётчиком `count` по опорному числу ЭТОГО столбца. Столбца вне
    /// сетки нет (и счётчика у него нет), поэтому яркость нулевая.
    fn intensity(&self, column: u32, count: u32) -> f32 {
        self.references
            .of_column(column)
            .map_or(0.0, |reference| reference.intensity(count))
    }

    /// Столбец сетки для пикселя `x` при показе на ВСЮ ширину картинки.
    fn column_over_full_width(&self, x: u32, width: u64) -> u32 {
        (u64::from(x) * u64::from(self.shape.columns()) / width) as u32
    }

    /// Секции: плоскость и столбец внутри неё — целочисленно.
    fn section_pixel(&self, colors: &[[f32; 3]], x: u32, row: u32, width: u64) -> TracePixel {
        let planes = u64::from(self.shape.planes());
        let plane = u64::from(x) * planes / width;
        let column = ((u64::from(x) * planes - plane * width) * u64::from(self.shape.columns()) / width) as u32;
        let count = self.counters.count(plane as u32, row, column).unwrap_or(0);
        if count == 0 {
            return TracePixel::ABSENT;
        }
        TracePixel {
            // Цвет плоскости есть у каждой плоскости (длину списка проверяют границы показа); без него — без следа.
            color: colors.get(plane as usize).copied().unwrap_or([0.0; 3]),
            alpha: self.intensity(column, count) * self.trace_opacity,
        }
    }

    /// Наложение: три плоскости в одном пикселе.
    fn overlaid_pixel(&self, primaries: &[[f32; 3]; 3], row: u32, column: u32) -> TracePixel {
        let mut weighted = [0.0_f32; 3];
        let mut brightest = 0.0_f32;
        for (plane, primary) in (0u32..).zip(primaries) {
            let intensity = self.intensity(column, self.counters.count(plane, row, column).unwrap_or(0));
            for channel in 0..3 {
                weighted[channel] += intensity * primary[channel];
            }
            brightest = brightest.max(intensity);
        }
        let peak = weighted[0].max(weighted[1]).max(weighted[2]);
        if brightest <= 0.0 || peak <= 0.0 {
            return TracePixel::ABSENT;
        }
        TracePixel {
            color: weighted.map(|channel| channel / peak),
            alpha: brightest * self.trace_opacity,
        }
    }
}

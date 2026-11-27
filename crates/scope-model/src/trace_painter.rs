//! Цвет и прозрачность следа одного пикселя картинки прибора — по способу рисования ([`TraceStyle`]).
//!
//! Это эталон веток шейдера `scope_render.comp`: каждая ветка здесь соответствует ветке там, формулы те же.
//! Счёт пикселя — попадания на пиксель после переноса сетки в картинку ([`PictureDensity`], решение 322), яркость —
//! [`TraceReference::intensity`].
//!
//! * секции: картинка делится на `planes` равных секций по ширине; секция показывает свою плоскость, цвет — цвет
//!   плоскости;
//! * наложение: яркость каждой плоскости `i_c` по общей лог-шкале; цвет = `Σ i_c · P_c`, делённый на свой
//!   наибольший канал (яркость задаёт альфа, а не цвет); альфа следа = `max_c(i_c) · trace_opacity`; серый кадр
//!   (три плоскости равны) даёт белый след, если `Σ P_c = (1, 1, 1)`;
//! * по месту: плоскость одна, цвет — `PositionPalette::colour_at_point` в центре пикселя (в координатах клеток
//!   сетки), альфа — по счёту пикселя.

use scope_contract::ScopeImageSize;

use crate::grid::GridShape;
use crate::trace_density::{PictureDensity, Section};
use crate::trace_reference::TraceReference;
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
    density: &'a PictureDensity,
    shape: GridShape,
    size: ScopeImageSize,
    style: &'a TraceStyle,
    reference: TraceReference,
    trace_opacity: f32,
}

impl<'a> TracePainter<'a> {
    pub(crate) fn new(
        density: &'a PictureDensity,
        shape: GridShape,
        size: ScopeImageSize,
        style: &'a TraceStyle,
        reference: TraceReference,
        trace_opacity: f32,
    ) -> Self {
        Self {
            density,
            shape,
            size,
            style,
            reference,
            trace_opacity,
        }
    }

    /// След пикселя `(x, y)`.
    pub(crate) fn pixel(&self, x: u32, y: u32) -> TracePixel {
        match self.style {
            TraceStyle::PlaneSections { colors } => {
                let hits = self.density.hits(x, y, 0);
                if hits <= 0.0 {
                    return TracePixel::ABSENT;
                }
                let plane = Section::of_pixel(x, self.size.width(), self.shape.planes()).plane;
                TracePixel {
                    // Цвет плоскости есть у каждой плоскости (длину списка проверяют границы показа); без него — без следа.
                    color: colors.get(plane as usize).copied().unwrap_or([0.0; 3]),
                    alpha: self.reference.intensity(hits) * self.trace_opacity,
                }
            }
            TraceStyle::PlanesOverlaid { primaries } => self.overlaid_pixel(primaries, x, y),
            TraceStyle::ColorByPosition { palette } => {
                let hits = self.density.hits(x, y, 0);
                if hits <= 0.0 {
                    return TracePixel::ABSENT;
                }
                // Центр пикселя в координатах клеток сетки (центр клетки i — в i + 0,5).
                let column = (x as f32 + 0.5) * self.shape.columns() as f32 / self.size.width() as f32;
                let row = (y as f32 + 0.5) * self.shape.rows() as f32 / self.size.height() as f32;
                TracePixel {
                    color: palette.colour_at_point(column, row),
                    alpha: self.reference.intensity(hits) * self.trace_opacity,
                }
            }
        }
    }

    /// Наложение: все плоскости в одном пикселе.
    fn overlaid_pixel(&self, primaries: &[[f32; 3]; 3], x: u32, y: u32) -> TracePixel {
        let mut weighted = [0.0_f32; 3];
        let mut brightest = 0.0_f32;
        for (slot, primary) in (0u32..).zip(primaries) {
            let intensity = self.reference.intensity(self.density.hits(x, y, slot));
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

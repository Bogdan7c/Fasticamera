//! Способ рисования следа прибора: КАК счётчики превращаются в цвет и прозрачность пикселя.
//!
//! Способ — свойство правила показа, а не вида прибора: решает его [`crate::ResolvedScope::trace_paint`]
//! (правило называет свой способ). Эталон ([`crate::render`]) и шейдер показа (`scope_render.comp`) обязаны
//! рисовать одинаково, поэтому способов ровно столько, сколько веток в шейдере, а в шейдер способ уходит одним
//! словом (`trace.w`); перевод способа в слово — одна функция-словарь в `isp-vulkan` (`match` без `_`).

/// Способ рисования следа.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TracePaint {
    /// Картинка делится на равные секции по ширине, по одной на плоскость счётчиков; каждая секция красится
    /// ЦВЕТОМ СВОЕЙ плоскости, яркость — по счётчику ячейки (классический RGB parade).
    PlaneSections,
    /// Все плоскости рисуются в одни и те же пиксели (на всю ширину): цвет — смесь цветов плоскостей по их
    /// яркостям, прозрачность — по самой яркой плоскости (RGB waveform).
    PlanesOverlaid,
    /// Одна плоскость; цвет пикселя — цвет МЕСТА ячейки на картинке (вектороскоп красится по положению),
    /// яркость — по счётчику ячейки.
    ColorByPosition,
    /// Гистограмма, плоскости НАЛОЖЕНЫ (режим RGB, решение 337): графики трёх плоскостей в одной полосе на всю высоту;
    /// заливки складываются по цвету (прозрачность — по самой высокой), линии верха рисуются поверх своими цветами (где
    /// совпали — белый). Высота — по шкале [`crate::HistogramHeight`], а не по лог-яркости счёта.
    HistogramOverlaid,
    /// Гистограмма, плоскости ПОЛОСАМИ (режимы PARADE и Y): по полосе на плоскость, одна над другой, каждая на всю
    /// ширину со своей базовой линией и цветом плоскости; у яркости полоса одна, на всю высоту.
    HistogramSections,
    /// Диаграмма CIE: одна плоскость, как у вектороскопа, но цвет точки — её цветность (или белый), под следом бледная
    /// заливка локуса. Рисует отдельный вариант шейдера показа (`SCOPE_RENDER_CIE`).
    Cie,
}

impl TracePaint {
    /// Все способы (для тестов, которые обязаны пройти по каждому).
    pub const ALL: [TracePaint; 6] = [
        Self::PlaneSections,
        Self::PlanesOverlaid,
        Self::ColorByPosition,
        Self::HistogramOverlaid,
        Self::HistogramSections,
        Self::Cie,
    ];

    /// Рисуется ли след СТОЛБИКАМИ (шкала высоты, один ряд счётчиков) вместо плотности «строка × столбец».
    pub fn draws_bars(self) -> bool {
        match self {
            Self::HistogramOverlaid | Self::HistogramSections => true,
            Self::PlaneSections | Self::PlanesOverlaid | Self::ColorByPosition | Self::Cie => false,
        }
    }

    /// Сколько плоскостей счётчиков способ требует от прибора; `None` — любое число (секции делятся на сколько
    /// есть).
    pub fn required_planes(self) -> Option<u32> {
        match self {
            Self::PlaneSections | Self::HistogramSections => None,
            Self::PlanesOverlaid | Self::HistogramOverlaid => Some(3),
            Self::ColorByPosition | Self::Cie => Some(1),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_paint_names_its_plane_requirement() {
        assert_eq!(TracePaint::PlaneSections.required_planes(), None);
        assert_eq!(TracePaint::PlanesOverlaid.required_planes(), Some(3));
        assert_eq!(TracePaint::ColorByPosition.required_planes(), Some(1));
        assert_eq!(TracePaint::HistogramOverlaid.required_planes(), Some(3));
        assert_eq!(TracePaint::HistogramSections.required_planes(), None);
        assert_eq!(TracePaint::Cie.required_planes(), Some(1));
        assert_eq!(TracePaint::ALL.len(), 6);
    }

    #[test]
    fn only_the_histogram_paints_draw_bars() {
        let bars: Vec<TracePaint> = TracePaint::ALL.into_iter().filter(|paint| paint.draws_bars()).collect();
        assert_eq!(bars, [TracePaint::HistogramOverlaid, TracePaint::HistogramSections]);
        assert!(!TracePaint::Cie.draws_bars());
    }
}

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
}

impl TracePaint {
    /// Все способы (для тестов, которые обязаны пройти по каждому).
    pub const ALL: [TracePaint; 3] = [Self::PlaneSections, Self::PlanesOverlaid, Self::ColorByPosition];

    /// Сколько плоскостей счётчиков способ требует от прибора; `None` — любое число (секции делятся на сколько
    /// есть).
    pub fn required_planes(self) -> Option<u32> {
        match self {
            Self::PlaneSections => None,
            Self::PlanesOverlaid => Some(3),
            Self::ColorByPosition => Some(1),
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
        assert_eq!(TracePaint::ALL.len(), 3);
    }
}

//! Способ рисования следа ВМЕСТЕ со всеми данными, которые ему нужны: цвета секций, основные цвета наложения,
//! таблица цветов по месту. Один тип отдаёт и эталон ([`crate::render`]), и GPU-сторона (`GpuScopeRule`), поэтому
//! способ без своих данных (наложение без трёх цветов, «по месту» без таблицы) не существует по построению.

use crate::position_palette::PositionPalette;
use crate::trace_paint::TracePaint;

/// Способ рисования следа и его данные.
#[derive(Debug, Clone, PartialEq)]
pub enum TraceStyle {
    /// Секции по плоскостям: цвет секции — цвет её плоскости (по одному на плоскость сетки).
    PlaneSections {
        /// Цвета плоскостей (цвета экрана).
        colors: Vec<[f32; 3]>,
    },
    /// Плоскости наложены: смесь основных цветов плоскостей R, G, B по их яркостям.
    PlanesOverlaid {
        /// Основные цвета трёх плоскостей (цвета экрана).
        primaries: [[f32; 3]; 3],
    },
    /// Цвет точки — цвет её места на картинке.
    ColorByPosition {
        /// Таблица цветов по углам (в коробке: она заметно больше остальных вариантов).
        palette: Box<PositionPalette>,
    },
}

impl TraceStyle {
    /// Способ рисования, без данных.
    pub fn paint(&self) -> TracePaint {
        match self {
            Self::PlaneSections { .. } => TracePaint::PlaneSections,
            Self::PlanesOverlaid { .. } => TracePaint::PlanesOverlaid,
            Self::ColorByPosition { .. } => TracePaint::ColorByPosition,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::position_palette::{POSITION_PALETTE_ANGLES, PositionPaletteEntry};

    #[test]
    fn style_names_the_matching_paint() {
        let entry = PositionPaletteEntry {
            edge_colour: [0.0; 3],
            edge_radius_cells: 1.0,
        };
        let palette = PositionPalette::new([entry; POSITION_PALETTE_ANGLES], [0.5; 3], 4.5);
        assert_eq!(
            TraceStyle::PlaneSections { colors: vec![[1.0; 3]] }.paint(),
            TracePaint::PlaneSections
        );
        assert_eq!(
            TraceStyle::PlanesOverlaid {
                primaries: [[0.0; 3]; 3]
            }
            .paint(),
            TracePaint::PlanesOverlaid
        );
        assert_eq!(
            TraceStyle::ColorByPosition {
                palette: Box::new(palette)
            }
            .paint(),
            TracePaint::ColorByPosition
        );
    }
}

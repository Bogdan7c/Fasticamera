//! Вёрстка одной строки по запечённым таблицам (решение 87).
//!
//! Алгоритм намеренно простой: продвижение символа + кернинг пары + трекинг. Без лигатур и контекстных
//! форм — набор подписей камеры в них не нуждается; тест сверяет результат с полной вёрсткой HarfBuzz.

use super::tables::FigureTable;
use super::{Figures, PlacedGlyph, TextError, TextRun, TextStyle};
use crate::atlas;
use crate::geometry::{PxPoint, PxRect, PxSize};

/// Верстает строку в одну линию слева направо.
///
/// Координаты глифов — в пикселях относительно начала строки, базовая линия `y = 0`, вниз — плюс.
/// Трекинг добавляется после **каждого** глифа, включая последний (как `letter-spacing` в CSS), поэтому
/// `advance_px` включает его. Символ вне набора — [`TextError::UnsupportedCharacter`], «тофу» не рисуем.
pub fn layout_line(text: &str, style: &TextStyle) -> Result<TextRun, TextError> {
    let tables = style.face.tables();
    let figure_table: &FigureTable = match style.figures {
        Figures::Proportional => &tables.proportional,
        Figures::Tabular => &tables.tabular,
    };
    let units_to_px = style.size_px / f32::from(tables.units_per_em);
    let tracking_px = style.tracking_em * style.size_px;

    let mut glyphs = Vec::with_capacity(text.len());
    // Позиция пера копится в целых единицах шрифта (без накопления ошибки f32), перевод в px — на выходе.
    let mut pen_units: i32 = 0;
    let mut glyph_count: usize = 0;
    let mut previous: Option<char> = None;
    for (index, character) in text.chars().enumerate() {
        let entry = figure_table
            .glyph(character)
            .ok_or(TextError::UnsupportedCharacter { ch: character, index })?;
        if let Some(previous) = previous {
            pen_units += figure_table.kerning_units(previous, character);
        }
        if let Some(cell) = entry.cell {
            let pen_px = pen_units as f32 * units_to_px + glyph_count as f32 * tracking_px;
            glyphs.push(PlacedGlyph {
                region: atlas::region_of_cell(cell.atlas_x, cell.atlas_y, cell.width_texels, cell.height_texels),
                quad: PxRect::new(
                    PxPoint::new(pen_px + cell.left_em * style.size_px, cell.top_em * style.size_px),
                    PxSize::new(
                        cell.width_texels as f32 / atlas::EM_TEXELS_F32 * style.size_px,
                        cell.height_texels as f32 / atlas::EM_TEXELS_F32 * style.size_px,
                    ),
                ),
            });
        }
        pen_units += entry.advance_units;
        glyph_count += 1;
        previous = Some(character);
    }
    Ok(TextRun {
        glyphs,
        advance_px: pen_units as f32 * units_to_px + glyph_count as f32 * tracking_px,
        cap_height_px: tables.cap_height_units as f32 * units_to_px,
    })
}

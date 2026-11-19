//! Иконки, запечённые в SDF-атлас при сборке (решения 81, 88).
//!
//! Набор задаёт манифест ассетов `assets/bake.json`, перечисление [`IconId`] генерируется при сборке.
//! Иконка занимает в атласе квадратную ячейку: квадрат `viewBox` исходного SVG плюс поле расстояний
//! (`spread`) со всех сторон. Экран кладёт квадрат иконки в нужное место и размер, а [`IconGlyph::quad_in_square`]
//! говорит, как ячейка выступает за этот квадрат.

use crate::atlas::{self, AtlasRegion};
use crate::geometry::{PxPoint, PxRect, PxSize};

/// Положение ячейки иконки в атласе (сгенерировано).
#[derive(Debug, Clone, Copy)]
struct IconCellEntry {
    atlas_x: u32,
    atlas_y: u32,
    size_texels: u32,
}

include!(concat!(env!("OUT_DIR"), "/icon_tables.rs"));

/// Иконка в атласе.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct IconGlyph {
    /// Область атласа, включая поле расстояний.
    pub region: AtlasRegion,
    /// Прямоугольник области **в долях квадрата viewBox иконки**: `(0, 0)–(1, 1)` — сам квадрат,
    /// отрицательные и большие единицы — поле расстояний, выступающее за него.
    pub quad_in_square: PxRect,
}

/// Иконка по идентификатору.
pub fn icon(id: IconId) -> IconGlyph {
    // Индекс совпадает с порядком `IconId::ALL`, по нему же сгенерирован `ICON_CELLS`.
    let cell = ICON_CELLS[id as usize];
    let padding = atlas::SPREAD_TEXELS_F32 / atlas::ICON_TEXELS_F32;
    let extent = cell.size_texels as f32 / atlas::ICON_TEXELS_F32;
    IconGlyph {
        region: atlas::region_of_cell(cell.atlas_x, cell.atlas_y, cell.size_texels, cell.size_texels),
        quad_in_square: PxRect::new(PxPoint::new(-padding, -padding), PxSize::new(extent, extent)),
    }
}

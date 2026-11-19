//! Детерминированная упаковка ячеек в атлас «полками».
//!
//! Ячейки сортируются по убыванию высоты (при равенстве — по ширине, затем по порядку входа) и
//! кладутся слева направо; когда строка заполнена, начинается новая полка. Одинаковый вход всегда даёт
//! одинаковое размещение: порядок входа задаёт вызывающий и он стабилен (без `HashMap`).

use super::error::{BakeError, BakeResult};

/// Размер ячейки, которую надо разместить.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CellSize {
    pub width: u32,
    pub height: u32,
}

/// Место ячейки в атласе, текселей от левого верхнего угла.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CellPlacement {
    pub x: u32,
    pub y: u32,
}

/// Итог упаковки: места ячеек в порядке входа и итоговая высота атласа.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Packing {
    pub placements: Vec<CellPlacement>,
    pub atlas_height: u32,
}

/// Размещает ячейки в полосе шириной `atlas_width`; между ячейками — зазор `gutter` текселей.
pub fn pack_shelves(cells: &[CellSize], atlas_width: u32, gutter: u32) -> BakeResult<Packing> {
    let mut order: Vec<usize> = (0..cells.len()).collect();
    order.sort_by_key(|&index| {
        (
            std::cmp::Reverse(cells[index].height),
            std::cmp::Reverse(cells[index].width),
            index,
        )
    });

    let mut placements = vec![CellPlacement { x: 0, y: 0 }; cells.len()];
    let mut cursor_x = 0;
    let mut shelf_top = 0;
    let mut shelf_height = 0;
    for index in order {
        let cell = cells[index];
        if cell.width > atlas_width {
            return Err(BakeError::new(format!(
                "ячейка шире атласа: {} > {atlas_width} текселей",
                cell.width
            )));
        }
        if cursor_x + cell.width > atlas_width {
            // Полка заполнена: следующая начинается под самой высокой ячейкой этой полки.
            shelf_top += shelf_height + gutter;
            cursor_x = 0;
            shelf_height = 0;
        }
        placements[index] = CellPlacement {
            x: cursor_x,
            y: shelf_top,
        };
        cursor_x += cell.width + gutter;
        shelf_height = shelf_height.max(cell.height);
    }
    Ok(Packing {
        placements,
        atlas_height: shelf_top + shelf_height,
    })
}

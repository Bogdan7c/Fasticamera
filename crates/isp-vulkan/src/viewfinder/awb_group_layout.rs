//! Лежит ли КАЖДАЯ рабочая группа ISP видоискателя целиком в одной плитке статистики AWB.
//!
//! # Зачем
//! Если да ([`AwbGroupLayout::SingleTilePerGroup`]), группа складывает вклады своих нитей в одну плитку, и
//! шейдеру не нужны ни определение плитки по нитям, ни окно плиток в shared-памяти с тремя `barrier()`:
//! вариант с константой `AWB_GROUPS_IN_ONE_TILE = true` содержит только короткий путь «5 слов в shared → одна
//! запись в глобальный буфер на поле». Замер на Adreno 740 (сессия 16a, пакет I): кадр со статистикой 12,6 мс
//! → ~7,4 мс, пока в шейдере одновременно живут оба пути, цена статистики выше вдвое.
//! Если нет ([`AwbGroupLayout::General`]) — прежний общий путь (он верен для любой геометрии).
//!
//! # Как решается
//! Решает CPU по геометрии кадра ДО записи команд: те же формулы, что у шейдера
//! (`cell_of_output` — поворот выхода в ячейки сенсора, `tile_of_cell` — целая сетка плиток), но не по нитям,
//! а по краям групп. Условие «все ячейки группы в одной плитке» разделяется по осям ячеек: номер плитки
//! `floor(c · G / N)` не убывает по каждой оси, поэтому достаточно, чтобы наименьшая и наибольшая ячейка группы
//! по этой оси лежали в одной плитке.

use isp_model::statistics::TileGrid;

/// Как рабочие группы ложатся на плитки статистики AWB (выбор варианта пайплайна).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AwbGroupLayout {
    /// Какая-то группа задевает несколько плиток: нужен общий путь (окно плиток в shared-памяти).
    General,
    /// Каждая группа целиком в одной плитке: достаточно короткого пути.
    SingleTilePerGroup,
}

impl AwbGroupLayout {
    /// Значение specialization-константы `AWB_GROUPS_IN_ONE_TILE` (`constant_id = 16`).
    pub const fn specialization_value(self) -> u32 {
        match self {
            Self::General => 0,
            Self::SingleTilePerGroup => 1,
        }
    }
}

/// Вход решения: что известно о раскладке dispatch до его записи.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DispatchShape {
    /// Размер прямого (upright) выхода, пиксели: одна нить — один пиксель.
    pub output: (u32, u32),
    /// Размер изображения ячеек кадрирования в ориентации сенсора (ширина, высота в ячейках 2×2).
    pub cells: (u32, u32),
    /// Поворот сенсора, четверти по часовой (0..=3).
    pub quarter_turns: u32,
    /// Размер рабочей группы (нитей по X и Y).
    pub group: (u32, u32),
    /// Сетка плиток статистики.
    pub grid: TileGrid,
}

/// Одна ось выхода и то, какую ось ячеек она питает.
#[derive(Debug, Clone, Copy)]
struct AxisMapping {
    /// Ось ячеек: `true` — X, `false` — Y.
    feeds_cell_x: bool,
    /// Нумерация ячеек идёт навстречу пикселям выхода (`cells − 1 − u`).
    reversed: bool,
}

/// Отображение осей выхода (X, Y) в оси ячеек для поворота `quarter_turns` — зеркало `cell_of_output`
/// в `viewfinder_isp.glsl`.
fn axis_mappings(quarter_turns: u32) -> (AxisMapping, AxisMapping) {
    let map = |feeds_cell_x, reversed| AxisMapping { feeds_cell_x, reversed };
    match quarter_turns % 4 {
        1 => (map(false, true), map(true, false)),
        2 => (map(true, true), map(false, true)),
        3 => (map(false, false), map(true, true)),
        _ => (map(true, false), map(false, false)),
    }
}

impl DispatchShape {
    /// Раскладка групп по плиткам (см. модуль).
    pub fn group_layout(&self) -> AwbGroupLayout {
        let (mapping_x, mapping_y) = axis_mappings(self.quarter_turns);
        let axes = [
            (self.output.0, self.group.0, mapping_x),
            (self.output.1, self.group.1, mapping_y),
        ];
        let every_group_in_one_tile = axes.into_iter().all(|(extent, group_size, mapping)| {
            let (cells, tiles) = if mapping.feeds_cell_x {
                (self.cells.0, self.grid.columns)
            } else {
                (self.cells.1, self.grid.rows)
            };
            axis_groups_stay_in_one_tile(extent, group_size, mapping.reversed, cells, tiles)
        });
        if every_group_in_one_tile {
            AwbGroupLayout::SingleTilePerGroup
        } else {
            AwbGroupLayout::General
        }
    }
}

/// Все ли группы вдоль одной оси выхода (протяжённость `extent`, группа `group_size` нитей) попадают каждая
/// в одну плитку оси ячеек (`cells` ячеек, `tiles` плиток).
fn axis_groups_stay_in_one_tile(extent: u32, group_size: u32, reversed: bool, cells: u32, tiles: u32) -> bool {
    if extent == 0 || group_size == 0 || cells == 0 || tiles == 0 {
        return false;
    }
    let tile_of = |cell: u32| u64::from(cell) * u64::from(tiles) / u64::from(cells);
    let cell_of = |pixel: u32| if reversed { cells - 1 - pixel } else { pixel };
    (0..extent.div_ceil(group_size)).all(|group| {
        let first = group * group_size;
        // Крайняя группа выходит за картинку: нити вне выхода в плитку ничего не дают (как в шейдере).
        let last = first.saturating_add(group_size - 1).min(extent - 1);
        let (cell_a, cell_b) = (cell_of(first), cell_of(last));
        tile_of(cell_a.min(cell_b)) == tile_of(cell_a.max(cell_b))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const GRID: TileGrid = TileGrid { columns: 32, rows: 24 };

    /// Продуктовый кадр OnePlus 11: 4096×3072 пикселей RAW, весь сенсор, поворот 90°.
    fn product_shape(group: (u32, u32)) -> DispatchShape {
        DispatchShape {
            output: (1536, 2048),
            cells: (2048, 1536),
            quarter_turns: 1,
            group,
            grid: GRID,
        }
    }

    /// Прямой перебор по нитям, как делал бы шейдер: эталон для проверки быстрого решения.
    fn brute_force_layout(shape: &DispatchShape) -> AwbGroupLayout {
        let (output_width, output_height) = shape.output;
        let (cells_width, cells_height) = shape.cells;
        let cell_of_output = |x: u32, y: u32| match shape.quarter_turns % 4 {
            1 => (y, cells_height - 1 - x),
            2 => (cells_width - 1 - x, cells_height - 1 - y),
            3 => (cells_width - 1 - y, x),
            _ => (x, y),
        };
        let tile_of = |cell: (u32, u32)| {
            (
                u64::from(cell.0) * u64::from(shape.grid.columns) / u64::from(cells_width),
                u64::from(cell.1) * u64::from(shape.grid.rows) / u64::from(cells_height),
            )
        };
        let groups_x = output_width.div_ceil(shape.group.0);
        let groups_y = output_height.div_ceil(shape.group.1);
        for group_y in 0..groups_y {
            for group_x in 0..groups_x {
                let mut tiles = std::collections::HashSet::new();
                for y in group_y * shape.group.1..((group_y + 1) * shape.group.1).min(output_height) {
                    for x in group_x * shape.group.0..((group_x + 1) * shape.group.0).min(output_width) {
                        tiles.insert(tile_of(cell_of_output(x, y)));
                    }
                }
                if tiles.len() > 1 {
                    return AwbGroupLayout::General;
                }
            }
        }
        AwbGroupLayout::SingleTilePerGroup
    }

    #[test]
    fn product_frame_with_power_of_two_groups_fits_one_tile_per_group() {
        for group in [(16, 8), (4, 32), (8, 16), (16, 16), (32, 8)] {
            assert_eq!(
                product_shape(group).group_layout(),
                AwbGroupLayout::SingleTilePerGroup,
                "группа {group:?}"
            );
        }
    }

    #[test]
    fn group_that_straddles_a_tile_boundary_needs_the_general_path() {
        // Плитка 64×64 ячеек; группа 48 нитей не делит 64 → какая-то группа стоит на стыке плиток.
        assert_eq!(product_shape((48, 8)).group_layout(), AwbGroupLayout::General);
        // Другая сетка: плитки по 2048 / 24 ≈ 85,3 ячейки — границы не кратны группе.
        let shape = DispatchShape {
            grid: TileGrid { columns: 24, rows: 24 },
            ..product_shape((16, 8))
        };
        assert_eq!(shape.group_layout(), AwbGroupLayout::General);
    }

    #[test]
    fn crop_with_fractional_tiles_needs_the_general_path() {
        // Кадрирование 1000×750 ячеек: плитка 31,25 ячейки по X.
        let shape = DispatchShape {
            output: (750, 1000),
            cells: (1000, 750),
            ..product_shape((16, 8))
        };
        assert_eq!(shape.group_layout(), AwbGroupLayout::General);
    }

    #[test]
    fn degenerate_shapes_never_claim_single_tile() {
        let zero_group = DispatchShape {
            group: (0, 8),
            ..product_shape((16, 8))
        };
        assert_eq!(zero_group.group_layout(), AwbGroupLayout::General);
        let no_cells = DispatchShape {
            cells: (0, 1536),
            ..product_shape((16, 8))
        };
        assert_eq!(no_cells.group_layout(), AwbGroupLayout::General);
    }

    #[test]
    fn fast_decision_equals_brute_force_over_threads_for_every_rotation() {
        // Малые кадры, чтобы перебор нитей был дешёвым: разные поворот, группа, сетка и «кривые» размеры.
        let cell_sizes = [(64_u32, 48_u32), (96, 64), (60, 45), (128, 96), (50, 50)];
        let grids = [
            TileGrid { columns: 4, rows: 3 },
            TileGrid { columns: 8, rows: 6 },
            TileGrid { columns: 5, rows: 5 },
            TileGrid { columns: 32, rows: 24 },
        ];
        let groups = [(16_u32, 8_u32), (4, 32), (8, 8), (5, 7), (32, 32), (1, 1)];
        let mut checked = 0;
        for quarter_turns in 0..4 {
            for &cells in &cell_sizes {
                let output = if quarter_turns % 2 == 1 {
                    (cells.1, cells.0)
                } else {
                    cells
                };
                for &grid in &grids {
                    for &group in &groups {
                        let shape = DispatchShape {
                            output,
                            cells,
                            quarter_turns,
                            group,
                            grid,
                        };
                        assert_eq!(shape.group_layout(), brute_force_layout(&shape), "{shape:?}");
                        checked += 1;
                    }
                }
            }
        }
        assert_eq!(checked, 4 * 5 * 4 * 6, "перебрана вся матрица");
    }

    #[test]
    fn matrix_contains_both_outcomes_so_the_comparison_is_not_trivial() {
        let outcomes: std::collections::HashSet<AwbGroupLayout> = [(16_u32, 8_u32), (5, 7)]
            .into_iter()
            .map(|group| {
                DispatchShape {
                    output: (64, 48),
                    cells: (64, 48),
                    quarter_turns: 0,
                    group,
                    grid: TileGrid { columns: 4, rows: 3 },
                }
                .group_layout()
            })
            .collect();
        assert_eq!(outcomes.len(), 2);
    }

    #[test]
    fn specialization_values_are_distinct_flags() {
        assert_eq!(AwbGroupLayout::General.specialization_value(), 0);
        assert_eq!(AwbGroupLayout::SingleTilePerGroup.specialization_value(), 1);
    }
}

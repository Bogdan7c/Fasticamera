//! Таблица цветов «по месту» для вектороскопа: цвет точки определяется её положением на картинке.
//!
//! Формула ОДНА для эталона ([`PositionPalette::colour_at`]) и шейдера показа (`scope_render.comp`,
//! `position_color`): оба читают одну и ту же таблицу, поэтому цвет не расходится. Таблица строится на CPU по
//! осям прибора (своя для Y'CbCr и для OKLab, модуль `vectorscope_palette`) и отдаётся GPU как есть.
//!
//! # Устройство
//! * Углы делят круг на [`POSITION_PALETTE_ANGLES`] равных долей; запись `i` лежит на угле `i · 2π / N`,
//!   считая от оси «вправо» против часовой стрелки (как математический угол при оси «вверх»).
//! * Запись — цвет на КРАЮ (rgb) и радиус края `r_lim` в клетках: дальше этого радиуса цвет не меняется
//!   (цвет «упёрся» в границу охвата экрана или сетки).
//! * Нейтраль — цвет центра: серое той же светлоты, что и цвета по кругу.
//! * Для клетки `(column, row)` сетки `size × size`: `d = (column + 0,5 − centre, centre − (row + 0,5))`,
//!   `r = |d|`, угол = `atan2(d.y, d.x)`; запись берётся линейной интерполяцией двух соседних по углу (по кругу);
//!   цвет = `mix(нейтраль, край(угол), clamp(r / r_lim(угол), 0, 1))`.
//! * Центр: центральная клетка даёт `d = (0, 0)`, а `atan(0, 0)` в GLSL не определён (Rust даёт 0; на другом
//!   GPU там может оказаться NaN). Поэтому при `r ≤ ε` цвет — нейтраль, и ветка стоит ДО вычисления угла и
//!   в эталоне, и в шейдере.

/// Сколько углов в таблице.
pub const POSITION_PALETTE_ANGLES: usize = 128;

/// Радиус (клеток), не больше которого точка считается центром и красится нейтралью без вычисления угла.
pub const NEUTRAL_RADIUS_EPSILON_CELLS: f32 = 1.0e-3;

/// Наименьший радиус края, на который делится `r`: защита от деления на ноль у вырожденной записи.
pub const MIN_EDGE_RADIUS_CELLS: f32 = 1.0e-4;

/// Одна запись таблицы: цвет края и радиус края.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PositionPaletteEntry {
    /// Цвет на краю (цвета экрана, каналы 0..=1).
    pub edge_colour: [f32; 3],
    /// Радиус края, клеток сетки.
    pub edge_radius_cells: f32,
}

/// Таблица цветов по положению для квадратной сетки вектороскопа.
#[derive(Debug, Clone, PartialEq)]
pub struct PositionPalette {
    entries: [PositionPaletteEntry; POSITION_PALETTE_ANGLES],
    neutral: [f32; 3],
    centre_cells: f32,
}

impl PositionPalette {
    /// Таблица из готовых записей. `centre_cells` — координата центра в клетках (`size / 2 + 0,5`).
    pub fn new(entries: [PositionPaletteEntry; POSITION_PALETTE_ANGLES], neutral: [f32; 3], centre_cells: f32) -> Self {
        Self {
            entries,
            neutral,
            centre_cells,
        }
    }

    /// Записи по углам.
    pub fn entries(&self) -> &[PositionPaletteEntry; POSITION_PALETTE_ANGLES] {
        &self.entries
    }

    /// Цвет центра.
    pub fn neutral(&self) -> [f32; 3] {
        self.neutral
    }

    /// Координата центра, клеток.
    pub fn centre_cells(&self) -> f32 {
        self.centre_cells
    }

    /// Цвет клетки `(column, row)`: эталон формулы шейдера (`position_color`).
    pub fn colour_at(&self, column: u32, row: u32) -> [f32; 3] {
        let dx = column as f32 + 0.5 - self.centre_cells;
        let dy = self.centre_cells - (row as f32 + 0.5);
        let radius = (dx * dx + dy * dy).sqrt();
        // Центр: угол у нулевого вектора не определён, поэтому ветка стоит ДО atan2 (см. описание модуля).
        // Сравнение записано так, чтобы NaN тоже попал в нейтраль, а не в угол.
        if radius.is_nan() || radius <= NEUTRAL_RADIUS_EPSILON_CELLS {
            return self.neutral;
        }
        let mut turns = dy.atan2(dx) / std::f32::consts::TAU;
        if turns < 0.0 {
            turns += 1.0;
        }
        let position = turns * POSITION_PALETTE_ANGLES as f32;
        let base = position.floor();
        let fraction = position - base;
        let first = base as usize % POSITION_PALETTE_ANGLES;
        let second = (first + 1) % POSITION_PALETTE_ANGLES;
        let (low, high) = (&self.entries[first], &self.entries[second]);
        let edge_radius = mix(low.edge_radius_cells, high.edge_radius_cells, fraction).max(MIN_EDGE_RADIUS_CELLS);
        let reach = (radius / edge_radius).clamp(0.0, 1.0);
        std::array::from_fn(|channel| {
            let edge = mix(low.edge_colour[channel], high.edge_colour[channel], fraction);
            mix(self.neutral[channel], edge, reach)
        })
    }
}

/// `a·(1 − t) + b·t` — как `mix` в GLSL.
fn mix(from: f32, to: f32, fraction: f32) -> f32 {
    from * (1.0 - fraction) + to * fraction
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Таблица, где у угла `i` красный канал края равен `i / N`, зелёный 1, радиус края 10 клеток, нейтраль
    /// серая 0,5, центр в `16,5` (клетка 16 — центральная).
    fn ramp_palette() -> PositionPalette {
        let entries = std::array::from_fn(|index| PositionPaletteEntry {
            edge_colour: [index as f32 / POSITION_PALETTE_ANGLES as f32, 1.0, 0.0],
            edge_radius_cells: 10.0,
        });
        PositionPalette::new(entries, [0.5; 3], 16.5)
    }

    #[test]
    fn centre_cell_is_neutral_and_finite() {
        // Клетка (16, 16): d = (0, 0), угол не определён.
        assert_eq!(ramp_palette().colour_at(16, 16), [0.5; 3]);
    }

    #[test]
    fn centre_is_neutral_without_touching_the_angle_table_even_if_the_table_is_poisoned() {
        // Ветка «центр» стоит ДО вычисления угла: на GPU atan(0, 0) не определён и может дать NaN. Отравленная
        // таблица (NaN во всех записях) доказывает, что центр её не читает; клетка рядом с центром — читает.
        let poisoned = PositionPaletteEntry {
            edge_colour: [f32::NAN; 3],
            edge_radius_cells: f32::NAN,
        };
        let palette = PositionPalette::new([poisoned; POSITION_PALETTE_ANGLES], [0.5; 3], 16.5);
        assert_eq!(palette.colour_at(16, 16), [0.5; 3]);
        assert!(palette.colour_at(17, 16).iter().any(|channel| channel.is_nan()));
    }

    #[test]
    fn colour_walks_from_neutral_to_edge_colour_along_the_radius() {
        let palette = ramp_palette();
        // Вправо от центра, угол 0: запись 0 (красный 0, зелёный 1, синий 0); пять клеток из десяти — полпути.
        let halfway = palette.colour_at(16 + 5, 16);
        assert!((halfway[0] - 0.25).abs() < 1e-5, "{halfway:?}");
        assert!((halfway[1] - 0.75).abs() < 1e-5, "{halfway:?}");
        let at_edge = palette.colour_at(16 + 10, 16);
        assert!((at_edge[1] - 1.0).abs() < 1e-5, "{at_edge:?}");
        // За краем цвет не меняется.
        assert_eq!(palette.colour_at(16 + 14, 16), palette.colour_at(16 + 20, 16));
    }

    #[test]
    fn angle_interpolates_between_neighbouring_entries_around_the_circle() {
        let palette = ramp_palette();
        // Прямо вверх от центра (строка меньше): угол 90° = запись N/4, красный = 0,25.
        let up = palette.colour_at(16, 16 - 10);
        assert!((up[0] - 0.25).abs() < 1e-4, "{up:?}");
        // Прямо влево: угол 180° = запись N/2, красный 0,5.
        let left = palette.colour_at(16 - 11, 16);
        assert!((left[0] - 0.5).abs() < 1e-4, "{left:?}");
        // Чуть ниже оси «вправо» угол отрицательный и переходит через ноль круга к последним записям.
        let below_right = palette.colour_at(16 + 10, 16 + 1);
        assert!(
            below_right[0] > 0.9,
            "красный у последних записей круга: {below_right:?}"
        );
    }
}

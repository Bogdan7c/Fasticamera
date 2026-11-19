//! Запечённые таблицы шрифта: типы записей и сгенерированные данные (`build.rs`, решение 87).
//!
//! Данные — только числа: продвижения, рамки ячеек атласа и пары кернинга. Ни одного шрифтового
//! крейта в продукте нет. Единицы `*_units` — единицы шрифта (`units_per_em` на em).

/// Ячейка глифа в атласе и её положение относительно начала глифа.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct GlyphCellEntry {
    /// Левый верхний угол ячейки в атласе, текселей.
    pub atlas_x: u32,
    pub atlas_y: u32,
    /// Размер ячейки в текселях (включая поле расстояний).
    pub width_texels: u32,
    pub height_texels: u32,
    /// Левый край ячейки от начала глифа, em.
    pub left_em: f32,
    /// Верхний край ячейки от базовой линии, em; ось вниз, поэтому над базовой линией — минус.
    pub top_em: f32,
}

/// Символ: продвижение и ячейка (`None` у пробела).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct GlyphEntry {
    pub character: char,
    pub advance_units: i32,
    pub cell: Option<GlyphCellEntry>,
}

/// Поправка продвижения первой буквы пары, единицы шрифта.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct KernEntry {
    pub first: char,
    pub second: char,
    pub value_units: i32,
}

/// Глифы и кернинг одного набора цифр; обе таблицы отсортированы для бинарного поиска.
#[derive(Debug, Clone, Copy)]
pub(super) struct FigureTable {
    /// По возрастанию `character`.
    pub glyphs: &'static [GlyphEntry],
    /// По возрастанию `(first, second)`.
    pub kerning: &'static [KernEntry],
}

/// Все данные одного начертания.
#[derive(Debug, Clone, Copy)]
pub(super) struct FaceTables {
    pub units_per_em: u16,
    pub cap_height_units: i32,
    pub ascent_units: i32,
    pub descent_units: i32,
    pub proportional: FigureTable,
    pub tabular: FigureTable,
}

include!(concat!(env!("OUT_DIR"), "/font_tables.rs"));

impl FontFace {
    /// Таблицы начертания.
    pub(super) fn tables(self) -> &'static FaceTables {
        // Индекс совпадает с порядком `FontFace::ALL`, по нему же сгенерирован `FACE_TABLES`.
        &FACE_TABLES[self as usize]
    }
}

impl FigureTable {
    /// Запись символа; `None`, если символа нет в наборе.
    pub(super) fn glyph(&self, character: char) -> Option<&'static GlyphEntry> {
        self.glyphs
            .binary_search_by_key(&character, |entry| entry.character)
            .ok()
            .map(|index| &self.glyphs[index])
    }

    /// Кернинг пары в единицах шрифта; 0, если пары нет в таблице.
    pub(super) fn kerning_units(&self, first: char, second: char) -> i32 {
        self.kerning
            .binary_search_by(|entry| (entry.first, entry.second).cmp(&(first, second)))
            .map(|index| self.kerning[index].value_units)
            .unwrap_or(0)
    }
}

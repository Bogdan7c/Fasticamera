//! Раскладка цветного фильтра Bayer (CFA): какой цвет в каком пикселе.
//!
//! Мозаика повторяется квадратом 2×2. Позиции квадрата нумеруются в порядке строк:
//! 0 — (0, 0), 1 — (1, 0), 2 — (0, 1), 3 — (1, 1). В том же порядке Camera2 отдаёт
//! `ACAMERA_SENSOR_BLACK_LEVEL_PATTERN`, поэтому чёрный уровень берётся по этому индексу.

use camera_contract::raw_format::CfaLayout;

/// Основной цвет фильтра или канала RGB.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RgbChannel {
    Red,
    Green,
    Blue,
}

impl RgbChannel {
    /// Индекс канала в пикселе `[R, G, B]`.
    pub const fn index(self) -> usize {
        match self {
            RgbChannel::Red => 0,
            RgbChannel::Green => 1,
            RgbChannel::Blue => 2,
        }
    }
}

/// Позиция в квадрате 2×2 (0…3, порядок строк).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CfaCell(u8);

impl CfaCell {
    /// Позиция квадрата для пикселя (x, y).
    pub const fn of_pixel(x: u32, y: u32) -> Self {
        // Чётность x даёт столбец квадрата, чётность y — строку.
        Self(((y % 2) * 2 + (x % 2)) as u8)
    }

    /// Индекс в массивах «по позициям квадрата» (чёрный уровень и т.п.).
    pub const fn index(self) -> usize {
        self.0 as usize
    }
}

/// Мозаика вне Bayer 2×2 эталоном не поддерживается (Quad Bayer, RGBW — отдельный план).
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("раскладка CFA {layout:?} не Bayer 2×2: эталон ISP её не поддерживает")]
pub struct UnsupportedCfa {
    pub layout: CfaLayout,
}

/// Раскладка Bayer: цвет каждой из четырёх позиций квадрата 2×2.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BayerPattern {
    cells: [RgbChannel; 4],
}

impl BayerPattern {
    pub const RGGB: Self = Self::from_cells([RgbChannel::Red, RgbChannel::Green, RgbChannel::Green, RgbChannel::Blue]);
    pub const GRBG: Self = Self::from_cells([RgbChannel::Green, RgbChannel::Red, RgbChannel::Blue, RgbChannel::Green]);
    pub const GBRG: Self = Self::from_cells([RgbChannel::Green, RgbChannel::Blue, RgbChannel::Red, RgbChannel::Green]);
    /// Раскладка всех камер OnePlus 11 (CFA 3, 00 C7).
    pub const BGGR: Self = Self::from_cells([RgbChannel::Blue, RgbChannel::Green, RgbChannel::Green, RgbChannel::Red]);

    const fn from_cells(cells: [RgbChannel; 4]) -> Self {
        Self { cells }
    }

    /// Раскладка из словаря камеры; всё, что не Bayer 2×2, — честная ошибка, а не догадка.
    pub fn from_layout(layout: CfaLayout) -> Result<Self, UnsupportedCfa> {
        match layout {
            CfaLayout::Rggb => Ok(Self::RGGB),
            CfaLayout::Grbg => Ok(Self::GRBG),
            CfaLayout::Gbrg => Ok(Self::GBRG),
            CfaLayout::Bggr => Ok(Self::BGGR),
            CfaLayout::Rgb | CfaLayout::Mono | CfaLayout::Nir => Err(UnsupportedCfa { layout }),
        }
    }

    /// Цвет фильтра в пикселе (x, y).
    pub const fn channel_at(self, x: u32, y: u32) -> RgbChannel {
        self.cells[CfaCell::of_pixel(x, y).index()]
    }

    /// Цвет позиции квадрата.
    pub const fn channel_of_cell(self, cell: CfaCell) -> RgbChannel {
        self.cells[cell.index()]
    }

    /// Позиции квадрата одного цвета: у красного и синего — одна, у зелёного — две.
    pub fn cells_of(self, channel: RgbChannel) -> impl Iterator<Item = CfaCell> {
        (0u8..4)
            .map(CfaCell)
            .filter(move |cell| self.channel_of_cell(*cell) == channel)
    }

    /// Строка y содержит красные пиксели (а не синие). В Bayer каждая строка — либо
    /// «зелёный + красный», либо «зелёный + синий».
    pub const fn row_has_red(self, y: u32) -> bool {
        matches!(self.channel_at(0, y), RgbChannel::Red) || matches!(self.channel_at(1, y), RgbChannel::Red)
    }

    /// Столбец x содержит красные пиксели (а не синие).
    pub const fn column_has_red(self, x: u32) -> bool {
        matches!(self.channel_at(x, 0), RgbChannel::Red) || matches!(self.channel_at(x, 1), RgbChannel::Red)
    }
}

#[cfg(test)]
#[path = "cfa_tests.rs"]
mod tests;

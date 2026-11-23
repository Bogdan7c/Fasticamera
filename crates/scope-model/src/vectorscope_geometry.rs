//! Общая геометрия квадратной сетки вектороскопа (обе системы осей).
//!
//! Сетка `size × size`, ось «вправо» — столбцы, ось «вверх» — СТРОКИ ВНИЗ (строка 0 — верх). Центр —
//! ячейка `(size/2, size/2)`; она покрывает координаты `[−0,5; +0,5)` клетки вокруг нуля, поэтому серый
//! (координата ≈ 0 с любой ошибкой округления менее полуклетки) всегда попадает ровно в центр.
//! Полный размах цвета (`Cb`, `Cr` = ±0,5 или OKLab-цветность = `range`) при усилении 1 уходит на
//! `half_extent = size/2 − 1` клеток от центра: самые насыщенные цвета (100 % синий, красный) остаются
//! внутри сетки с запасом в одну клетку. Усиление больше 1 приближает картину, а то, что вышло за сетку, —
//! не попадание.

use crate::grid::{GridError, GridShape};

/// Наименьшая сторона сетки вектороскопа.
pub const MIN_VECTORSCOPE_SIZE: u32 = 8;

/// Наибольшая сторона сетки вектороскопа.
pub const MAX_VECTORSCOPE_SIZE: u32 = 1024;

/// Ошибка геометрии вектороскопа.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum VectorscopeGeometryError {
    /// Сторона вне `MIN..=MAX`.
    #[error("сторона вектороскопа {size} вне {MIN_VECTORSCOPE_SIZE}..={MAX_VECTORSCOPE_SIZE}")]
    SizeOutOfRange {
        /// Недопустимая сторона.
        size: u32,
    },
    /// Форма сетки недопустима.
    #[error(transparent)]
    Grid(#[from] GridError),
}

/// Размер и положение центра квадратной сетки.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VectorscopeGeometry {
    size: u32,
    shape: GridShape,
}

impl VectorscopeGeometry {
    /// Геометрия сетки `size × size`.
    pub fn try_new(size: u32) -> Result<Self, VectorscopeGeometryError> {
        if !(MIN_VECTORSCOPE_SIZE..=MAX_VECTORSCOPE_SIZE).contains(&size) {
            return Err(VectorscopeGeometryError::SizeOutOfRange { size });
        }
        Ok(Self {
            size,
            shape: GridShape::try_new(size, size, 1)?,
        })
    }

    /// Сторона сетки.
    pub fn size(&self) -> u32 {
        self.size
    }

    /// Форма сетки (одна плоскость).
    pub fn shape(&self) -> GridShape {
        self.shape
    }

    /// Номер центральной строки и столбца.
    pub fn centre_cell(&self) -> u32 {
        self.size / 2
    }

    /// Смещение «центр + полклетки» в клетках: к координате прибавляется оно, затем `floor` даёт клетку.
    pub fn centre_offset_cells(&self) -> f64 {
        f64::from(self.centre_cell()) + 0.5
    }

    /// Клеток от центра до края при усилении 1 (для полного размаха цвета).
    pub fn half_extent_cells(&self) -> f64 {
        f64::from(self.size / 2 - 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn geometry_matches_documented_numbers() {
        let geometry = VectorscopeGeometry::try_new(256).unwrap();
        assert_eq!(geometry.centre_cell(), 128);
        assert_eq!(geometry.centre_offset_cells(), 128.5);
        assert_eq!(geometry.half_extent_cells(), 127.0);
        assert_eq!(geometry.shape().cell_count(), 256 * 256);
    }

    #[test]
    fn size_limits_are_enforced() {
        assert_eq!(
            VectorscopeGeometry::try_new(MIN_VECTORSCOPE_SIZE - 1),
            Err(VectorscopeGeometryError::SizeOutOfRange {
                size: MIN_VECTORSCOPE_SIZE - 1
            })
        );
        assert!(VectorscopeGeometry::try_new(MAX_VECTORSCOPE_SIZE + 1).is_err());
    }
}

//! Сетка счётчиков прибора: форма и значения.

/// Наибольшая сторона сетки (столбцы, строки, плоскости).
pub const MAX_GRID_SIDE: u32 = 4096;

/// Наибольшее число ячеек сетки: 4 млн (16 МБ счётчиков `u32`) — бюджет памяти GPU-буфера.
pub const MAX_GRID_CELLS: u64 = 4 << 20;

/// Ошибка формы сетки.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum GridError {
    /// Нулевая сторона.
    #[error("сетка пуста: {columns}×{rows}×{planes}")]
    Empty {
        /// Столбцы.
        columns: u32,
        /// Строки.
        rows: u32,
        /// Плоскости.
        planes: u32,
    },
    /// Одна из сторон больше предела.
    #[error("сторона сетки больше {MAX_GRID_SIDE}: {columns}×{rows}×{planes}")]
    SideTooLarge {
        /// Столбцы.
        columns: u32,
        /// Строки.
        rows: u32,
        /// Плоскости.
        planes: u32,
    },
    /// Слишком много ячеек.
    #[error("в сетке {cells} ячеек, предел {MAX_GRID_CELLS}")]
    TooManyCells {
        /// Число ячеек.
        cells: u64,
    },
}

/// Форма сетки: столбцы × строки × плоскости (у parade плоскость — канал).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GridShape {
    columns: u32,
    rows: u32,
    planes: u32,
}

impl GridShape {
    /// Проверенная форма: стороны в `1..=MAX_GRID_SIDE`, ячеек не больше `MAX_GRID_CELLS`.
    pub fn try_new(columns: u32, rows: u32, planes: u32) -> Result<Self, GridError> {
        if columns == 0 || rows == 0 || planes == 0 {
            return Err(GridError::Empty { columns, rows, planes });
        }
        if columns > MAX_GRID_SIDE || rows > MAX_GRID_SIDE || planes > MAX_GRID_SIDE {
            return Err(GridError::SideTooLarge { columns, rows, planes });
        }
        let cells = u64::from(columns) * u64::from(rows) * u64::from(planes);
        if cells > MAX_GRID_CELLS {
            return Err(GridError::TooManyCells { cells });
        }
        Ok(Self { columns, rows, planes })
    }

    /// Столбцы.
    pub fn columns(self) -> u32 {
        self.columns
    }

    /// Строки (строка 0 — верх картинки прибора).
    pub fn rows(self) -> u32 {
        self.rows
    }

    /// Плоскости.
    pub fn planes(self) -> u32 {
        self.planes
    }

    /// Число ячеек.
    pub fn cell_count(self) -> usize {
        self.columns as usize * self.rows as usize * self.planes as usize
    }

    /// Индекс ячейки в плоском массиве: `(plane · rows + row) · columns + column`. `None`, если ячейка вне сетки.
    pub fn index_of(self, plane: u32, row: u32, column: u32) -> Option<usize> {
        if plane >= self.planes || row >= self.rows || column >= self.columns {
            return None;
        }
        Some(((plane as usize * self.rows as usize) + row as usize) * self.columns as usize + column as usize)
    }
}

/// Счётчики прибора: целые, поэтому CPU и GPU совпадают бит в бит.
///
/// Переполнения нет по построению: в одну ячейку один отсчёт даёт не больше одного попадания, отсчётов не
/// больше `MAX_SHOWN_PIXELS = 2^26`, а счётчик — `u32`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopeCounters {
    shape: GridShape,
    counts: Vec<u32>,
}

impl ScopeCounters {
    /// Нулевые счётчики формы `shape`.
    pub fn zeroed(shape: GridShape) -> Self {
        Self {
            shape,
            counts: vec![0; shape.cell_count()],
        }
    }

    /// Счётчики из готового массива (чтение GPU-буфера). Длина обязана совпасть с формой.
    pub fn from_counts(shape: GridShape, counts: Vec<u32>) -> Result<Self, CountsLengthMismatch> {
        if counts.len() != shape.cell_count() {
            return Err(CountsLengthMismatch {
                expected: shape.cell_count(),
                actual: counts.len(),
            });
        }
        Ok(Self { shape, counts })
    }

    /// Форма сетки.
    pub fn shape(&self) -> GridShape {
        self.shape
    }

    /// Все счётчики в порядке `(plane · rows + row) · columns + column`.
    pub fn counts(&self) -> &[u32] {
        &self.counts
    }

    /// Счётчик ячейки; `None` вне сетки.
    pub fn count(&self, plane: u32, row: u32, column: u32) -> Option<u32> {
        self.shape
            .index_of(plane, row, column)
            .and_then(|index| self.counts.get(index).copied())
    }

    /// Сумма всех счётчиков (число принятых попаданий).
    pub fn total(&self) -> u64 {
        self.counts.iter().map(|&count| u64::from(count)).sum()
    }

    /// Прибавляет попадание. `false`, если ячейка вне сетки (вызывающий обязан сообщить об ошибке правила).
    pub(crate) fn add_hit(&mut self, plane: u32, row: u32, column: u32) -> bool {
        match self
            .shape
            .index_of(plane, row, column)
            .and_then(|index| self.counts.get_mut(index))
        {
            Some(count) => {
                *count += 1;
                true
            }
            None => false,
        }
    }
}

/// Длина массива счётчиков не сошлась с формой.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("счётчиков {actual}, а форма требует {expected}")]
pub struct CountsLengthMismatch {
    /// Ожидаемая длина.
    pub expected: usize,
    /// Фактическая длина.
    pub actual: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_follows_documented_layout() {
        let shape = GridShape::try_new(4, 3, 2).unwrap();
        assert_eq!(shape.index_of(0, 0, 0), Some(0));
        assert_eq!(shape.index_of(1, 2, 3), Some((3 + 2) * 4 + 3));
        assert_eq!(shape.index_of(2, 0, 0), None);
        assert_eq!(shape.index_of(0, 3, 0), None);
        assert_eq!(shape.index_of(0, 0, 4), None);
        assert_eq!(shape.cell_count(), 24);
    }

    #[test]
    fn shape_validation_has_distinct_errors() {
        assert!(matches!(GridShape::try_new(0, 1, 1), Err(GridError::Empty { .. })));
        assert!(matches!(
            GridShape::try_new(MAX_GRID_SIDE + 1, 1, 1),
            Err(GridError::SideTooLarge { .. })
        ));
        assert!(matches!(
            GridShape::try_new(4096, 4096, 1),
            Err(GridError::TooManyCells { .. })
        ));
    }

    #[test]
    fn counters_count_and_total() {
        let shape = GridShape::try_new(2, 2, 1).unwrap();
        let mut counters = ScopeCounters::zeroed(shape);
        assert!(counters.add_hit(0, 1, 0));
        assert!(counters.add_hit(0, 1, 0));
        assert!(!counters.add_hit(0, 2, 0));
        assert_eq!(counters.count(0, 1, 0), Some(2));
        assert_eq!(counters.count(0, 0, 0), Some(0));
        assert_eq!(counters.count(0, 2, 0), None);
        assert_eq!(counters.total(), 2);
    }

    #[test]
    fn from_counts_checks_length() {
        let shape = GridShape::try_new(2, 2, 1).unwrap();
        assert!(ScopeCounters::from_counts(shape, vec![0; 3]).is_err());
        let counters = ScopeCounters::from_counts(shape, vec![1, 2, 3, 4]).unwrap();
        assert_eq!(counters.count(0, 1, 1), Some(4));
    }
}

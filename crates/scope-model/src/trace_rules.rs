//! RGB parade и RGB waveform: «столбец картинки × уровень × канал». Правило у них одно ([`ParadeRule`]): три плоскости
//! R', G', B'; отличаются они только способом показа (секции или наложение, см. [`crate::TracePaint`]).
//!
//! Столбец: `column = (x · columns) / width` — целочисленно в `u32`; переполнения нет, потому что
//! `x < width ≤ MAX_SHOWN_SIDE = 2^14` и `columns ≤ 4096 = 2^12` (`x · columns < 2^26`).
//!
//! Уровень: `bin = projection(code)` — целочисленная проекция, равная `floor(код · levels / 1024)`, где
//! `код` — код канала R', G' или B'. Веса суммируются ровно в
//! `levels · 2^16 / 1024`, поэтому белый `1023` попадает в `levels − 1`, чёрный `0` — в 0, а серый `c` —
//! в `floor(c · levels / 1024)` без ошибки округления. Строка 0 — ВЕРХ прибора: `row = levels − 1 − bin`
//! (пересвет — в верхней строке).
//!
//! Каналы меряются по КОДАМ сигнала (R', G', B'), и для Display P3 тоже: прибор меряет сигнал так, как
//! видеоинструменты (решение 183, выбор агента). Яркость Y' (BT.709) нужна только вектороскопу Y'CbCr, поэтому
//! веса [`BT709_LUMA_WEIGHTS`] живут здесь, а не в одном из приборов.
//!
//! `x` и `width` здесь — положение на ГОРИЗОНТАЛИ ГЛАЗА (`SamplePosition::from_shown` по `ScopeScan`).
//!
//! GLSL (на отсчёт): `uint column = (x * columns) / width; int bin = projection(code);
//! if (bin < 0 || bin >= levels) return; uint row = uint(levels - 1 - bin);`.

use scope_contract::ScopeScan;

use crate::grid::{GridError, GridShape};
use crate::projection::{IntegerProjection, PROJECTION_SHIFT, ProjectionError, WeightSum};
use crate::rule::{Hit, SamplePosition, ScopeRule};

/// Коэффициенты яркости BT.709 (на КОДАХ сигнала, не на линейном свете).
pub const BT709_LUMA_WEIGHTS: [f64; 3] = [0.2126, 0.7152, 0.0722];

/// Наибольшее число уровней: больше, чем кодов (1024), нет смысла.
pub const MAX_LEVELS: u32 = 1024;

/// Ошибка построения правил waveform и parade.
#[derive(Debug, Clone, Copy, PartialEq, thiserror::Error)]
pub enum TraceRuleError {
    /// Число уровней вне `1..=MAX_LEVELS`.
    #[error("число уровней {levels} вне 1..={MAX_LEVELS}")]
    LevelsOutOfRange {
        /// Недопустимое число.
        levels: u32,
    },
    /// Форма сетки недопустима.
    #[error(transparent)]
    Grid(#[from] GridError),
    /// Проекцию не удалось построить.
    #[error(transparent)]
    Projection(#[from] ProjectionError),
}

/// Общая ось «столбец × уровень» для waveform и parade.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LevelAxis {
    columns: u32,
    levels: u32,
    /// Проверенная форма сетки (колонки × уровни × плоскости).
    shape: GridShape,
    /// Как глаз читает картинку слева направо: откуда берётся положение отсчёта.
    scan: ScopeScan,
}

impl LevelAxis {
    fn new(columns: u32, levels: u32, planes: u32) -> Result<Self, TraceRuleError> {
        if !(1..=MAX_LEVELS).contains(&levels) {
            return Err(TraceRuleError::LevelsOutOfRange { levels });
        }
        // Проверка формы (столбцы и предел ячеек) — той же функцией, что и сетка.
        let shape = GridShape::try_new(columns, levels, planes)?;
        Ok(Self {
            columns,
            levels,
            shape,
            scan: ScopeScan::Upright,
        })
    }

    /// Столбец сетки для столбца картинки.
    fn column_of(&self, position: SamplePosition) -> u32 {
        (position.x * self.columns) / position.picture_width
    }

    /// Строка сетки для уровня: `None`, если уровень вне `0..levels` (нет попадания).
    fn row_of(&self, level: i32) -> Option<u32> {
        let level = u32::try_from(level).ok()?;
        if level >= self.levels {
            None
        } else {
            Some(self.levels - 1 - level)
        }
    }

    /// Вес «код → уровень»: `levels / 1024` на единицу кода.
    fn level_per_code(&self) -> f64 {
        f64::from(self.levels) / 1024.0
    }
}

/// RGB parade: те же столбец и уровень для каждого канала, три плоскости (R', G', B').
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParadeRule {
    axis: LevelAxis,
    projections: [IntegerProjection; 3],
}

impl ParadeRule {
    /// Правило с `columns` столбцами и `levels` уровнями на канал.
    pub fn new(columns: u32, levels: u32) -> Result<Self, TraceRuleError> {
        let axis = LevelAxis::new(columns, levels, 3)?;
        let per_code = axis.level_per_code();
        let projection_of_channel = |channel: usize| {
            let mut coefficients = [0.0; 3];
            coefficients[channel] = per_code;
            IntegerProjection::from_real(coefficients, 0.0, PROJECTION_SHIFT, WeightSum::Unconstrained)
        };
        Ok(Self {
            axis,
            projections: [
                projection_of_channel(0)?,
                projection_of_channel(1)?,
                projection_of_channel(2)?,
            ],
        })
    }

    /// Проекции «код → уровень» по каналам R, G, B (целые веса для GPU).
    pub fn level_projections(&self) -> [IntegerProjection; 3] {
        self.projections
    }

    /// Читает картинку так, как её видит глаз при положении телефона `scan` (по умолчанию — портрет).
    #[must_use]
    pub fn with_scan(mut self, scan: ScopeScan) -> Self {
        self.axis.scan = scan;
        self
    }

    /// Число столбцов сетки.
    pub fn columns(&self) -> u32 {
        self.axis.columns
    }

    /// Число уровней.
    pub fn levels(&self) -> u32 {
        self.axis.levels
    }
}

impl ScopeRule for ParadeRule {
    fn shape(&self) -> GridShape {
        self.axis.shape
    }

    fn scan(&self) -> ScopeScan {
        self.axis.scan
    }

    fn hits(&self, position: SamplePosition, code: [u16; 3], report: &mut dyn FnMut(Hit)) {
        let column = self.axis.column_of(position);
        for (plane, projection) in (0u32..).zip(&self.projections) {
            if let Some(row) = self.axis.row_of(projection.apply(code)) {
                report(Hit { plane, row, column });
            }
        }
    }
}

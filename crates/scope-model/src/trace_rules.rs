//! RGB parade и RGB waveform: «столбец картинки × уровень × канал». Правило у них одно ([`ParadeRule`]): три плоскости
//! R', G', B'; отличаются они только способом показа (секции или наложение, см. [`crate::TracePaint`]). Яркостный
//! waveform ([`LumaRule`], решение 321) — та же ось «столбец × уровень», но одна плоскость: яркость Y' по кодам.
//!
//! Столбец: `column = номер отсчёта вдоль взгляда` ([`SamplePosition::eye_index`], решение 322). Столбцов у сетки
//! ровно столько, сколько отсчётов вдоль взгляда в ЭТОМ кадре ([`ParadeRule::shape`]), поэтому каждый столбец
//! получает одно и то же число отсчётов — поперёк взгляда. Пустых столбцов (тёмные полосы 14o: окно раздела 917 px
//! при шаге 2 давало 458 отсчётов на 512 столбцов) и двойных (полосы 14m) нет по построению. Потолок столбцов
//! (`max_columns`) — ёмкость буфера счётчиков; план отсчётов ([`crate::SampleCount::planned`]) держит кадр под ним.
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
//! Номер отсчёта считается на ГОРИЗОНТАЛИ ГЛАЗА (`SamplePosition::of_sample` по `ScopeScan`).
//!
//! GLSL (на отсчёт): `uint column = eye_index; int bin = projection(code);
//! if (bin < 0 || bin >= levels) return; uint row = uint(levels - 1 - bin);`.

use scope_contract::ScopeScan;

use crate::grid::{GridError, GridShape};
use crate::projection::{IntegerProjection, PROJECTION_SHIFT, ProjectionError, WeightSum};
use crate::rule::{Hit, SamplePosition, ScopeRule};
use crate::sample_count::SampleCount;

/// Коэффициенты яркости BT.709 (на КОДАХ сигнала, не на линейном свете): ими меряет и яркостный waveform, и вектороскоп
/// Y'CbCr.
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
    /// Наибольшая сетка: потолок столбцов (ёмкость буфера счётчиков) × уровни × плоскости. Столбцов в кадре —
    /// по отсчётам, но не больше потолка.
    largest: GridShape,
    /// Как глаз читает картинку слева направо: откуда берётся положение отсчёта.
    scan: ScopeScan,
}

impl LevelAxis {
    fn new(max_columns: u32, levels: u32, planes: u32) -> Result<Self, TraceRuleError> {
        if !(1..=MAX_LEVELS).contains(&levels) {
            return Err(TraceRuleError::LevelsOutOfRange { levels });
        }
        // Наибольшая сетка (потолок столбцов) проверяется той же функцией, что и сетка: любая сетка кадра не больше.
        let largest = GridShape::try_new(max_columns, levels, planes)?;
        Ok(Self {
            largest,
            scan: ScopeScan::Upright,
        })
    }

    /// Форма сетки кадра: столбцов — отсчётов вдоль взгляда, но не меньше одного и не больше потолка.
    fn shape_for(&self, samples: SampleCount) -> GridShape {
        self.largest.narrowed_to_columns(samples.along_eye(self.scan))
    }

    /// Строка сетки для уровня: `None`, если уровень вне `0..levels` (нет попадания).
    fn row_of(&self, level: i32) -> Option<u32> {
        let level = u32::try_from(level).ok()?;
        let levels = self.largest.rows();
        if level >= levels {
            None
        } else {
            Some(levels - 1 - level)
        }
    }

    /// Вес «код → уровень»: `levels / 1024` на единицу кода.
    fn level_per_code(&self) -> f64 {
        f64::from(self.largest.rows()) / 1024.0
    }
}

/// RGB parade: те же столбец и уровень для каждого канала, три плоскости (R', G', B').
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParadeRule {
    axis: LevelAxis,
    projections: [IntegerProjection; 3],
}

impl ParadeRule {
    /// Правило с потолком `max_columns` столбцов (столбцов в кадре — по отсчётам вдоль взгляда) и `levels`
    /// уровнями на канал.
    pub fn new(max_columns: u32, levels: u32) -> Result<Self, TraceRuleError> {
        let axis = LevelAxis::new(max_columns, levels, 3)?;
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

    /// Потолок столбцов сетки (ёмкость буфера счётчиков).
    pub fn max_columns(&self) -> u32 {
        self.axis.largest.columns()
    }

    /// Число уровней.
    pub fn levels(&self) -> u32 {
        self.axis.largest.rows()
    }
}

impl ScopeRule for ParadeRule {
    fn shape(&self, samples: SampleCount) -> GridShape {
        self.axis.shape_for(samples)
    }

    fn scan(&self) -> ScopeScan {
        self.axis.scan
    }

    fn hits(&self, position: SamplePosition, code: [u16; 3], report: &mut dyn FnMut(Hit)) {
        // Столбец — номер отсчёта вдоль взгляда (решение 322).
        let column = position.eye_index;
        for (plane, projection) in (0u32..).zip(&self.projections) {
            if let Some(row) = self.axis.row_of(projection.apply(code)) {
                report(Hit { plane, row, column });
            }
        }
    }
}

/// Яркостный waveform (Luma, решение 321): столбец — номер отсчёта вдоль взгляда (как у [`ParadeRule`]), уровень —
/// яркость `Y' = 0,2126 R' + 0,7152 G' + 0,0722 B'` по кодам сигнала, одна плоскость. Сумма весов проекции ровно
/// `levels / 1024`: серый код `c` попадает в ту же строку, что у RGB waveform, — линии уровней и подписи общие.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LumaRule {
    axis: LevelAxis,
    projection: IntegerProjection,
}

impl LumaRule {
    /// Правило с потолком `max_columns` столбцов и `levels` уровнями.
    pub fn new(max_columns: u32, levels: u32) -> Result<Self, TraceRuleError> {
        let axis = LevelAxis::new(max_columns, levels, 1)?;
        let per_code = axis.level_per_code();
        let projection = IntegerProjection::from_real(
            BT709_LUMA_WEIGHTS.map(|weight| weight * per_code),
            0.0,
            PROJECTION_SHIFT,
            WeightSum::Exactly(per_code),
        )?;
        Ok(Self { axis, projection })
    }

    /// Проекция «коды → уровень яркости» (целые веса для GPU).
    pub fn level_projection(&self) -> IntegerProjection {
        self.projection
    }

    /// Читает картинку так, как её видит глаз при положении телефона `scan`.
    #[must_use]
    pub fn with_scan(mut self, scan: ScopeScan) -> Self {
        self.axis.scan = scan;
        self
    }

    /// Число уровней.
    pub fn levels(&self) -> u32 {
        self.axis.largest.rows()
    }
}

impl ScopeRule for LumaRule {
    fn shape(&self, samples: SampleCount) -> GridShape {
        self.axis.shape_for(samples)
    }

    fn scan(&self) -> ScopeScan {
        self.axis.scan
    }

    fn hits(&self, position: SamplePosition, code: [u16; 3], report: &mut dyn FnMut(Hit)) {
        if let Some(row) = self.axis.row_of(self.projection.apply(code)) {
            report(Hit {
                plane: 0,
                row,
                column: position.eye_index,
            });
        }
    }
}

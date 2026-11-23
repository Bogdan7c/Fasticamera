//! Вектороскоп по цветоразностным осям Y'CbCr BT.709 (стандарт видеомонтажа).
//!
//! `R'G'B' = код / 1023`; `Cb = (B' − Y') / 1,8556`, `Cr = (R' − Y') / 1,5748`, где
//! `Y' = 0,2126 R' + 0,7152 G' + 0,0722 B'`, а `1,8556 = 2 (1 − Kb)`, `1,5748 = 2 (1 − Kr)`.
//! Cb идёт вправо (+столбец), Cr — вверх (−строка). Обе координаты — целочисленные проекции
//! (см. [`crate::projection`]): `column = (Σ wᵢ·cᵢ + offset) >> 16`, веса — округлённые f64-коэффициенты
//! формулы, умноженные на «клеток на единицу Cb/Cr» `s = 2 · half_extent · gain`, и делённые на 1023.
//! Веса цветоразности суммируются ровно в 0, поэтому серый — ровно центр, без допуска.
//!
//! GLSL: `int column = proj_column(code); int row = proj_row(code);
//! if (column < 0 || column >= size || row < 0 || row >= size) return;`.

use crate::grid::GridShape;
use crate::projection::{IntegerProjection, PROJECTION_SHIFT, ProjectionError, WeightSum};
use crate::rule::{Hit, SamplePosition, ScopeRule};
use crate::trace_rules::BT709_LUMA_WEIGHTS;
use crate::vectorscope_geometry::{VectorscopeGeometry, VectorscopeGeometryError};

/// Наибольшее усиление вектороскопа Y'CbCr (иначе проекция переполнит `i32`).
pub const MAX_YCBCR_GAIN: f32 = 8.0;

/// Ошибка построения вектороскопа Y'CbCr.
#[derive(Debug, Clone, Copy, PartialEq, thiserror::Error)]
pub enum YCbCrVectorscopeError {
    /// Усиление вне `1..=MAX_YCBCR_GAIN` или не число.
    #[error("усиление вектороскопа {gain} вне 1..={MAX_YCBCR_GAIN}")]
    GainOutOfRange {
        /// Недопустимое усиление.
        gain: f32,
    },
    /// Геометрия сетки недопустима.
    #[error(transparent)]
    Geometry(#[from] VectorscopeGeometryError),
    /// Проекцию не удалось построить.
    #[error(transparent)]
    Projection(#[from] ProjectionError),
}

/// Вектороскоп Y'CbCr BT.709.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct YCbCrVectorscopeRule {
    geometry: VectorscopeGeometry,
    gain: f32,
    column_projection: IntegerProjection,
    row_projection: IntegerProjection,
}

impl YCbCrVectorscopeRule {
    /// Правило с сеткой `size × size` и усилением `gain ≥ 1`.
    pub fn new(size: u32, gain: f32) -> Result<Self, YCbCrVectorscopeError> {
        if !(1.0..=MAX_YCBCR_GAIN).contains(&gain) {
            return Err(YCbCrVectorscopeError::GainOutOfRange { gain });
        }
        let geometry = VectorscopeGeometry::try_new(size)?;
        let [kr, kg, kb] = BT709_LUMA_WEIGHTS;
        // Клеток на единицу Cb/Cr (единица Cb = полный размах 1,0 = ±0,5), и перевод кода в 0…1.
        let cells_per_unit = 2.0 * geometry.half_extent_cells() * f64::from(gain) / 1023.0;
        let cb_divisor = 2.0 * (1.0 - kb);
        let cr_divisor = 2.0 * (1.0 - kr);
        let column_coefficients =
            [-kr / cb_divisor, -kg / cb_divisor, (1.0 - kb) / cb_divisor].map(|c| c * cells_per_unit);
        // Строка растёт вниз, а Cr — вверх, поэтому коэффициенты Cr берутся с минусом.
        let row_coefficients =
            [(1.0 - kr) / cr_divisor, -kg / cr_divisor, -kb / cr_divisor].map(|c| -c * cells_per_unit);
        let offset = geometry.centre_offset_cells();
        Ok(Self {
            geometry,
            gain,
            column_projection: IntegerProjection::from_real(
                column_coefficients,
                offset,
                PROJECTION_SHIFT,
                WeightSum::Exactly(0.0),
            )?,
            row_projection: IntegerProjection::from_real(
                row_coefficients,
                offset,
                PROJECTION_SHIFT,
                WeightSum::Exactly(0.0),
            )?,
        })
    }

    /// Геометрия сетки.
    pub fn geometry(&self) -> VectorscopeGeometry {
        self.geometry
    }

    /// Усиление.
    pub fn gain(&self) -> f32 {
        self.gain
    }

    /// Проекция «код → столбец» (целые веса для GPU).
    pub fn column_projection(&self) -> IntegerProjection {
        self.column_projection
    }

    /// Проекция «код → строка» (целые веса для GPU).
    pub fn row_projection(&self) -> IntegerProjection {
        self.row_projection
    }
}

impl ScopeRule for YCbCrVectorscopeRule {
    fn shape(&self) -> GridShape {
        self.geometry.shape()
    }

    fn hits(&self, _position: SamplePosition, code: [u16; 3], report: &mut dyn FnMut(Hit)) {
        let column = self.column_projection.apply(code);
        let row = self.row_projection.apply(code);
        let size = i64::from(self.geometry.size());
        if (0..size).contains(&i64::from(column)) && (0..size).contains(&i64::from(row)) {
            report(Hit {
                plane: 0,
                // Диапазон только что проверен, приведение не теряет знак.
                row: row as u32,
                column: column as u32,
            });
        }
    }
}

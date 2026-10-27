//! Матрица 3×3 и вектор из трёх чисел для цветовой математики.
//!
//! Подготовка матриц на кадр (интерполяция DNG, обращение, адаптация) идёт в f64: это
//! считается один раз на кадр, а точность нужна, чтобы эталон не накапливал ошибку.
//! Попиксельное применение — в f32 через [`Matrix3::to_f32`], как будет на GPU.

use camera_contract::capabilities::{Matrix3x3, Rational};

/// Матрица вырождена (определитель почти 0): цветовое преобразование не обратить.
#[derive(Debug, Clone, Copy, PartialEq, thiserror::Error)]
#[error("матрица 3×3 вырождена (определитель {determinant:e}), обратить нельзя")]
pub struct SingularMatrix {
    pub determinant: f64,
}

/// В метаданных HAL у рационального числа нулевой знаменатель.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("элемент {element} матрицы из метаданных имеет нулевой знаменатель")]
pub struct BrokenRational {
    pub element: usize,
}

/// Порог вырожденности. Цветовые матрицы камер имеют определитель порядка 0,1–10,
/// поэтому 1e-12 отличает только действительно сломанные данные.
const SINGULAR_DETERMINANT_THRESHOLD: f64 = 1e-12;

/// Вектор-столбец из трёх компонент (XYZ, RGB, нейтраль камеры).
pub type Vector3 = [f64; 3];

/// Матрица 3×3 в порядке строк: `rows[i][j]` — строка i, столбец j.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Matrix3 {
    pub rows: [[f64; 3]; 3],
}

impl Matrix3 {
    pub const IDENTITY: Self = Self {
        rows: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
    };

    pub const fn from_rows(rows: [[f64; 3]; 3]) -> Self {
        Self { rows }
    }

    /// Диагональная матрица (масштаб каналов: баланс белого, AnalogBalance).
    pub const fn diagonal(values: Vector3) -> Self {
        Self {
            rows: [[values[0], 0.0, 0.0], [0.0, values[1], 0.0], [0.0, 0.0, values[2]]],
        }
    }

    /// Матрица из метаданных Camera2/DNG (рациональные числа в порядке строк).
    pub fn from_rationals(matrix: &Matrix3x3) -> Result<Self, BrokenRational> {
        let mut rows = [[0.0; 3]; 3];
        for (element, rational) in matrix.0.iter().enumerate() {
            rows[element / 3][element % 3] = rational_value(*rational, element)?;
        }
        Ok(Self { rows })
    }

    /// Произведение `self · other` (сначала применяется `other`, потом `self`).
    pub fn mul(&self, other: &Matrix3) -> Matrix3 {
        let mut rows = [[0.0; 3]; 3];
        for (row, output_row) in rows.iter_mut().enumerate() {
            for (column, output) in output_row.iter_mut().enumerate() {
                *output = (0..3).map(|k| self.rows[row][k] * other.rows[k][column]).sum();
            }
        }
        Matrix3 { rows }
    }

    /// Применение к вектору: `self · vector`.
    pub fn apply(&self, vector: Vector3) -> Vector3 {
        let mut output = [0.0; 3];
        for (row, value) in output.iter_mut().enumerate() {
            *value = (0..3).map(|k| self.rows[row][k] * vector[k]).sum();
        }
        output
    }

    /// Линейная смесь `weight · self + (1 − weight) · other` (интерполяция DNG по температуре).
    pub fn blend(&self, other: &Matrix3, weight: f64) -> Matrix3 {
        let mut rows = [[0.0; 3]; 3];
        for (row, output_row) in rows.iter_mut().enumerate() {
            for (column, output) in output_row.iter_mut().enumerate() {
                *output = weight * self.rows[row][column] + (1.0 - weight) * other.rows[row][column];
            }
        }
        Matrix3 { rows }
    }

    pub fn determinant(&self) -> f64 {
        let m = &self.rows;
        m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1]) - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
            + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0])
    }

    /// Обратная матрица через присоединённую (adjugate / determinant).
    pub fn inverse(&self) -> Result<Matrix3, SingularMatrix> {
        let determinant = self.determinant();
        if determinant.abs() < SINGULAR_DETERMINANT_THRESHOLD || !determinant.is_finite() {
            return Err(SingularMatrix { determinant });
        }
        let m = &self.rows;
        // Алгебраические дополнения, уже транспонированные (присоединённая матрица).
        let adjugate = [
            [
                m[1][1] * m[2][2] - m[1][2] * m[2][1],
                m[0][2] * m[2][1] - m[0][1] * m[2][2],
                m[0][1] * m[1][2] - m[0][2] * m[1][1],
            ],
            [
                m[1][2] * m[2][0] - m[1][0] * m[2][2],
                m[0][0] * m[2][2] - m[0][2] * m[2][0],
                m[0][2] * m[1][0] - m[0][0] * m[1][2],
            ],
            [
                m[1][0] * m[2][1] - m[1][1] * m[2][0],
                m[0][1] * m[2][0] - m[0][0] * m[2][1],
                m[0][0] * m[1][1] - m[0][1] * m[1][0],
            ],
        ];
        let mut rows = [[0.0; 3]; 3];
        for (row, output_row) in rows.iter_mut().enumerate() {
            for (column, output) in output_row.iter_mut().enumerate() {
                *output = adjugate[row][column] / determinant;
            }
        }
        Ok(Matrix3 { rows })
    }

    /// Та же матрица в f32 для попиксельной работы (и для GPU).
    pub fn to_f32(&self) -> PixelMatrix {
        // Сужение f64 → f32 осознанное: пиксели считаются в f32, как в шейдере.
        let narrow = |row: [f64; 3]| row.map(|value| value as f32);
        PixelMatrix {
            rows: self.rows.map(narrow),
        }
    }
}

/// Матрица 3×3 в f32 для применения к каждому пикселю.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PixelMatrix {
    pub rows: [[f32; 3]; 3],
}

impl PixelMatrix {
    /// `self · pixel` в f32.
    pub fn apply(&self, pixel: [f32; 3]) -> [f32; 3] {
        let row = |r: [f32; 3]| r[0] * pixel[0] + r[1] * pixel[1] + r[2] * pixel[2];
        [row(self.rows[0]), row(self.rows[1]), row(self.rows[2])]
    }
}

fn rational_value(rational: Rational, element: usize) -> Result<f64, BrokenRational> {
    rational.to_f64().ok_or(BrokenRational { element })
}

#[cfg(test)]
#[path = "matrix_tests.rs"]
mod tests;

//! Вектороскоп по осям a/b пространства OKLab (те же оси, что у ручек цвета вида, 14a).
//!
//! Цепочка на отсчёт (каждая арифметическая операция — f32 add/sub/mul, `precise` на GPU; порядок фиксирован):
//! 1. код → свет экрана: `linear = table[code]`, [`DisplayDecodeTable`] (f64 один раз → f32; на GPU таблица
//!    загружается как есть, а не пересчитывается). Для Display P3 передача та же sRGB (как `isp-model::transfer`).
//! 2. `l = (m00·r + m01·g) + m02·b`, так же `m` и `s` по строкам матрицы «свет экрана → LMS»
//!    ([`isp_model::color::oklab_working::OklabWorkspace::working_to_lms`] для основных цветов экрана).
//! 3. `l' = cube_root(l)` и т.д. ([`crate::cube_root`]).
//! 4. `a = (a0·l' + a1·m') + a2·s'`, `b = (b0·l' + b1·m') + b2·s'` — строки 1 и 2 матрицы `M2`.
//! 5. `column_t = a · scale + centre_offset`, `row_t = centre_offset − b · scale`; `scale = half_extent / range`
//!    (клеток на единицу OKLab-цветности), `centre_offset = size/2 + 0,5` (центральная ячейка покрывает
//!    полклетки вокруг нуля — серый, у которого `a`, `b` ≈ 10⁻⁷ из-за округления f32, попадает ровно в центр).
//! 6. Если `column_t` и `row_t` оба в `[0, size)` (сравнение в f32, NaN → нет попадания) — `floor` и в ячейку.
//!
//! GLSL: `precise` на всех промежуточных; `if (!(ct >= 0.0 && ct < size_f && rt >= 0.0 && rt < size_f)) return;
//! uint column = uint(floor(ct)); uint row = uint(floor(rt));`.

use isp_model::color::oklab_working::OklabWorkspace;
use isp_model::image::DisplayGamut;

use crate::codes::{DISPLAY_CODE_COUNT, MAX_DISPLAY_CODE};
use crate::cube_root::cube_root;
use crate::grid::GridShape;
use crate::rule::{Hit, SamplePosition, ScopeRule};
use crate::vectorscope_geometry::{VectorscopeGeometry, VectorscopeGeometryError};

/// Наименьший размах OKLab-цветности на край сетки.
pub const MIN_OKLAB_RANGE: f32 = 0.05;

/// Наибольший размах OKLab-цветности на край сетки (цветность видимых цветов < 0,4).
pub const MAX_OKLAB_RANGE: f32 = 2.0;

/// Ошибка построения вектороскопа OKLab.
#[derive(Debug, Clone, Copy, PartialEq, thiserror::Error)]
pub enum OklabVectorscopeError {
    /// Размах вне `MIN..=MAX` или не число.
    #[error("размах OKLab-цветности {range} вне {MIN_OKLAB_RANGE}..={MAX_OKLAB_RANGE}")]
    RangeOutOfRange {
        /// Недопустимый размах.
        range: f32,
    },
    /// Геометрия сетки недопустима.
    #[error(transparent)]
    Geometry(#[from] VectorscopeGeometryError),
    /// Основные цвета экрана дали вырожденную матрицу.
    #[error("основные цвета экрана дают вырожденную матрицу OKLab")]
    SingularPrimaries,
}

/// Таблица «код 0…1023 → свет экрана» (передача sRGB). Её содержимое — часть решённого правила:
/// GPU получает те же числа.
#[derive(Debug, Clone, PartialEq)]
pub struct DisplayDecodeTable {
    values: Box<[f32; DISPLAY_CODE_COUNT]>,
}

impl DisplayDecodeTable {
    /// Таблица sRGB EOTF: считается в f64 и один раз округляется до f32.
    pub fn srgb() -> Self {
        let mut values = Box::new([0.0_f32; DISPLAY_CODE_COUNT]);
        for (code, value) in values.iter_mut().enumerate() {
            let encoded = code as f64 / f64::from(MAX_DISPLAY_CODE);
            let linear = if encoded <= 0.04045 {
                encoded / 12.92
            } else {
                ((encoded + 0.055) / 1.055).powf(2.4)
            };
            *value = linear as f32;
        }
        Self { values }
    }

    /// Свет экрана для кода (код ограничен 1023).
    pub fn linear_light(&self, code: u16) -> f32 {
        self.values[usize::from(code.min(MAX_DISPLAY_CODE))]
    }

    /// Все значения (для загрузки на GPU).
    pub fn values(&self) -> &[f32; DISPLAY_CODE_COUNT] {
        &self.values
    }
}

/// Вектороскоп OKLab a/b для одних основных цветов экрана.
#[derive(Debug, Clone, PartialEq)]
pub struct OklabVectorscopeRule {
    geometry: VectorscopeGeometry,
    gamut: DisplayGamut,
    range: f32,
    decode_table: DisplayDecodeTable,
    working_to_lms: [[f32; 3]; 3],
    lab_a_row: [f32; 3],
    lab_b_row: [f32; 3],
    scale: f32,
    centre_offset: f32,
    size_limit: f32,
}

impl OklabVectorscopeRule {
    /// Правило для сетки `size × size`, размаха `range` и основных цветов экрана `gamut`.
    pub fn new(size: u32, range: f32, gamut: DisplayGamut) -> Result<Self, OklabVectorscopeError> {
        if !(MIN_OKLAB_RANGE..=MAX_OKLAB_RANGE).contains(&range) {
            return Err(OklabVectorscopeError::RangeOutOfRange { range });
        }
        let geometry = VectorscopeGeometry::try_new(size)?;
        let workspace =
            OklabWorkspace::new(&gamut.primaries()).map_err(|_| OklabVectorscopeError::SingularPrimaries)?;
        Ok(Self {
            geometry,
            gamut,
            range,
            decode_table: DisplayDecodeTable::srgb(),
            working_to_lms: workspace.working_to_lms,
            lab_a_row: workspace.lms_prime_to_lab[1],
            lab_b_row: workspace.lms_prime_to_lab[2],
            scale: (geometry.half_extent_cells() / f64::from(range)) as f32,
            centre_offset: geometry.centre_offset_cells() as f32,
            size_limit: size as f32,
        })
    }

    /// Геометрия сетки.
    pub fn geometry(&self) -> VectorscopeGeometry {
        self.geometry
    }

    /// Основные цвета экрана.
    pub fn gamut(&self) -> DisplayGamut {
        self.gamut
    }

    /// Размах OKLab-цветности на край сетки.
    pub fn range(&self) -> f32 {
        self.range
    }

    /// Таблица декодирования (для GPU).
    pub fn decode_table(&self) -> &DisplayDecodeTable {
        &self.decode_table
    }

    /// Матрица «свет экрана → LMS» (для GPU).
    pub fn working_to_lms(&self) -> [[f32; 3]; 3] {
        self.working_to_lms
    }

    /// Строки `a` и `b` матрицы `M2` (для GPU).
    pub fn lab_rows(&self) -> [[f32; 3]; 2] {
        [self.lab_a_row, self.lab_b_row]
    }

    /// Клеток на единицу OKLab-цветности (для GPU).
    pub fn scale(&self) -> f32 {
        self.scale
    }

    /// Смещение центра `size/2 + 0,5` (для GPU).
    pub fn centre_offset(&self) -> f32 {
        self.centre_offset
    }

    /// OKLab `(a, b)` кода по цепочке правила (шаги 1–4).
    pub fn lab_ab(&self, code: [u16; 3]) -> [f32; 2] {
        let linear = code.map(|channel| self.decode_table.linear_light(channel));
        let lms_prime = self
            .working_to_lms
            .map(|row| cube_root(dot_in_fixed_order(row, linear)));
        [
            dot_in_fixed_order(self.lab_a_row, lms_prime),
            dot_in_fixed_order(self.lab_b_row, lms_prime),
        ]
    }

    /// Положение кода в клетках ДО округления: `[column_t, row_t]` (шаг 5).
    pub fn grid_position(&self, code: [u16; 3]) -> [f32; 2] {
        let [a, b] = self.lab_ab(code);
        [a * self.scale + self.centre_offset, self.centre_offset - b * self.scale]
    }
}

/// `(r0·v0 + r1·v1) + r2·v2` — единственный порядок суммирования, одинаковый на GPU.
fn dot_in_fixed_order(row: [f32; 3], vector: [f32; 3]) -> f32 {
    (row[0] * vector[0] + row[1] * vector[1]) + row[2] * vector[2]
}

impl ScopeRule for OklabVectorscopeRule {
    fn shape(&self) -> GridShape {
        self.geometry.shape()
    }

    fn hits(&self, _position: SamplePosition, code: [u16; 3], report: &mut dyn FnMut(Hit)) {
        let [column_t, row_t] = self.grid_position(code);
        let inside = |value: f32| value >= 0.0 && value < self.size_limit;
        if inside(column_t) && inside(row_t) {
            report(Hit {
                plane: 0,
                // Значение в [0, size): floor и приведение безопасны.
                row: row_t.floor() as u32,
                column: column_t.floor() as u32,
            });
        }
    }
}

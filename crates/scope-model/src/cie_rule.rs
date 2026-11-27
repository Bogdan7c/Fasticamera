//! Диаграмма цветности CIE: правило попадания (эталон шейдера `scope_accumulate_cie_rule.glsl`).
//!
//! # Цепочка на отсчёт (f32 `+ − ×` в фиксированном порядке, `precise` на GPU; деления нет — см. [`crate::cie_cell`])
//! 1. код 0…1023 → свет экрана по таблице sRGB ([`DisplayDecodeTable`]; у Display P3 передача та же);
//! 2. свет → четыре числа ОДНОЙ матрицей 4×3 (каждая строка — `dot_in_fixed_order`): числитель столбца `Nc`,
//!    числитель строки `Nr`, знаменатель `S` и яркость `Y`. Шкала решена заранее в f64 и один раз округлена до f32:
//!    xy — `Nc = X`, `Nr = Y`, `S = X+Y+Z`; u′v′ — `Nc = 4X`, `Nr = 9Y`, `S = X+15Y+3Z` (`X, Y, Z` — через основные
//!    цвета ИЗМЕРЯЕМОГО охвата). Шкала — данные матрицы, а не ветка шейдера;
//! 3. если `S ≤ 0` или `Y < min_luminance` — нет попадания (почти чёрное: цветность — шум квантования);
//! 4. `t_столбца = (Nc − левая·S)·k`, `t_строки = (верх·S − Nr)·k`, `k = size / сторона окна`;
//! 5. ячейка по оси — точное деление ([`crate::cie_cell::cell_on_axis`]); вне сетки — нет попадания.
//!
//! # Охват измерения
//! Охват, в котором закодированы показанные коды (сегодня — охват экрана, который даёт `ResolvedScope::resolve`), —
//! свойство правила. Список опорных контуров — настройки показа и с правилом не связан (решение 333).

use isp_model::color::matrix::Matrix3;
use isp_model::image::DisplayGamut;
use scope_contract::CieDiagram;

use crate::cie_cell::cell_on_axis;
use crate::cie_chart::ChartWindow;
use crate::cie_settings::{CieSettings, CieSettingsError};
use crate::grid::GridShape;
use crate::oklab_rule::{DisplayDecodeTable, dot_in_fixed_order};
use crate::rule::{Hit, SamplePosition, ScopeRule};
use crate::sample_count::SampleCount;
use crate::vectorscope_geometry::{VectorscopeGeometry, VectorscopeGeometryError};

/// Ошибка построения правила CIE.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum CieRuleError {
    /// Настройки недопустимы.
    #[error(transparent)]
    Settings(#[from] CieSettingsError),
    /// Геометрия сетки недопустима.
    #[error(transparent)]
    Geometry(#[from] VectorscopeGeometryError),
    /// Основные цвета измеряемого охвата дали вырожденную матрицу.
    #[error("основные цвета измеряемого охвата дают вырожденную матрицу XYZ")]
    SingularPrimaries,
}

/// Четыре строки «свет экрана → (Nc, Nr, S, Y)» в порядке, в котором их читает шейдер.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CieRows {
    /// Числитель столбца.
    pub column_numerator: [f32; 3],
    /// Числитель строки.
    pub row_numerator: [f32; 3],
    /// Знаменатель.
    pub denominator: [f32; 3],
    /// Линейная яркость Y.
    pub luminance: [f32; 3],
}

/// Диаграмма CIE для одной шкалы и одного измеряемого охвата.
#[derive(Debug, Clone, PartialEq)]
pub struct CieRule {
    geometry: VectorscopeGeometry,
    diagram: CieDiagram,
    measured_gamut: DisplayGamut,
    window: ChartWindow,
    decode_table: DisplayDecodeTable,
    rows: CieRows,
    xyz_to_display: [[f32; 3]; 3],
    min_luminance: f32,
    cells_per_unit: f32,
    window_top: f32,
    size_limit: f32,
}

impl CieRule {
    /// Правило шкалы `diagram` для охвата измерения `measured_gamut` и настроек `settings`.
    pub fn new(
        diagram: CieDiagram,
        measured_gamut: DisplayGamut,
        settings: &CieSettings,
    ) -> Result<Self, CieRuleError> {
        settings.validate()?;
        let geometry = VectorscopeGeometry::try_new(settings.size)?;
        let rgb_to_xyz = measured_gamut
            .primaries()
            .rgb_to_xyz()
            .map_err(|_| CieRuleError::SingularPrimaries)?;
        let xyz_to_rgb = rgb_to_xyz.inverse().map_err(|_| CieRuleError::SingularPrimaries)?;
        let window = match diagram {
            CieDiagram::Xy1931 => settings.xy_window,
            CieDiagram::UvPrime1976 => settings.uv_window,
        };
        Ok(Self {
            geometry,
            diagram,
            measured_gamut,
            window,
            decode_table: DisplayDecodeTable::srgb(),
            rows: rows_for(diagram, &rgb_to_xyz),
            xyz_to_display: matrix_to_f32(&xyz_to_rgb),
            min_luminance: settings.min_luminance,
            cells_per_unit: (f64::from(settings.size) / f64::from(window.extent)) as f32,
            window_top: window.top(),
            size_limit: settings.size as f32,
        })
    }

    /// Шкала.
    pub fn diagram(&self) -> CieDiagram {
        self.diagram
    }

    /// Охват, в котором закодирована измеряемая картинка.
    pub fn measured_gamut(&self) -> DisplayGamut {
        self.measured_gamut
    }

    /// Окно диаграммы.
    pub fn window(&self) -> ChartWindow {
        self.window
    }

    /// Геометрия сетки.
    pub fn geometry(&self) -> VectorscopeGeometry {
        self.geometry
    }

    /// Строки «свет → (Nc, Nr, S, Y)» (для GPU).
    pub fn rows(&self) -> CieRows {
        self.rows
    }

    /// Матрица «XYZ → линейный свет измеряемого охвата» (цвета по месту).
    pub fn xyz_to_display(&self) -> [[f32; 3]; 3] {
        self.xyz_to_display
    }

    /// Таблица декодирования (для GPU).
    pub fn decode_table(&self) -> &DisplayDecodeTable {
        &self.decode_table
    }

    /// Порог яркости (для GPU).
    pub fn min_luminance(&self) -> f32 {
        self.min_luminance
    }

    /// Клеток на единицу шкалы (для GPU).
    pub fn cells_per_unit(&self) -> f32 {
        self.cells_per_unit
    }

    /// Верхняя граница окна (для GPU).
    pub fn window_top(&self) -> f32 {
        self.window_top
    }

    /// Предел сетки `size` (для GPU).
    pub fn size_limit(&self) -> f32 {
        self.size_limit
    }

    /// Ячейка кода `[столбец, строка]`; `None` — нет попадания (чёрное, вне окна).
    pub fn cell_of_code(&self, code: [u16; 3]) -> Option<[u32; 2]> {
        let linear = code.map(|channel| self.decode_table.linear_light(channel));
        let denominator = dot_in_fixed_order(self.rows.denominator, linear);
        let luminance = dot_in_fixed_order(self.rows.luminance, linear);
        // Не число и неположительный знаменатель отсекает отрицание сравнения; яркость ниже порога — тёмный шум.
        if denominator.is_nan() || denominator <= 0.0 || luminance < self.min_luminance {
            return None;
        }
        let column_numerator = dot_in_fixed_order(self.rows.column_numerator, linear);
        let row_numerator = dot_in_fixed_order(self.rows.row_numerator, linear);
        let shifted_column = column_numerator - self.window.left * denominator;
        let column_t = shifted_column * self.cells_per_unit;
        let shifted_row = self.window_top * denominator - row_numerator;
        let row_t = shifted_row * self.cells_per_unit;
        let column = cell_on_axis(column_t, denominator, self.size_limit)?;
        let row = cell_on_axis(row_t, denominator, self.size_limit)?;
        Some([column, row])
    }
}

/// Строки шкалы из матрицы `rgb_to_xyz` измеряемого охвата (f64 → f32 один раз).
fn rows_for(diagram: CieDiagram, rgb_to_xyz: &Matrix3) -> CieRows {
    let [x, y, z] = rgb_to_xyz.rows;
    let combine = |weights: [f64; 3]| -> [f32; 3] {
        std::array::from_fn(|channel| {
            (weights[0] * x[channel] + weights[1] * y[channel] + weights[2] * z[channel]) as f32
        })
    };
    let (column_weights, row_weights, denominator_weights) = match diagram {
        CieDiagram::Xy1931 => ([1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [1.0, 1.0, 1.0]),
        CieDiagram::UvPrime1976 => ([4.0, 0.0, 0.0], [0.0, 9.0, 0.0], [1.0, 15.0, 3.0]),
    };
    CieRows {
        column_numerator: combine(column_weights),
        row_numerator: combine(row_weights),
        denominator: combine(denominator_weights),
        luminance: combine([0.0, 1.0, 0.0]),
    }
}

fn matrix_to_f32(matrix: &Matrix3) -> [[f32; 3]; 3] {
    matrix.rows.map(|row| row.map(|value| value as f32))
}

impl ScopeRule for CieRule {
    fn shape(&self, _samples: SampleCount) -> GridShape {
        // Сетка от кадра не зависит: оси — цветность, а не положение.
        self.geometry.shape()
    }

    fn wanted_steps(&self, settings: &crate::settings::ScopeModelSettings) -> crate::sampling::SampleSteps {
        crate::sampling::SampleSteps::uniform(settings.cie.sample_stride)
    }

    fn hits(&self, _position: SamplePosition, code: [u16; 3], report: &mut dyn FnMut(Hit)) {
        if let Some([column, row]) = self.cell_of_code(code) {
            report(Hit { plane: 0, row, column });
        }
    }
}

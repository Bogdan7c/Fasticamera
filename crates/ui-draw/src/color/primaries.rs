//! Перевод цвета картинки в цвет экрана: кривая sRGB и матрица «основные цвета sRGB → основные цвета экрана».
//!
//! Цвета UI лежат в закодированных значениях экрана (см. модуль `color`), а картинка (миниатюра снимка)
//! закодирована как sRGB. Когда экран — Display P3, одни и те же числа означали бы разные цвета, поэтому
//! рендер переводит пиксель картинки в цвет экрана. Порядок действий ЕДИН для эталона на CPU и шейдера
//! (`ui_image.frag`), иначе паритет бессмыслен:
//!
//! 1. выборка (билинейная) идёт по ЗАКОДИРОВАННЫМ значениям источника — так работает сэмплер GPU над
//!    форматом `Rgba8Unorm`; кривая в шейдере не участвует до этого шага;
//! 2. кривая sRGB → линейный свет по каждому каналу;
//! 3. матрица 3×3 [`DisplayPrimaries::from_srgb_matrix`] (для sRGB-экрана — единичная);
//! 4. линейный свет зажимается в `0..=1` и кодируется кривой sRGB (Display P3 использует ту же кривую);
//! 5. предумножение на альфу × покрытие × прозрачность (в `shade_image_pixel`).

/// Основные цвета экрана, на котором показывается UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DisplayPrimaries {
    /// Основные цвета sRGB (BT.709): картинка sRGB показывается как есть.
    Srgb,
    /// Основные цвета Display P3 (точка белого D65): картинка sRGB переводится матрицей.
    DisplayP3,
}

/// Матрица 3×3 по строкам: `выход = матрица · вход` (линейный свет, столбец RGB).
pub type Matrix3 = [[f32; 3]; 3];

/// Единичная матрица.
const IDENTITY: Matrix3 = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

/// Линейный sRGB → линейный Display P3.
///
/// Получена как `P3→XYZ⁻¹ · sRGB→XYZ` из координат цветности основных цветов и общей точки белого D65
/// (x = 0,3127, y = 0,3290): sRGB — IEC 61966-2-1 (R 0,640/0,330, G 0,300/0,600, B 0,150/0,060),
/// Display P3 — SMPTE EG 432-1 (R 0,680/0,320, G 0,265/0,690, B 0,150/0,060). Совпадает с матрицей,
/// которую публикуют CSS Color 4 и ColorAide. Тест `matrix_matches_derivation_from_chromaticities`
/// выводит её заново из координат и сверяет; строки в сумме дают 1 (серый остаётся серым).
const SRGB_TO_DISPLAY_P3: Matrix3 = [
    [0.822_461_97, 0.177_538_03, 0.0],
    [0.033_194_2, 0.966_805_8, 0.0],
    [0.017_082_63, 0.072_397_44, 0.910_519_93],
];

impl DisplayPrimaries {
    /// Матрица перевода линейного света sRGB в линейный свет экрана.
    pub fn from_srgb_matrix(self) -> Matrix3 {
        match self {
            Self::Srgb => IDENTITY,
            Self::DisplayP3 => SRGB_TO_DISPLAY_P3,
        }
    }
}

/// Кривая sRGB: закодированное значение → линейный свет (IEC 61966-2-1).
pub fn srgb_decode(encoded: f32) -> f32 {
    if encoded <= 0.040_45 {
        encoded / 12.92
    } else {
        ((encoded + 0.055) / 1.055).powf(2.4)
    }
}

/// Кривая sRGB: линейный свет → закодированное значение. Вход зажимается в `0..=1`: отрицательное под
/// степенью дало бы NaN, а больше 1 экран всё равно не покажет.
pub fn srgb_encode(linear: f32) -> f32 {
    let clamped = linear.clamp(0.0, 1.0);
    if clamped <= 0.003_130_8 {
        clamped * 12.92
    } else {
        1.055 * clamped.powf(1.0 / 2.4) - 0.055
    }
}

/// Цвет RGB, закодированный sRGB, → закодированный цвет экрана с основными цветами `primaries`.
/// Альфа не затрагивается (её обрабатывает вызывающий).
pub fn srgb_picture_to_display(encoded_rgb: [f32; 3], primaries: DisplayPrimaries) -> [f32; 3] {
    // Экран sRGB: кривая и единичная матрица взаимно сокращаются, значения остаются как есть (без потерь
    // на округлении туда-обратно). Шейдер делает то же самое.
    if primaries == DisplayPrimaries::Srgb {
        return encoded_rgb;
    }
    let linear = encoded_rgb.map(srgb_decode);
    let matrix = primaries.from_srgb_matrix();
    let converted = matrix.map(|row| row[0] * linear[0] + row[1] * linear[1] + row[2] * linear[2]);
    converted.map(srgb_encode)
}

#[cfg(test)]
mod tests {
    use super::*;

    type Vec3 = [f64; 3];
    type Mat3 = [[f64; 3]; 3];

    /// XYZ белой точки по координатам цветности (Y = 1).
    fn white_xyz(x: f64, y: f64) -> Vec3 {
        [x / y, 1.0, (1.0 - x - y) / y]
    }

    fn invert(m: Mat3) -> Mat3 {
        let det = m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1]) - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
            + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
        let cofactor = |r: usize, c: usize| {
            let (r1, r2) = ((r + 1) % 3, (r + 2) % 3);
            let (c1, c2) = ((c + 1) % 3, (c + 2) % 3);
            m[r1][c1] * m[r2][c2] - m[r1][c2] * m[r2][c1]
        };
        // Обратная = транспонированная матрица алгебраических дополнений / det.
        let mut result = [[0.0; 3]; 3];
        for (r, row) in result.iter_mut().enumerate() {
            for (c, cell) in row.iter_mut().enumerate() {
                *cell = cofactor(c, r) / det;
            }
        }
        result
    }

    fn multiply(a: Mat3, b: Mat3) -> Mat3 {
        let mut out = [[0.0; 3]; 3];
        for r in 0..3 {
            for c in 0..3 {
                out[r][c] = (0..3).map(|k| a[r][k] * b[k][c]).sum();
            }
        }
        out
    }

    /// RGB → XYZ по цветностям основных цветов и точке белого.
    fn rgb_to_xyz(primaries: [(f64, f64); 3], white: (f64, f64)) -> Mat3 {
        let columns: Vec<Vec3> = primaries.iter().map(|&(x, y)| white_xyz(x, y)).collect();
        let unscaled: Mat3 = [
            [columns[0][0], columns[1][0], columns[2][0]],
            [columns[0][1], columns[1][1], columns[2][1]],
            [columns[0][2], columns[1][2], columns[2][2]],
        ];
        let target = white_xyz(white.0, white.1);
        let inverse = invert(unscaled);
        let scale: Vec3 = std::array::from_fn(|r| (0..3).map(|k| inverse[r][k] * target[k]).sum());
        std::array::from_fn(|r| std::array::from_fn(|c| unscaled[r][c] * scale[c]))
    }

    #[test]
    fn matrix_matches_derivation_from_chromaticities() {
        let d65 = (0.3127, 0.3290);
        let srgb = rgb_to_xyz([(0.64, 0.33), (0.30, 0.60), (0.15, 0.06)], d65);
        let p3 = rgb_to_xyz([(0.680, 0.320), (0.265, 0.690), (0.150, 0.060)], d65);
        let derived = multiply(invert(p3), srgb);
        let constant = DisplayPrimaries::DisplayP3.from_srgb_matrix();
        for r in 0..3 {
            for c in 0..3 {
                assert!(
                    (derived[r][c] - f64::from(constant[r][c])).abs() < 1.0e-6,
                    "[{r}][{c}]: вывод {} против константы {}",
                    derived[r][c],
                    constant[r][c]
                );
            }
            let row_sum: f32 = constant[r].iter().sum();
            assert!(
                (row_sum - 1.0).abs() < 1.0e-6,
                "строка {r} суммируется в {row_sum}, серый поплыл бы"
            );
        }
    }

    #[test]
    fn srgb_display_keeps_every_value_and_p3_desaturates_pure_red() {
        let red = [1.0, 0.0, 0.0];
        assert_eq!(srgb_picture_to_display(red, DisplayPrimaries::Srgb), red);
        let in_p3 = srgb_picture_to_display(red, DisplayPrimaries::DisplayP3);
        // Красный sRGB лежит внутри охвата P3: в числах P3 он «менее красный» (≈ 0,917, 0,200, 0,139).
        assert!((in_p3[0] - 0.917).abs() < 0.002, "{in_p3:?}");
        assert!((in_p3[1] - 0.200).abs() < 0.002, "{in_p3:?}");
        assert!((in_p3[2] - 0.139).abs() < 0.002, "{in_p3:?}");
    }

    #[test]
    fn grey_is_identical_in_both_primaries() {
        for level in [0.0, 0.1, 0.5, 0.83, 1.0] {
            let grey = [level; 3];
            let converted = srgb_picture_to_display(grey, DisplayPrimaries::DisplayP3);
            for channel in converted {
                assert!((channel - level).abs() < 1.0e-5, "серый {level} стал {converted:?}");
            }
        }
    }

    #[test]
    fn curve_round_trips_and_clamps_instead_of_producing_nan() {
        for step in 0..=20 {
            let value = step as f32 / 20.0;
            assert!((srgb_encode(srgb_decode(value)) - value).abs() < 1.0e-5, "{value}");
        }
        assert_eq!(srgb_encode(-0.5), 0.0);
        assert!((srgb_encode(2.0) - 1.0).abs() < 1.0e-6);
        assert!((srgb_decode(0.5) - 0.2140).abs() < 1.0e-3);
    }
}

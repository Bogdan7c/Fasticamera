//! OKLab / OKLCh (Björn Ottosson, 2020; код и матрицы — public domain / MIT):
//! <https://bottosson.github.io/posts/oklab/>.
//!
//! Нужен для измерения сдвига оттенка и гладкости пути в белое; слой ручек вида пользуется теми же
//! матрицами. Вход — XYZ с белой D65 (`Y = 1` у белого).

use super::matrix::{Matrix3, Vector3};

/// `M1`: XYZ (D65) → LMS (таблица Оттоссона).
pub(super) const XYZ_TO_LMS: Matrix3 = Matrix3::from_rows([
    [0.8189330101, 0.3618667424, -0.1288597137],
    [0.0329845436, 0.9293118715, 0.0361456387],
    [0.0482003018, 0.2643662691, 0.6338517070],
]);

/// `M2`: нелинейные LMS' → Lab.
pub(super) const LMS_PRIME_TO_LAB: Matrix3 = Matrix3::from_rows([
    [0.2104542553, 0.7936177850, -0.0040720468],
    [1.9779984951, -2.4285922050, 0.4505937099],
    [0.0259040371, 0.7827717662, -0.8086757660],
]);

/// Цвет в OKLCh: светлота, цветность, оттенок в градусах `[0, 360)`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Oklch {
    pub lightness: f64,
    pub chroma: f64,
    pub hue_degrees: f64,
}

/// XYZ D65 → OKLab `[L, a, b]`. Кубический корень со знаком: отрицательный LMS (вне охвата) не даёт NaN.
pub fn xyz_d65_to_oklab(xyz: Vector3) -> Vector3 {
    let lms = XYZ_TO_LMS.apply(xyz);
    LMS_PRIME_TO_LAB.apply(lms.map(f64::cbrt))
}

/// OKLab → OKLCh.
pub fn oklab_to_oklch(lab: Vector3) -> Oklch {
    Oklch {
        lightness: lab[0],
        chroma: lab[1].hypot(lab[2]),
        hue_degrees: lab[2].atan2(lab[1]).to_degrees().rem_euclid(360.0),
    }
}

/// Линейный RGB пространства с матрицей `rgb_to_xyz` (белая D65) → OKLCh.
pub fn linear_rgb_to_oklch(rgb_to_xyz: &Matrix3, rgb: [f32; 3]) -> Oklch {
    oklab_to_oklch(xyz_d65_to_oklab(rgb_to_xyz.apply(rgb.map(f64::from))))
}

/// Наименьшая разность оттенков в градусах, `0…180`.
pub fn hue_difference_degrees(first: f64, second: f64) -> f64 {
    let difference = (first - second).rem_euclid(360.0);
    difference.min(360.0 - difference)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::chromaticity::Primaries;

    #[test]
    fn white_is_neutral_and_srgb_red_matches_published_values() {
        let to_xyz = Primaries::SRGB.rgb_to_xyz().unwrap();
        let white = linear_rgb_to_oklch(&to_xyz, [1.0; 3]);
        assert!((white.lightness - 1.0).abs() < 2e-3 && white.chroma < 2e-3, "{white:?}");
        // Оттоссон: красный sRGB — L = 0,628, C = 0,258, h = 29,2°.
        let red = linear_rgb_to_oklch(&to_xyz, [1.0, 0.0, 0.0]);
        assert!((red.lightness - 0.628).abs() < 1e-3, "{red:?}");
        assert!((red.chroma - 0.258).abs() < 1e-3, "{red:?}");
        assert!((red.hue_degrees - 29.2).abs() < 0.2, "{red:?}");
    }

    #[test]
    fn hue_difference_wraps_around_zero() {
        assert!((hue_difference_degrees(359.0, 1.0) - 2.0).abs() < 1e-12);
        assert!((hue_difference_degrees(10.0, 200.0) - 170.0).abs() < 1e-12);
    }
}

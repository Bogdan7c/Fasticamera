//! OKLab в f32 для рабочего пространства `W` (попиксельно; общий для возврата оттенка AgX и цвета вида).
//!
//! Матрицы Оттоссона (`oklab`) заданы для XYZ D65. Для пространства `W` с матрицей `W → XYZ` готовая
//! матрица «`W` → LMS» — это `M1 · XYZ_from_W`; считается в f64 и сужается до f32 ОДИН раз.
//!
//! Отступление от букв Оттоссона: строки `W → LMS` делятся на отклик белого `(1, 1, 1)`, чтобы серый `W` давал
//! РОВНО равные LMS, а значит `a = b = 0` (у исходной `M1` белая D65 даёт цветность ≈ 5·10⁻⁴: насыщенность ×3
//! красила бы серый). Строки меняются меньше чем на 0,02 %, оттенки и цветности цветов — на доли процента. Обратный путь —
//! обратные матрицы (также в f64). Кубический корень — со знаком: `sign(x)·|x|^(1/3)`; GPU пишет его так же
//! (`sign(x) * pow(abs(x), 1.0/3.0)`), а не через `cbrt` (его в GLSL нет).
//!
//! Источник: <https://bottosson.github.io/posts/oklab/> (public domain / MIT).

use super::chromaticity::Primaries;
use super::matrix::{Matrix3, PixelMatrix, SingularMatrix};
use super::oklab::{LMS_PRIME_TO_LAB, XYZ_TO_LMS};

/// Показатель кубического корня.
const CUBE_ROOT_EXPONENT: f32 = 1.0 / 3.0;

/// Матрицы OKLab для одного рабочего пространства. Открытые числа — то, что GPU кладёт в uniform.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize)]
pub struct OklabWorkspace {
    /// `W → LMS`: `M1 · XYZ_from_W`.
    pub working_to_lms: [[f32; 3]; 3],
    /// `LMS' → Lab` (`M2`).
    pub lms_prime_to_lab: [[f32; 3]; 3],
    /// `Lab → LMS'` (`M2⁻¹`).
    pub lab_to_lms_prime: [[f32; 3]; 3],
    /// `LMS → W` (`(M1 · XYZ_from_W)⁻¹`).
    pub lms_to_working: [[f32; 3]; 3],
}

impl OklabWorkspace {
    /// Строит матрицы для пространства с основными цветами `primaries` (белая D65).
    pub fn new(primaries: &Primaries) -> Result<Self, SingularMatrix> {
        let raw_to_lms = XYZ_TO_LMS.mul(&primaries.rgb_to_xyz()?);
        // Отклик белого (1, 1, 1) по строкам; деление строк на него переводит белый в равные LMS.
        let white_response = raw_to_lms.apply([1.0; 3]);
        let working_to_lms =
            Matrix3::from_rows([0, 1, 2].map(|row| raw_to_lms.rows[row].map(|value| value / white_response[row])));
        Ok(Self {
            working_to_lms: working_to_lms.to_f32().rows,
            lms_prime_to_lab: LMS_PRIME_TO_LAB.to_f32().rows,
            lab_to_lms_prime: LMS_PRIME_TO_LAB.inverse()?.to_f32().rows,
            lms_to_working: working_to_lms.inverse()?.to_f32().rows,
        })
    }

    /// Линейный RGB пространства `W` → OKLab `[L, a, b]`.
    pub fn to_lab(&self, rgb: [f32; 3]) -> [f32; 3] {
        let lms = PixelMatrix {
            rows: self.working_to_lms,
        }
        .apply(rgb);
        PixelMatrix {
            rows: self.lms_prime_to_lab,
        }
        .apply(lms.map(signed_cube_root))
    }

    /// OKLab `[L, a, b]` → линейный RGB пространства `W`.
    pub fn to_rgb(&self, lab: [f32; 3]) -> [f32; 3] {
        let lms_prime = PixelMatrix {
            rows: self.lab_to_lms_prime,
        }
        .apply(lab);
        PixelMatrix {
            rows: self.lms_to_working,
        }
        .apply(lms_prime.map(|channel| channel * channel * channel))
    }
}

/// Кубический корень со знаком (отрицательный LMS вне охвата не даёт NaN).
fn signed_cube_root(value: f32) -> f32 {
    value.abs().powf(CUBE_ROOT_EXPONENT).copysign(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::oklab::{linear_rgb_to_oklch, oklab_to_oklch, xyz_d65_to_oklab};

    #[test]
    fn matches_f64_reference_and_round_trips() {
        let primaries = Primaries::REC_2020;
        let workspace = OklabWorkspace::new(&primaries).unwrap();
        let to_xyz = primaries.rgb_to_xyz().unwrap();
        for rgb in [[0.2_f32, 0.5, 0.1], [1.0, 0.0, 0.0], [0.0, 0.0, 0.9], [0.4, 0.4, 0.4]] {
            let lab = workspace.to_lab(rgb);
            let reference = xyz_d65_to_oklab(to_xyz.apply(rgb.map(f64::from)));
            for axis in 0..3 {
                assert!(
                    (f64::from(lab[axis]) - reference[axis]).abs() < 2e-3,
                    "{rgb:?} {lab:?} {reference:?}"
                );
            }
            let back = workspace.to_rgb(lab);
            for axis in 0..3 {
                assert!((back[axis] - rgb[axis]).abs() < 2e-6, "{rgb:?} -> {back:?}");
            }
            let lch = linear_rgb_to_oklch(&to_xyz, rgb);
            let own = oklab_to_oklch(lab.map(f64::from));
            assert!((lch.chroma - own.chroma).abs() < 2e-3);
        }
    }

    #[test]
    fn grey_has_exactly_zero_chroma_in_every_workspace() {
        // Строки нормированы на отклик белого: серый не красится.
        for primaries in [Primaries::REC_2020, Primaries::SRGB, Primaries::DISPLAY_P3] {
            let workspace = OklabWorkspace::new(&primaries).unwrap();
            for level in [0.01_f32, 0.2, 1.0, 4.0] {
                let [_, a, b] = workspace.to_lab([level; 3]);
                assert!(a.hypot(b) < 2e-6, "{level}: {a} {b}");
            }
        }
    }

    #[test]
    fn negative_light_does_not_produce_nan() {
        let workspace = OklabWorkspace::new(&Primaries::REC_2020).unwrap();
        let lab = workspace.to_lab([-0.1, 0.3, 0.2]);
        assert!(lab.iter().all(|value| value.is_finite()));
        let back = workspace.to_rgb(lab);
        assert!((back[0] + 0.1).abs() < 1e-5);
    }
}

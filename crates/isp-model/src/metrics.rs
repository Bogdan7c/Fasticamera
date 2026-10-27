//! Метрики сравнения: ΔE2000 для цвета, PSNR и максимальная разность для паритета CPU ↔ GPU.
//!
//! Метрики нужны тестам и сверке GPU-шейдеров с эталоном; в конвейер кадра они не входят.

use crate::color::matrix::{Matrix3, SingularMatrix, Vector3};
use crate::image::DisplayGamut;

/// ε формулы CIELAB: `(6/29)³ = 216/24389` (CIE 15:2004, таблица 1, примечание о точных
/// рациональных значениях).
const LAB_EPSILON: f64 = 216.0 / 24389.0;

/// κ формулы CIELAB: `(29/3)³ = 24389/27` (CIE 15:2004).
const LAB_KAPPA: f64 = 24389.0 / 27.0;

/// Поправка линейного участка CIELAB `f(t) = (κ·t + 16) / 116`.
const LAB_LINEAR_OFFSET: f64 = 16.0;
const LAB_LINEAR_DIVISOR: f64 = 116.0;

/// Пороговая разность оттенков, при которой считаем, что они противоположны (в градусах).
/// В точной арифметике пары вроде (−0,001; 2,49)/(0,0011; −2,49) дают ровно 180°, а округление
/// f64 даёт 180 ± 1e-13 и меняет ветку формулы среднего оттенка; допуск возвращает ветку
/// «≤ 180», как в тестовых данных Sharma et al. (4,8045 для пары № 14).
const HUE_TIE_TOLERANCE_DEGREES: f64 = 1e-9;

/// Цвет в CIELAB.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Lab {
    pub l: f64,
    pub a: f64,
    pub b: f64,
}

/// XYZ → CIELAB относительно белого `reference_white` (CIE 15:2004, раздел 8.2.1.1).
///
/// `f(t) = ∛t` при `t > ε`, иначе `(κ·t + 16)/116`;
/// `L = 116·f(Y/Yn) − 16`, `a = 500·(f(X/Xn) − f(Y/Yn))`, `b = 200·(f(Y/Yn) − f(Z/Zn))`.
pub fn xyz_to_lab(xyz: Vector3, reference_white: Vector3) -> Lab {
    let fx = lab_f(xyz[0] / reference_white[0]);
    let fy = lab_f(xyz[1] / reference_white[1]);
    let fz = lab_f(xyz[2] / reference_white[2]);
    Lab {
        l: 116.0 * fy - 16.0,
        a: 500.0 * (fx - fy),
        b: 200.0 * (fy - fz),
    }
}

fn lab_f(ratio: f64) -> f64 {
    if ratio > LAB_EPSILON {
        ratio.cbrt()
    } else {
        (LAB_KAPPA * ratio + LAB_LINEAR_OFFSET) / LAB_LINEAR_DIVISOR
    }
}

/// Перевод линейного RGB экрана в Lab для гаммы: матрица из основных цветов строится один
/// раз (а не на каждый пиксель), белый — D65 этой гаммы. Значения вне 0…1 не обрезаются.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DisplayToLab {
    display_to_xyz: Matrix3,
    reference_white: Vector3,
}

impl DisplayToLab {
    pub fn new(gamut: DisplayGamut) -> Result<Self, SingularMatrix> {
        let primaries = gamut.primaries();
        Ok(Self {
            display_to_xyz: primaries.rgb_to_xyz()?,
            reference_white: primaries.white.to_xyz(),
        })
    }

    pub fn convert(&self, rgb: [f32; 3]) -> Lab {
        xyz_to_lab(self.display_to_xyz.apply(rgb.map(f64::from)), self.reference_white)
    }
}

/// Один пиксель линейного RGB экрана → Lab (для тестов и разовых сравнений; в цикле —
/// [`DisplayToLab`]).
pub fn display_linear_to_lab(rgb: [f32; 3], gamut: DisplayGamut) -> Result<Lab, SingularMatrix> {
    Ok(DisplayToLab::new(gamut)?.convert(rgb))
}

/// Цветовая разность CIEDE2000 (`kL = kC = kH = 1`).
///
/// Формулы и порядок шагов: G. Sharma, W. Wu, E. N. Dalal, «The CIEDE2000 color-difference
/// formula: implementation notes, supplementary test data, and mathematical observations»,
/// Color Research & Application 30(1), 2005, раздел 2.
pub fn delta_e_2000(reference: Lab, sample: Lab) -> f64 {
    // Шаг 1: поправка G к оси a' для малонасыщенных цветов.
    let chroma_reference = reference.a.hypot(reference.b);
    let chroma_sample = sample.a.hypot(sample.b);
    let chroma_mean = (chroma_reference + chroma_sample) * 0.5;
    let chroma_mean_pow7 = chroma_mean.powi(7);
    // 25⁷ = 6103515625 — константа формулы CIEDE2000.
    let g = 0.5 * (1.0 - (chroma_mean_pow7 / (chroma_mean_pow7 + 25f64.powi(7))).sqrt());
    let a_reference = (1.0 + g) * reference.a;
    let a_sample = (1.0 + g) * sample.a;
    let c_reference = a_reference.hypot(reference.b);
    let c_sample = a_sample.hypot(sample.b);
    let h_reference = hue_degrees(reference.b, a_reference);
    let h_sample = hue_degrees(sample.b, a_sample);

    // Шаг 2: разности L', C', H'.
    let delta_l = sample.l - reference.l;
    let delta_c = c_sample - c_reference;
    let chroma_product = c_reference * c_sample;
    let delta_h_degrees = if chroma_product == 0.0 {
        0.0
    } else {
        let difference = h_sample - h_reference;
        if difference > 180.0 + HUE_TIE_TOLERANCE_DEGREES {
            difference - 360.0
        } else if difference < -180.0 - HUE_TIE_TOLERANCE_DEGREES {
            difference + 360.0
        } else {
            difference
        }
    };
    let delta_h = 2.0 * chroma_product.sqrt() * (delta_h_degrees.to_radians() * 0.5).sin();

    // Шаг 3: средние L', C', h' и весовые функции.
    let l_mean = (reference.l + sample.l) * 0.5;
    let c_mean = (c_reference + c_sample) * 0.5;
    let h_mean = mean_hue_degrees(h_reference, h_sample, chroma_product);
    let t = 1.0 - 0.17 * (h_mean - 30.0).to_radians().cos()
        + 0.24 * (2.0 * h_mean).to_radians().cos()
        + 0.32 * (3.0 * h_mean + 6.0).to_radians().cos()
        - 0.20 * (4.0 * h_mean - 63.0).to_radians().cos();
    let delta_theta = 30.0 * (-((h_mean - 275.0) / 25.0).powi(2)).exp();
    let c_mean_pow7 = c_mean.powi(7);
    let rotation_c = 2.0 * (c_mean_pow7 / (c_mean_pow7 + 25f64.powi(7))).sqrt();
    let l_offset_squared = (l_mean - 50.0).powi(2);
    let s_l = 1.0 + 0.015 * l_offset_squared / (20.0 + l_offset_squared).sqrt();
    let s_c = 1.0 + 0.045 * c_mean;
    let s_h = 1.0 + 0.015 * c_mean * t;
    let rotation_term = -(2.0 * delta_theta).to_radians().sin() * rotation_c;

    // Шаг 4: итог.
    let l_term = delta_l / s_l;
    let c_term = delta_c / s_c;
    let h_term = delta_h / s_h;
    (l_term * l_term + c_term * c_term + h_term * h_term + rotation_term * c_term * h_term).sqrt()
}

/// Оттенок `atan2(b, a')` в градусах 0…360; для нулевого вектора — 0.
fn hue_degrees(b: f64, a_prime: f64) -> f64 {
    if b == 0.0 && a_prime == 0.0 {
        return 0.0;
    }
    let degrees = b.atan2(a_prime).to_degrees();
    if degrees < 0.0 { degrees + 360.0 } else { degrees }
}

/// Средний оттенок h̄' с учётом перехода через 0°/360° (Sharma 2005, формула 14).
fn mean_hue_degrees(h_reference: f64, h_sample: f64, chroma_product: f64) -> f64 {
    if chroma_product == 0.0 {
        return h_reference + h_sample;
    }
    let sum = h_reference + h_sample;
    if (h_reference - h_sample).abs() <= 180.0 + HUE_TIE_TOLERANCE_DEGREES {
        sum * 0.5
    } else if sum < 360.0 {
        (sum + 360.0) * 0.5
    } else {
        (sum - 360.0) * 0.5
    }
}

/// Ошибка вычисления метрик по массивам отсчётов.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum MetricsError {
    #[error("длины массивов различаются: эталон {reference}, образец {sample}")]
    LengthMismatch { reference: usize, sample: usize },
    #[error("массивы пусты: метрика не определена")]
    Empty,
    #[error("пиковое значение должно быть конечным и положительным")]
    InvalidPeak,
    #[error("в данных есть NaN или бесконечность: метрика не определена")]
    NonFiniteSample,
}

/// Результат PSNR: у одинаковых данных ошибка нулевая и PSNR бесконечен — это отдельный случай.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Psnr {
    /// Отношение пика к среднеквадратичной ошибке в децибелах.
    Finite(f64),
    /// Массивы совпадают бит в бит по значению (MSE = 0).
    Identical,
}

/// `PSNR = 10 · log10(peak² / MSE)`, дБ. `peak` — максимально возможное значение отсчёта
/// (1,0 для нормированных линейных изображений).
pub fn psnr(reference: &[f32], sample: &[f32], peak: f32) -> Result<Psnr, MetricsError> {
    check_comparable(reference, sample)?;
    if !peak.is_finite() || peak <= 0.0 {
        return Err(MetricsError::InvalidPeak);
    }
    let squared_error_sum: f64 = reference
        .iter()
        .zip(sample)
        .map(|(expected, actual)| (f64::from(*expected) - f64::from(*actual)).powi(2))
        .sum();
    if !squared_error_sum.is_finite() {
        return Err(MetricsError::NonFiniteSample);
    }
    if squared_error_sum == 0.0 {
        return Ok(Psnr::Identical);
    }
    let mean_squared_error = squared_error_sum / reference.len() as f64;
    Ok(Psnr::Finite(
        10.0 * (f64::from(peak).powi(2) / mean_squared_error).log10(),
    ))
}

/// Максимум `|эталон − образец|` по всем отсчётам.
pub fn max_abs_difference(reference: &[f32], sample: &[f32]) -> Result<f32, MetricsError> {
    check_comparable(reference, sample)?;
    let mut largest = 0.0_f32;
    for (expected, actual) in reference.iter().zip(sample) {
        let difference = (expected - actual).abs();
        // `f32::max` молча игнорирует NaN, поэтому проверяем явно.
        if !difference.is_finite() {
            return Err(MetricsError::NonFiniteSample);
        }
        largest = largest.max(difference);
    }
    Ok(largest)
}

fn check_comparable(reference: &[f32], sample: &[f32]) -> Result<(), MetricsError> {
    if reference.len() != sample.len() {
        return Err(MetricsError::LengthMismatch {
            reference: reference.len(),
            sample: sample.len(),
        });
    }
    if reference.is_empty() {
        return Err(MetricsError::Empty);
    }
    Ok(())
}

#[cfg(test)]
#[path = "metrics_tests.rs"]
mod tests;

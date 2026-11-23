//! Рабочее пространство формирования картинки — линейный Rec. 2020 (D65), решение 176.
//!
//! Здесь только матрицы (всё в f64, в f32 их сужает вызывающий один раз):
//! * `XYZ D50 → W` — Брэдфорд D50 → D65 и основные цвета Rec. 2020;
//! * `W → цвета выхода` — обе стороны с белой D65, поэтому без адаптации;
//! * матрицы «внутрь» и «наружу» основы AgX: основные цвета W сводятся к белой точке и слегка
//!   поворачиваются в плоскости цветности xy.
//!
//! # Матрицы «внутрь» и «наружу»
//! Пусть `p_i` — основные цвета W в координатах цветности xy, `w` — белая точка (D65). Для каждого
//! основного цвета задано сведение `a_i` (0 — не двигать, доля пути к белой) и поворот `θ_i` вокруг белой:
//! ```text
//! p_i' = w + (1 − a_i) · Rot(θ_i) · (p_i − w)
//! B(p') = RGB_W_from_XYZ · XYZ_from_RGB(p', w)         — «координаты в пространстве p' → координаты в W»
//! M_in  = B(p'_in)          M_out = B(p'_out)⁻¹
//! ```
//! Числа на входе `M_in` трактуются как координаты в «суженном» пространстве, поэтому чистый цвет
//! теряет чистоту. Строки `B` дают в сумме 1 (оба пространства имеют белую D65), значит серое и
//! белый `(L, L, L)` остаются собой. При одинаковых параметрах `M_out = M_in⁻¹` ровно.
//! Положительный `θ` — против часовой стрелки на диаграмме xy.

use super::chromaticity::{Chromaticity, Primaries, xyz_d50_to_linear_rgb};
use super::matrix::{Matrix3, SingularMatrix};
use crate::image::DisplayGamut;

/// Рабочее пространство: основные цвета и белая.
pub const WORKING_PRIMARIES: Primaries = Primaries::REC_2020;

/// Сведение и поворот одного основного цвета (конфиг; значения — оценка, подбор в 14b).
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrimaryAdjustment {
    /// Доля пути к белой точке: 0 — на месте, 0,3 — на 30 % ближе к белой. Допустимо `0 ≤ a < 1`.
    pub amount: f32,
    /// Поворот вокруг белой точки в градусах (против часовой стрелки на диаграмме xy).
    pub rotation_degrees: f32,
}

impl PrimaryAdjustment {
    /// Без изменений.
    pub const NONE: Self = Self {
        amount: 0.0,
        rotation_degrees: 0.0,
    };
}

/// Корректировка основных цветов не имеет смысла.
#[derive(Debug, Clone, Copy, PartialEq, thiserror::Error)]
pub enum InvalidPrimaryAdjustment {
    #[error("сведение основного цвета {amount} вне [0, 1)")]
    AmountOutOfRange { amount: f32 },
    #[error("поворот основного цвета {rotation_degrees}° не конечен или вне ±90°")]
    RotationOutOfRange { rotation_degrees: f32 },
}

impl PrimaryAdjustment {
    /// Проверка диапазонов (NaN отвергается).
    pub fn validate(&self) -> Result<(), InvalidPrimaryAdjustment> {
        if !(0.0..1.0).contains(&self.amount) {
            return Err(InvalidPrimaryAdjustment::AmountOutOfRange { amount: self.amount });
        }
        if !(-90.0..=90.0).contains(&self.rotation_degrees) {
            return Err(InvalidPrimaryAdjustment::RotationOutOfRange {
                rotation_degrees: self.rotation_degrees,
            });
        }
        Ok(())
    }
}

/// Основные цвета `p'` после сведения и поворота (формула модуля).
pub fn adjusted_primaries(base: &Primaries, adjustments: &[PrimaryAdjustment; 3]) -> Primaries {
    let white = base.white;
    let adjust = |primary: Chromaticity, adjustment: &PrimaryAdjustment| {
        let angle = f64::from(adjustment.rotation_degrees).to_radians();
        let keep = 1.0 - f64::from(adjustment.amount);
        let (dx, dy) = (primary.x - white.x, primary.y - white.y);
        Chromaticity {
            x: white.x + keep * (dx * angle.cos() - dy * angle.sin()),
            y: white.y + keep * (dx * angle.sin() + dy * angle.cos()),
        }
    };
    Primaries {
        red: adjust(base.red, &adjustments[0]),
        green: adjust(base.green, &adjustments[1]),
        blue: adjust(base.blue, &adjustments[2]),
        white,
    }
}

/// `B(p') = RGB_W_from_XYZ · XYZ_from_RGB(p')`: координаты в пространстве `p'` → координаты в W.
fn adjusted_to_working(adjustments: &[PrimaryAdjustment; 3]) -> Result<Matrix3, SingularMatrix> {
    let adjusted = adjusted_primaries(&WORKING_PRIMARIES, adjustments).rgb_to_xyz()?;
    Ok(WORKING_PRIMARIES.rgb_to_xyz()?.inverse()?.mul(&adjusted))
}

/// Матрица «внутрь»: `M_in = B(p'_in)`.
pub fn inset_matrix(adjustments: &[PrimaryAdjustment; 3]) -> Result<Matrix3, SingularMatrix> {
    adjusted_to_working(adjustments)
}

/// Матрица «наружу»: `M_out = B(p'_out)⁻¹`.
pub fn outset_matrix(adjustments: &[PrimaryAdjustment; 3]) -> Result<Matrix3, SingularMatrix> {
    adjusted_to_working(adjustments)?.inverse()
}

/// `XYZ D50 → W` (Брэдфорд D50 → D65 внутри).
pub fn xyz_d50_to_working() -> Result<Matrix3, SingularMatrix> {
    xyz_d50_to_linear_rgb(&WORKING_PRIMARIES)
}

/// `W → линейные цвета выхода`: `XYZ_from_RGB(выход)⁻¹ · XYZ_from_RGB(W)`; белая D65 с обеих сторон.
pub fn working_to_output(gamut: DisplayGamut) -> Result<Matrix3, SingularMatrix> {
    Ok(gamut
        .primaries()
        .rgb_to_xyz()?
        .inverse()?
        .mul(&WORKING_PRIMARIES.rgb_to_xyz()?))
}

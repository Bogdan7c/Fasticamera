//! Координаты цветности, основные цвета экрана и адаптация белой точки по Брэдфорду.
//!
//! Здесь связываются два мира: DNG работает в CIE XYZ с белой D50 (профильное
//! пространство ICC), а экран — в RGB со своими основными цветами и белой D65.
//! Путь `XYZ D50 → линейный RGB экрана` — это [`xyz_d50_to_scene_linear`].

use super::matrix::{Matrix3, SingularMatrix, Vector3};
use crate::image::DisplayGamut;

/// Порог суммы X+Y+Z, ниже которого цветность не определена (деление на ~0).
const MIN_TRISTIMULUS_SUM: f64 = 1e-12;

/// Координаты цветности CIE 1931 (xy). Яркость Y в них не хранится.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Chromaticity {
    pub x: f64,
    pub y: f64,
}

impl Chromaticity {
    /// D50 по определению DNG/ICC: `x = 0,3457`, `y = 0,3585`.
    /// Источник: Adobe DNG SDK (`PCStoXY`) и ICC v4; совпадает с `CCS_ILLUMINANT_ADOBEDNG`
    /// из colour-hdri (CIE 1931, 2°).
    pub const D50: Self = Self { x: 0.3457, y: 0.3585 };

    /// D65 по IEC 61966-2-1 (sRGB) и SMPTE EG 432-1 (Display P3): `x = 0,3127`, `y = 0,3290`.
    pub const D65: Self = Self { x: 0.3127, y: 0.3290 };

    /// XYZ белого с яркостью `Y = 1`: `X = x/y`, `Z = (1 − x − y)/y`.
    ///
    /// Требует `y > 0`; для `y = 0` результат нефинитен (такой цветности не бывает у света).
    pub fn to_xyz(self) -> Vector3 {
        [self.x / self.y, 1.0, (1.0 - self.x - self.y) / self.y]
    }

    /// Цветность по XYZ: `x = X/(X+Y+Z)`, `y = Y/(X+Y+Z)`.
    /// `None`, если сумма ≈ 0 или компоненты не конечны (чёрный/битый вектор).
    pub fn from_xyz(xyz: Vector3) -> Option<Self> {
        let sum: f64 = xyz.iter().sum();
        if !sum.is_finite() || sum.abs() < MIN_TRISTIMULUS_SUM {
            return None;
        }
        Some(Self {
            x: xyz[0] / sum,
            y: xyz[1] / sum,
        })
    }
}

/// Основные цвета RGB-пространства и его белая точка.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Primaries {
    pub red: Chromaticity,
    pub green: Chromaticity,
    pub blue: Chromaticity,
    pub white: Chromaticity,
}

impl Primaries {
    /// sRGB: основные цвета Rec. 709 (ITU-R BT.709) и D65, IEC 61966-2-1.
    pub const SRGB: Self = Self {
        red: Chromaticity { x: 0.64, y: 0.33 },
        green: Chromaticity { x: 0.30, y: 0.60 },
        blue: Chromaticity { x: 0.15, y: 0.06 },
        white: Chromaticity::D65,
    };

    /// Display P3: основные цвета DCI-P3, белый D65, SMPTE EG 432-1.
    pub const DISPLAY_P3: Self = Self {
        red: Chromaticity { x: 0.680, y: 0.320 },
        green: Chromaticity { x: 0.265, y: 0.690 },
        blue: Chromaticity { x: 0.150, y: 0.060 },
        white: Chromaticity::D65,
    };

    /// Rec. 2020 (ITU-R BT.2020-2, таблица 4) и D65: широкое рабочее пространство AgX и ручек вида
    /// (решение 176). Основные цвета лежат на спектральном локусе, поэтому у реальных цветов сцены
    /// отрицательных компонент здесь почти нет.
    pub const REC_2020: Self = Self {
        red: Chromaticity { x: 0.708, y: 0.292 },
        green: Chromaticity { x: 0.170, y: 0.797 },
        blue: Chromaticity { x: 0.131, y: 0.046 },
        white: Chromaticity::D65,
    };

    /// Матрица линейный RGB → XYZ по SMPTE RP 177 (приложение B).
    ///
    /// Столбцы — XYZ основных цветов (Y = 1), масштабированные коэффициентами
    /// `S = P⁻¹ · XYZ(белый)` так, чтобы RGB (1, 1, 1) давал в точности белую точку.
    /// Матрица не хранится готовой: всё выводится из координат цветности.
    pub fn rgb_to_xyz(&self) -> Result<Matrix3, SingularMatrix> {
        let red = self.red.to_xyz();
        let green = self.green.to_xyz();
        let blue = self.blue.to_xyz();
        // Столбцы P — ненормированные XYZ основных цветов.
        let unscaled = Matrix3::from_rows([
            [red[0], green[0], blue[0]],
            [red[1], green[1], blue[1]],
            [red[2], green[2], blue[2]],
        ]);
        let channel_scale = unscaled.inverse()?.apply(self.white.to_xyz());
        Ok(unscaled.mul(&Matrix3::diagonal(channel_scale)))
    }
}

impl DisplayGamut {
    /// Основные цвета и белая точка гаммы.
    pub fn primaries(self) -> Primaries {
        match self {
            DisplayGamut::Srgb => Primaries::SRGB,
            DisplayGamut::DisplayP3 => Primaries::DISPLAY_P3,
        }
    }
}

/// Матрица конусов Брэдфорда (XYZ → отклики «конусов»).
/// Источник: Lindbloom, «Chromatic Adaptation»; CIE 160:2004; ICC v4, приложение E.
const BRADFORD_CONE_RESPONSE: Matrix3 = Matrix3::from_rows([
    [0.8951, 0.2664, -0.1614],
    [-0.7502, 1.7135, 0.0367],
    [0.0389, -0.0685, 1.0296],
]);

/// Матрица хроматической адаптации Брэдфорда (фон Кризс в пространстве конусов):
/// `M⁻¹ · diag(ρ_dst / ρ_src) · M`, где `ρ = M · XYZ(белый)`, `M` — матрица конусов Брэдфорда.
///
/// Переводит XYZ, снятые при белом `source_white`, в XYZ при белом `destination_white`;
/// сам `source_white` (Y = 1) уходит ровно в `destination_white`. Вырождение — это нулевой
/// отклик конуса исходного белого (деление на 0) либо вырожденная матрица.
pub fn bradford_adaptation(
    source_white: Chromaticity,
    destination_white: Chromaticity,
) -> Result<Matrix3, SingularMatrix> {
    let source_cones = BRADFORD_CONE_RESPONSE.apply(source_white.to_xyz());
    let destination_cones = BRADFORD_CONE_RESPONSE.apply(destination_white.to_xyz());
    let mut gains = [0.0; 3];
    for ((gain, source), destination) in gains.iter_mut().zip(source_cones).zip(destination_cones) {
        if source == 0.0 || !source.is_finite() || !destination.is_finite() {
            return Err(SingularMatrix { determinant: 0.0 });
        }
        *gain = destination / source;
    }
    let cone_to_xyz = BRADFORD_CONE_RESPONSE.inverse()?;
    Ok(cone_to_xyz.mul(&Matrix3::diagonal(gains)).mul(&BRADFORD_CONE_RESPONSE))
}

/// Матрица `XYZ D50 → линейный RGB экрана`: `M_rgb→xyz⁻¹ · Bradford(D50 → белая гаммы)`.
///
/// Белая D50 уходит ровно в RGB (1, 1, 1) — серое остаётся серым.
pub fn xyz_d50_to_scene_linear(gamut: DisplayGamut) -> Result<Matrix3, SingularMatrix> {
    xyz_d50_to_linear_rgb(&gamut.primaries())
}

/// Та же матрица для произвольных основных цветов (рабочее пространство Rec. 2020 и гаммы экрана).
pub fn xyz_d50_to_linear_rgb(primaries: &Primaries) -> Result<Matrix3, SingularMatrix> {
    let display_to_xyz = primaries.rgb_to_xyz()?;
    let adaptation = bradford_adaptation(Chromaticity::D50, primaries.white)?;
    Ok(display_to_xyz.inverse()?.mul(&adaptation))
}

#[cfg(test)]
#[path = "chromaticity_tests.rs"]
mod tests;

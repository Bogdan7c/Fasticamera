//! Демозаик: мозаика Bayer после баланса белого → линейный RGB камеры.
//!
//! Два режима ([`DemosaicMethod`]): быстрый суперпиксель для видоискателя и качественный
//! Menon 2007 (с шагом уточнения) для фото. Выбор метода — решение владельца (сессия 06).

mod menon2007;
mod planes;
mod superpixel;

use crate::image::{LinearCameraRgb, WhiteBalancedMosaic};

/// Метод демозаика.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DemosaicMethod {
    /// Суперпиксель 2×2, половинное разрешение (видоискатель).
    Preview,
    /// Menon 2007 с шагом уточнения, полное разрешение (фото).
    Quality,
}

/// Демозаик выбранным методом.
pub fn demosaic(mosaic: &WhiteBalancedMosaic, method: DemosaicMethod) -> LinearCameraRgb {
    match method {
        DemosaicMethod::Preview => demosaic_preview(mosaic),
        DemosaicMethod::Quality => demosaic_quality(mosaic),
    }
}

/// Суперпиксель: результат (W/2)×(H/2); R — красная ячейка квадрата 2×2, G — среднее двух зелёных,
/// B — синяя ячейка.
pub fn demosaic_preview(mosaic: &WhiteBalancedMosaic) -> LinearCameraRgb {
    superpixel::demosaic_superpixel(mosaic)
}

/// Menon 2007 (DDFAPD) с шагом уточнения; тот же размер, что у мозаики. Значения не обрезаются:
/// у резких краёв возможен лёгкий звон за пределами 0…1 (обрезка — дело стадии тона).
pub fn demosaic_quality(mosaic: &WhiteBalancedMosaic) -> LinearCameraRgb {
    menon2007::demosaic_menon2007(mosaic)
}

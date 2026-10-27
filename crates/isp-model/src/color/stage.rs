//! Стадия 5: попиксельное применение подготовленных на кадр матриц цвета.
//!
//! Матрицы готовит [`crate::params::IspParams::resolve`] (DNG-математика в f64), а здесь —
//! только умножение каждого пикселя на матрицу в f32, как в шейдере. GPU может склеить две
//! матрицы в одну; эталон держит их раздельно, чтобы промежуточный XYZ можно было сверять.

use crate::color::matrix::PixelMatrix;
use crate::image::{DisplayGamut, LinearCameraRgb, SceneLinearImage, XyzD50Image, space};

/// RGB камеры после баланса белого → CIE XYZ D50.
///
/// `camera_to_xyz_d50` уже учитывает усиления баланса белого (см. `params::ColorTransforms`):
/// серый объект `t·(1,1,1)` переходит в `t·XYZ(D50)`.
pub fn camera_rgb_to_xyz_d50(image: &LinearCameraRgb, camera_to_xyz_d50: &PixelMatrix) -> XyzD50Image {
    image.map_into(space::XyzD50, |pixel| camera_to_xyz_d50.apply(pixel))
}

/// XYZ D50 → линейный свет сцены в основных цветах экрана (адаптация Брэдфорда D50 → D65
/// уже внутри матрицы).
pub fn xyz_d50_to_scene_linear(
    image: &XyzD50Image,
    xyz_d50_to_scene: &PixelMatrix,
    gamut: DisplayGamut,
) -> SceneLinearImage {
    image.map_into(space::SceneLinear { gamut }, |pixel| xyz_d50_to_scene.apply(pixel))
}

#[cfg(test)]
#[path = "stage_tests.rs"]
mod tests;

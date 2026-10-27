use camera_contract::geometry::PixelSize;

use super::{camera_rgb_to_xyz_d50, xyz_d50_to_scene_linear};
use crate::color::matrix::Matrix3;
use crate::image::{DisplayGamut, LinearCameraRgb, XyzD50Image, space};

#[test]
fn camera_to_xyz_applies_matrix_to_every_pixel() {
    let matrix = Matrix3::from_rows([[0.5, 0.25, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 2.0]]).to_f32();
    let image = LinearCameraRgb::from_pixels(
        PixelSize::new(2, 1),
        vec![[1.0, 1.0, 1.0], [0.2, 0.4, 0.1]],
        space::CameraRgb,
    )
    .unwrap();
    let xyz = camera_rgb_to_xyz_d50(&image, &matrix);
    assert_eq!(xyz.pixels(), &[[0.75, 1.0, 2.0], [0.2, 0.4, 0.2]]);
    assert_eq!(xyz.size(), PixelSize::new(2, 1));
}

#[test]
fn scene_image_carries_requested_gamut() {
    let xyz = XyzD50Image::from_pixels(PixelSize::new(1, 1), vec![[0.3, 0.2, 0.1]], space::XyzD50).unwrap();
    let swap = Matrix3::from_rows([[0.0, 0.0, 1.0], [0.0, 1.0, 0.0], [1.0, 0.0, 0.0]]).to_f32();
    let scene = xyz_d50_to_scene_linear(&xyz, &swap, DisplayGamut::DisplayP3);
    assert_eq!(scene.space().gamut, DisplayGamut::DisplayP3);
    assert_eq!(scene.pixel(0, 0), [0.1, 0.2, 0.3]);
}

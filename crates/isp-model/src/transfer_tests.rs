use camera_contract::geometry::PixelSize;

use super::{encode_display, srgb_decode, srgb_encode};
use crate::image::{DisplayGamut, DisplayLinearImage, space};

#[test]
fn known_points_of_srgb_curve() {
    assert_eq!(srgb_encode(0.0), 0.0);
    assert!((srgb_encode(1.0) - 1.0).abs() < 1e-6);
    // Средний серый 18 % кодируется в ≈ 0,4613 (≈ 118 из 255).
    assert!((srgb_encode(0.18) - 0.461_356).abs() < 1e-5, "{}", srgb_encode(0.18));
    // Линейный участок: 0,001 · 12,92.
    assert!((srgb_encode(0.001) - 0.012_92).abs() < 1e-7);
}

#[test]
fn curve_is_continuous_at_the_segment_joint() {
    let below = srgb_encode(0.003_130_8);
    let above = srgb_encode(0.003_130_9);
    assert!((above - below).abs() < 1e-5, "{below} {above}");
}

#[test]
fn decode_inverts_encode_over_whole_range() {
    for step in 0..=1000 {
        let linear = step as f32 / 1000.0;
        let round_trip = srgb_decode(srgb_encode(linear));
        assert!((round_trip - linear).abs() < 2e-6, "{linear} → {round_trip}");
    }
}

#[test]
fn out_of_range_light_is_clamped() {
    assert_eq!(srgb_encode(-0.5), 0.0);
    assert!((srgb_encode(4.0) - 1.0).abs() < 1e-6);
}

#[test]
fn encode_display_keeps_gamut_and_encodes_each_channel() {
    let image = DisplayLinearImage::from_pixels(
        PixelSize::new(1, 1),
        vec![[0.0, 0.18, 1.0]],
        space::DisplayLinear {
            gamut: DisplayGamut::DisplayP3,
        },
    )
    .unwrap();
    let encoded = encode_display(&image);
    assert_eq!(encoded.space().gamut, DisplayGamut::DisplayP3);
    let [red, green, blue] = encoded.pixel(0, 0);
    assert_eq!(red, 0.0);
    assert!((green - 0.461_356).abs() < 1e-5);
    assert!((blue - 1.0).abs() < 1e-6);
}

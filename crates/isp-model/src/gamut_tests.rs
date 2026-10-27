use camera_contract::geometry::PixelSize;

use super::{GamutCompression, GamutCompressionSettings, InvalidGamutCompression, compress_to_display_gamut};
use crate::image::{DisplayGamut, SceneLinearImage, space};

fn default_compression() -> GamutCompression {
    GamutCompression::resolve(&GamutCompressionSettings::default()).unwrap()
}

#[test]
fn colors_inside_threshold_are_untouched() {
    let compression = default_compression();
    for pixel in [[0.5, 0.5, 0.5], [0.8, 0.4, 0.3], [0.2, 0.25, 0.3], [0.0, 0.0, 0.0]] {
        assert_eq!(compression.compress_pixel(pixel), pixel, "{pixel:?}");
    }
}

#[test]
fn distance_limit_lands_exactly_on_gamut_boundary() {
    let settings = GamutCompressionSettings::default();
    let compression = GamutCompression::resolve(&settings).unwrap();
    // Красный канал на расстоянии limit[0] от серого: c = ach·(1 − limit) < 0.
    let achromatic = 0.6;
    let red = achromatic * (1.0 - settings.distance_limit[0]);
    let [compressed_red, green, blue] = compression.compress_pixel([red, achromatic, achromatic]);
    assert!(red < 0.0);
    assert!(
        compressed_red.abs() < 1e-5,
        "на границе охвата канал = 0, получено {compressed_red}"
    );
    assert_eq!(
        [green, blue],
        [achromatic, achromatic],
        "максимальный канал не меняется"
    );
}

#[test]
fn compression_is_monotonic_and_continuous() {
    let compression = default_compression();
    let mut previous = f32::INFINITY;
    // Синий канал идёт от серого (0,5) в минус: результат не возрастает и не прыгает.
    for step in 0..=300 {
        let blue = 0.5 - step as f32 * 0.002;
        let [_, _, compressed] = compression.compress_pixel([0.5, 0.4, blue]);
        assert!(compressed <= previous + 1e-6, "немонотонно на {blue}");
        if previous.is_finite() {
            assert!((previous - compressed).abs() < 0.01, "скачок на {blue}");
        }
        previous = compressed;
    }
}

#[test]
fn out_of_gamut_pixels_become_non_negative() {
    let compression = default_compression();
    let output = compression.compress_pixel([1.2, -0.3, 0.1]);
    assert!(output.iter().all(|&channel| channel >= 0.0), "{output:?}");
    assert_eq!(output[0], 1.2, "ахроматическая ось сохранена");
}

#[test]
fn fully_negative_pixel_is_black() {
    assert_eq!(default_compression().compress_pixel([-0.1, -0.2, -0.05]), [0.0; 3]);
}

#[test]
fn stage_keeps_gamut_tag() {
    let image = SceneLinearImage::from_pixels(
        PixelSize::new(1, 1),
        vec![[0.9, -0.1, 0.2]],
        space::SceneLinear {
            gamut: DisplayGamut::Srgb,
        },
    )
    .unwrap();
    let compressed = compress_to_display_gamut(&image, &default_compression());
    assert_eq!(compressed.space().gamut, DisplayGamut::Srgb);
    assert!(compressed.pixel(0, 0)[1] >= 0.0);
}

#[test]
fn invalid_settings_are_rejected() {
    let mut settings = GamutCompressionSettings::default();
    settings.threshold[1] = 1.0;
    assert_eq!(
        GamutCompression::resolve(&settings),
        Err(InvalidGamutCompression::ThresholdOutOfRange {
            channel: 1,
            threshold: 1.0
        })
    );
    let mut settings = GamutCompressionSettings::default();
    settings.distance_limit[2] = 0.9;
    assert_eq!(
        GamutCompression::resolve(&settings),
        Err(InvalidGamutCompression::LimitNotAboveBoundary { channel: 2, limit: 0.9 })
    );
    let settings = GamutCompressionSettings {
        power: 0.5,
        ..GamutCompressionSettings::default()
    };
    assert_eq!(
        GamutCompression::resolve(&settings),
        Err(InvalidGamutCompression::InvalidPower { power: 0.5 })
    );
}

#[test]
fn nan_pixel_is_black() {
    assert_eq!(default_compression().compress_pixel([f32::NAN, 0.2, 0.1]), [0.0; 3]);
}

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

/// Формула модуля, вычисленная заново ТОЛЬКО из экспортированных чисел — так, как её считает шейдер.
fn compress_from_parameters(parameters: &super::GamutCompressionParameters, pixel: [f32; 3]) -> [f32; 3] {
    let achromatic = pixel[0].max(pixel[1]).max(pixel[2]);
    if achromatic <= 0.0 {
        return [0.0; 3];
    }
    let mut output = [0.0; 3];
    for channel in 0..3 {
        let distance = (achromatic - pixel[channel]) / achromatic;
        let threshold = parameters.threshold[channel];
        let compressed = if distance < threshold {
            distance
        } else {
            let scale = parameters.scale[channel];
            let normalized = (distance - threshold) / scale;
            threshold + scale * normalized / (1.0 + normalized.powf(parameters.power)).powf(1.0 / parameters.power)
        };
        output[channel] = (achromatic - compressed * achromatic).max(0.0);
    }
    output
}

#[test]
fn exported_parameters_reproduce_compression_bit_for_bit() {
    // Разные пороги, пределы и степень по каналам: перепутанные поля или каналы экспорта
    // дали бы другой результат хотя бы на одном пикселе.
    let settings = GamutCompressionSettings {
        threshold: [0.7, 0.8, 0.9],
        distance_limit: [1.1, 1.3, 1.5],
        power: 1.5,
    };
    let compression = GamutCompression::resolve(&settings).unwrap();
    let parameters = compression.parameters();
    assert_eq!(parameters.threshold, settings.threshold);
    assert_eq!(parameters.power, settings.power);
    // Пиксели и внутри порога, и в колене, и за пределом — по каждому каналу.
    let pixels = [
        [0.5, 0.45, 0.48],
        [-0.05, 0.6, 0.6],
        [0.6, -0.1, 0.6],
        [0.6, 0.6, -0.2],
        [0.9, 0.12, -0.3],
        [-0.4, 0.3, 1.2],
        [0.05, 0.9, 0.02],
    ];
    let mut compressed_pixels = 0;
    for pixel in pixels {
        let expected = compression.compress_pixel(pixel);
        let reproduced = compress_from_parameters(&parameters, pixel);
        assert_eq!(expected.map(f32::to_bits), reproduced.map(f32::to_bits), "{pixel:?}");
        compressed_pixels += usize::from(expected != pixel);
    }
    // Все, кроме первого (внутри порога), реально проходят колено — иначе масштаб не проверен.
    assert_eq!(compressed_pixels, pixels.len() - 1);
}

use super::*;
use crate::demosaic::{DemosaicMethod, demosaic};
use crate::image::mosaic_stage;

/// Детерминированный «случайный» f32 в [0, 1): линейный конгруэнтный генератор (Numerical Recipes).
fn pseudo_random_samples(count: usize, mut state: u32) -> Vec<f32> {
    (0..count)
        .map(|_| {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (state >> 8) as f32 / (1u32 << 24) as f32
        })
        .collect()
}

fn random_mosaic(pattern: BayerPattern, width: u32, height: u32) -> WhiteBalancedMosaic {
    let samples = pseudo_random_samples((width * height) as usize, 7);
    WhiteBalancedMosaic::from_samples(
        PixelSize::new(width, height),
        pattern,
        samples,
        mosaic_stage::WhiteBalanced,
    )
    .unwrap()
}

#[test]
fn preview_equals_the_direct_two_by_two_formula_for_all_layouts() {
    for pattern in [
        BayerPattern::RGGB,
        BayerPattern::GRBG,
        BayerPattern::GBRG,
        BayerPattern::BGGR,
    ] {
        let mosaic = random_mosaic(pattern, 12, 8);
        let image = demosaic(&mosaic, DemosaicMethod::Preview);
        assert_eq!((image.size().width, image.size().height), (6, 4));
        for y in 0..4u32 {
            for x in 0..6u32 {
                // Прямая формула: собираем цвета квадрата независимо от реализации.
                let (mut red, mut blue, mut greens) = (None, None, Vec::new());
                for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    let (px, py) = (2 * x + dx, 2 * y + dy);
                    let sample = mosaic.sample(px, py);
                    match pattern.channel_at(px, py) {
                        RgbChannel::Red => red = Some(sample),
                        RgbChannel::Blue => blue = Some(sample),
                        RgbChannel::Green => greens.push(sample),
                    }
                }
                let expected = [red.unwrap(), (greens[0] + greens[1]) / 2.0, blue.unwrap()];
                assert_eq!(image.pixel(x, y), expected, "{pattern:?} квадрат ({x}, {y})");
            }
        }
    }
}

#[test]
fn preview_of_a_flat_colour_keeps_the_colour() {
    let image = crate::test_patterns::flat_color(PixelSize::new(4, 4), [0.25, 0.5, 0.75]).unwrap();
    let mosaic = crate::test_patterns::mosaic_from_rgb(&image, BayerPattern::BGGR).unwrap();
    let preview = demosaic(&mosaic, DemosaicMethod::Preview);
    assert!(preview.pixels().iter().all(|pixel| *pixel == [0.25, 0.5, 0.75]));
}

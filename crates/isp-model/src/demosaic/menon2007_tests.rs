use super::*;
use crate::test_patterns::{flat_color, mosaic_from_rgb};

const ALL_PATTERNS: [BayerPattern; 4] = [
    BayerPattern::RGGB,
    BayerPattern::GRBG,
    BayerPattern::GBRG,
    BayerPattern::BGGR,
];

fn quality_of_flat(color: [f32; 3], width: u32, height: u32, pattern: BayerPattern) -> LinearCameraRgb {
    let mosaic = mosaic_from_rgb(&flat_color(PixelSize::new(width, height), color).unwrap(), pattern).unwrap();
    demosaic_menon2007(&mosaic)
}

#[test]
fn flat_gray_stays_exactly_gray_everywhere_including_borders() {
    for pattern in ALL_PATTERNS {
        let image = quality_of_flat([0.5; 3], 16, 12, pattern);
        assert!(image.pixels().iter().all(|pixel| *pixel == [0.5; 3]), "{pattern:?}");
    }
}

#[test]
fn flat_colour_stays_exactly_that_colour_everywhere_including_borders() {
    // Значения степени двойки: f32-арифметика алгоритма на них точна, поэтому сравнение строгое.
    for pattern in ALL_PATTERNS {
        let image = quality_of_flat([0.25, 0.5, 0.75], 16, 12, pattern);
        assert!(
            image.pixels().iter().all(|pixel| *pixel == [0.25, 0.5, 0.75]),
            "{pattern:?}"
        );
    }
}

#[test]
fn flat_colour_with_non_dyadic_values_is_preserved_within_rounding() {
    for pattern in ALL_PATTERNS {
        let image = quality_of_flat([0.2, 0.55, 0.9], 8, 8, pattern);
        for pixel in image.pixels() {
            for (got, want) in pixel.iter().zip([0.2, 0.55, 0.9]) {
                assert!((got - want).abs() < 1e-6, "{pattern:?}: {pixel:?}");
            }
        }
    }
}

#[test]
fn smallest_mosaic_does_not_panic_and_keeps_the_sampled_values() {
    // 2×2: окно мозаики короче ядра 5×5, всё держится на периодическом зеркале.
    let image = quality_of_flat([0.25, 0.5, 0.75], 2, 2, BayerPattern::RGGB);
    assert_eq!((image.size().width, image.size().height), (2, 2));
    assert!(image.pixels().iter().all(|pixel| *pixel == [0.25, 0.5, 0.75]));
}

#[test]
fn classifier_kernel_orientation_matches_the_paper_layout() {
    // Транспонирование — инволюция, а само ядро несимметрично (иначе проверка ориентации бессмысленна).
    assert_eq!(transposed(&transposed(&CLASSIFIER_KERNEL)), CLASSIFIER_KERNEL);
    assert_ne!(transposed(&CLASSIFIER_KERNEL), CLASSIFIER_KERNEL);
}

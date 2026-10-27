use camera_contract::capabilities::SensorLevels;
use camera_contract::geometry::PixelSize;

use super::{InvalidLevels, NormalizationLevels, normalize_levels};
use crate::cfa::BayerPattern;
use crate::image::RawMosaic;

/// Уровни OnePlus 11 (00 C3): чёрный 64 во всех позициях, белый 1023.
fn oneplus_levels() -> NormalizationLevels {
    NormalizationLevels::from_sensor_levels(&SensorLevels {
        black_level_pattern: [64; 4],
        white_level: 1023,
    })
    .unwrap()
}

#[test]
fn black_maps_to_zero_white_to_one_and_midpoint_linearly() {
    let raw = RawMosaic::new(PixelSize::new(2, 2), BayerPattern::BGGR, vec![64, 1023, 543, 300]).unwrap();
    let mosaic = normalize_levels(&raw, &oneplus_levels());
    assert_eq!(mosaic.sample(0, 0), 0.0);
    assert_eq!(mosaic.sample(1, 0), 1.0);
    // (543 − 64) / (1023 − 64) = 479 / 959.
    assert_eq!(mosaic.sample(0, 1), 479.0 / 959.0);
    assert_eq!(mosaic.sample(1, 1), 236.0 / 959.0);
}

#[test]
fn values_below_black_are_clipped_to_zero_not_negative() {
    let raw = RawMosaic::new(PixelSize::new(2, 2), BayerPattern::BGGR, vec![0, 63, 60, 64]).unwrap();
    let mosaic = normalize_levels(&raw, &oneplus_levels());
    assert!(
        mosaic.samples().iter().all(|&sample| sample == 0.0),
        "{:?}",
        mosaic.samples()
    );
}

#[test]
fn each_cell_uses_its_own_black_level() {
    let levels = NormalizationLevels::new([60.0, 62.0, 64.0, 66.0], 1000.0).unwrap();
    // Все пиксели 100: результат различается только чёрным уровнем позиции.
    let raw = RawMosaic::new(PixelSize::new(4, 2), BayerPattern::RGGB, vec![100; 8]).unwrap();
    let mosaic = normalize_levels(&raw, &levels);
    assert_eq!(mosaic.sample(0, 0), 40.0 / 940.0);
    assert_eq!(mosaic.sample(1, 0), 38.0 / 938.0);
    assert_eq!(mosaic.sample(2, 1), 36.0 / 936.0);
    assert_eq!(mosaic.sample(3, 1), 34.0 / 934.0);
}

#[test]
fn values_above_white_are_not_clipped_here() {
    // Обрезка пересвета — дело баланса белого: здесь значение > 1 сохраняется.
    let levels = NormalizationLevels::new([64.0; 4], 1000.0).unwrap();
    let raw = RawMosaic::new(PixelSize::new(2, 2), BayerPattern::BGGR, vec![1023; 4]).unwrap();
    let mosaic = normalize_levels(&raw, &levels);
    assert!(mosaic.sample(0, 0) > 1.0);
}

#[test]
fn normalization_keeps_size_and_pattern() {
    let raw = RawMosaic::new(PixelSize::new(6, 4), BayerPattern::GRBG, vec![500; 24]).unwrap();
    let mosaic = normalize_levels(&raw, &oneplus_levels());
    assert_eq!(mosaic.size(), PixelSize::new(6, 4));
    assert_eq!(mosaic.pattern(), BayerPattern::GRBG);
}

#[test]
fn white_not_above_black_is_rejected() {
    assert_eq!(
        NormalizationLevels::new([64.0, 64.0, 1023.0, 64.0], 1023.0),
        Err(InvalidLevels::WhiteNotAboveBlack {
            cell: 2,
            black: 1023.0,
            white: 1023.0
        })
    );
    assert!(matches!(
        NormalizationLevels::new([f32::NAN, 0.0, 0.0, 0.0], 1023.0),
        Err(InvalidLevels::NotFiniteOrNegative { .. })
    ));
}

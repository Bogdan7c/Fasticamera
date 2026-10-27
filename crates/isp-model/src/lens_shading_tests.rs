use camera_contract::capabilities::LensShading;
use camera_contract::geometry::PixelSize;

use super::apply_lens_shading;
use super::{InvalidGainMap, LensShadingGainMap, LensShadingOutcome, LensShadingSkipReason, LensShadingStep};
use crate::cfa::BayerPattern;
use crate::image::{CameraMosaic, mosaic_stage};

fn flat_mosaic(width: u32, height: u32, value: f32) -> CameraMosaic {
    let size = PixelSize::new(width, height);
    CameraMosaic::from_samples(
        size,
        BayerPattern::BGGR,
        vec![value; size.pixel_count() as usize],
        mosaic_stage::Normalized,
    )
    .unwrap()
}

/// Карта 2×2: в левом верхнем углу усиление `corner`, в остальных точках 1, во всех каналах.
fn corner_map(corner: f32) -> LensShadingGainMap {
    LensShadingGainMap::new(2, 2, vec![[corner; 4], [1.0; 4], [1.0; 4], [1.0; 4]]).unwrap()
}

#[test]
fn vendor_applied_correction_is_never_applied_twice() {
    let step = LensShadingStep::decide(LensShading::AppliedByVendor, Some(corner_map(4.0)));
    assert_eq!(step, LensShadingStep::Skip(LensShadingSkipReason::VendorAlreadyApplied));
    let mosaic = flat_mosaic(4, 4, 0.25);
    let (output, outcome) = apply_lens_shading(&mosaic, &step);
    assert_eq!(
        outcome,
        LensShadingOutcome::Skipped(LensShadingSkipReason::VendorAlreadyApplied)
    );
    assert_eq!(output.samples(), mosaic.samples(), "пиксели не должны меняться");
}

#[test]
fn missing_map_is_reported_separately_from_vendor_correction() {
    assert_eq!(
        LensShadingStep::decide(LensShading::NotApplied, None),
        LensShadingStep::Skip(LensShadingSkipReason::NoGainMap)
    );
    assert_eq!(
        LensShadingStep::decide(LensShading::NotReported, None),
        LensShadingStep::Skip(LensShadingSkipReason::NoGainMap)
    );
}

#[test]
fn map_gains_are_exact_at_corners_and_bilinear_between() {
    let step = LensShadingStep::decide(LensShading::NotApplied, Some(corner_map(3.0)));
    // Кадр 6×4: угол (0,0) совпадает с точкой карты, (5,3) — с противоположной.
    let (output, outcome) = apply_lens_shading(&flat_mosaic(6, 4, 0.1), &step);
    assert_eq!(outcome, LensShadingOutcome::Applied);
    assert!((output.sample(0, 0) - 0.3).abs() < 1e-6, "угол ×3");
    assert!((output.sample(5, 3) - 0.1).abs() < 1e-6, "противоположный угол ×1");
    // x = 5/5·1 = 1 по сетке, y = 0: верхний правый угол ×1.
    assert!((output.sample(5, 0) - 0.1).abs() < 1e-6);
    // Середина верхней кромки x = 2,5 → нет такого пикселя; берём x = 1: доля 0,2 → 3·0,8 + 1·0,2 = 2,6.
    assert!((output.sample(1, 0) - 0.26).abs() < 1e-6, "{}", output.sample(1, 0));
}

#[test]
fn green_rows_use_their_own_map_channel() {
    // Канал G_even (индекс 1) ×2, G_odd (индекс 2) ×5, R и B ×1, по всей карте.
    let map = LensShadingGainMap::new(2, 2, vec![[1.0, 2.0, 5.0, 1.0]; 4]).unwrap();
    let step = LensShadingStep::decide(LensShading::NotApplied, Some(map));
    let (output, _) = apply_lens_shading(&flat_mosaic(2, 2, 0.1), &step);
    // BGGR: (1,0) — зелёный чётной строки, (0,1) — зелёный нечётной, (0,0) синий, (1,1) красный.
    assert!((output.sample(1, 0) - 0.2).abs() < 1e-6);
    assert!((output.sample(0, 1) - 0.5).abs() < 1e-6);
    assert!((output.sample(0, 0) - 0.1).abs() < 1e-6);
    assert!((output.sample(1, 1) - 0.1).abs() < 1e-6);
}

#[test]
fn malformed_maps_are_rejected() {
    assert_eq!(
        LensShadingGainMap::new(1, 5, vec![[1.0; 4]; 5]),
        Err(InvalidGainMap::GridTooSmall { columns: 1, rows: 5 })
    );
    assert_eq!(
        LensShadingGainMap::new(2, 2, vec![[1.0; 4]; 3]),
        Err(InvalidGainMap::PointCountMismatch {
            columns: 2,
            rows: 2,
            expected: 4,
            actual: 3
        })
    );
    assert_eq!(
        LensShadingGainMap::new(2, 2, vec![[1.0; 4], [1.0; 4], [1.0, 0.0, 1.0, 1.0], [1.0; 4]]),
        Err(InvalidGainMap::InvalidGain { point: 2, gain: 0.0 })
    );
}

#[test]
fn map_channels_follow_camera2_order_not_cfa_position() {
    // Camera2 (NdkCameraMetadataTags.h, ACAMERA_STATISTICS_LENS_SHADING_MAP): порядок каналов
    // всегда [R, G_even, G_odd, B], какой бы ни была раскладка CFA. Все четыре усиления разные,
    // чтобы перестановка R и B (или зелёных) была видна.
    let map = LensShadingGainMap::new(2, 2, vec![[3.0, 5.0, 7.0, 11.0]; 4]).unwrap();
    for pattern in [
        BayerPattern::BGGR,
        BayerPattern::GRBG,
        BayerPattern::RGGB,
        BayerPattern::GBRG,
    ] {
        let mosaic =
            CameraMosaic::from_samples(PixelSize::new(2, 2), pattern, vec![1.0; 4], mosaic_stage::Normalized).unwrap();
        let step = LensShadingStep::decide(LensShading::NotApplied, Some(map.clone()));
        let (output, _) = apply_lens_shading(&mosaic, &step);
        for (x, y) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
            let expected = match pattern.channel_at(x, y) {
                crate::cfa::RgbChannel::Red => 3.0,
                crate::cfa::RgbChannel::Green if y == 0 => 5.0,
                crate::cfa::RgbChannel::Green => 7.0,
                crate::cfa::RgbChannel::Blue => 11.0,
            };
            assert_eq!(output.sample(x, y), expected, "{pattern:?} ({x}, {y})");
        }
    }
}

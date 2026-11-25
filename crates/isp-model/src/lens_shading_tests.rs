use std::sync::Arc;

use camera_contract::geometry::{PixelRect, PixelSize};
use camera_contract::lens_shading::{InvalidGainGrid, ValidAreaOnSensor};

use super::{
    CorrectionStrength, CropInValidArea, InvalidCorrectionStrength, InvalidCrop, LensShadingControl,
    LensShadingGainMap, LensShadingOutcome, LensShadingSampling, LensShadingSkipReason, LensShadingStep,
    StreamMapError, apply_lens_shading, apply_lens_shading_at, apply_lens_shading_sampled, apply_lens_shading_with,
};
use crate::cfa::{BayerPattern, RgbChannel};
use crate::image::{CameraMosaic, mosaic_stage};
use crate::test_support::{MAIN_ACTIVE_ARRAY, asymmetric_calibration, corner_field_gain};

const FULL: CorrectionStrength = CorrectionStrength::FULL;

fn strength(value: f64) -> CorrectionStrength {
    CorrectionStrength::new(value).unwrap()
}

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
fn corner_map(corner: f32) -> Arc<LensShadingGainMap> {
    Arc::new(LensShadingGainMap::new(2, 2, vec![[corner; 4], [1.0; 4], [1.0; 4], [1.0; 4]]).unwrap())
}

fn enabled_step(map: Arc<LensShadingGainMap>) -> LensShadingStep {
    LensShadingStep::decide(Some(&map), LensShadingControl::Enabled(FULL))
}

fn whole_sensor() -> ValidAreaOnSensor {
    ValidAreaOnSensor::whole_active_array(MAIN_ACTIVE_ARRAY)
}

/// Центральная полоса 16:9 основной камеры: строки 384..2687 при 4096×3072.
fn widescreen_band() -> ValidAreaOnSensor {
    ValidAreaOnSensor::new(
        MAIN_ACTIVE_ARRAY,
        PixelRect {
            left: 0,
            top: 384,
            size: PixelSize::new(4096, 2304),
        },
    )
    .unwrap()
}

fn gain(map: &LensShadingGainMap, column: u32, row: u32, channel: usize) -> f32 {
    map.points()[(row * map.columns() + column) as usize][channel]
}

// ---- решение стадии ----

#[test]
fn decide_covers_the_whole_matrix_of_calibration_and_control() {
    let map = corner_map(3.0);
    // Есть калибровка и коррекция включена: карта общая с потоком (тот же Arc, без копии).
    let LensShadingStep::Apply(applied) = LensShadingStep::decide(Some(&map), LensShadingControl::Enabled(FULL)) else {
        panic!("ожидали Apply");
    };
    assert!(Arc::ptr_eq(&applied, &map));
    // Выключено управлением — приоритетнее отсутствия калибровки.
    for stream_map in [Some(&map), None] {
        assert_eq!(
            LensShadingStep::decide(stream_map, LensShadingControl::Disabled),
            LensShadingStep::Skip(LensShadingSkipReason::DisabledByControl)
        );
    }
    // Калибровки нет: честная причина.
    assert_eq!(
        LensShadingStep::decide(None, LensShadingControl::Enabled(FULL)),
        LensShadingStep::Skip(LensShadingSkipReason::NotCalibrated)
    );
}

#[test]
fn partial_strength_builds_a_weakened_map_and_leaves_the_stream_map_untouched() {
    let map = corner_map(3.0);
    let before = (*map).clone();
    let LensShadingStep::Apply(weakened) =
        LensShadingStep::decide(Some(&map), LensShadingControl::Enabled(strength(0.5)))
    else {
        panic!("ожидали Apply");
    };
    assert!(!Arc::ptr_eq(&weakened, &map));
    assert_eq!(gain(&weakened, 0, 0, 0), 2.0, "1 + (3 − 1)·0,5");
    assert_eq!(gain(&weakened, 1, 1, 0), 1.0);
    assert_eq!(*map, before, "карта потока не меняется");
}

#[test]
fn correction_strength_accepts_only_unit_interval() {
    assert_eq!(CorrectionStrength::new(0.0).map(CorrectionStrength::value), Ok(0.0));
    assert_eq!(CorrectionStrength::new(1.0), Ok(FULL));
    for requested in [-0.01, 1.01, f64::NAN, f64::INFINITY] {
        let error = CorrectionStrength::new(requested).unwrap_err();
        assert_eq!(error.requested.is_nan(), requested.is_nan());
        let _: InvalidCorrectionStrength = error;
    }
    assert_eq!(LensShadingControl::default(), LensShadingControl::Enabled(FULL));
}

// ---- применение к пикселям ----

#[test]
fn skipped_stage_leaves_pixels_untouched_and_reports_the_reason() {
    let mosaic = flat_mosaic(4, 4, 0.25);
    for reason in [
        LensShadingSkipReason::NotCalibrated,
        LensShadingSkipReason::DisabledByControl,
    ] {
        let (output, outcome) = apply_lens_shading(&mosaic, &LensShadingStep::Skip(reason));
        assert_eq!(outcome, LensShadingOutcome::Skipped(reason));
        assert_eq!(output.samples(), mosaic.samples(), "пиксели не должны меняться");
    }
}

#[test]
fn map_gains_are_exact_at_corners_and_bilinear_between() {
    let step = enabled_step(corner_map(3.0));
    // Кадр 6×4: угол (0,0) совпадает с точкой карты, (5,3) — с противоположной.
    let (output, outcome) = apply_lens_shading(&flat_mosaic(6, 4, 0.1), &step);
    assert_eq!(outcome, LensShadingOutcome::Applied);
    assert!((output.sample(0, 0) - 0.3).abs() < 1e-6, "угол ×3");
    assert!((output.sample(5, 3) - 0.1).abs() < 1e-6, "противоположный угол ×1");
    assert!((output.sample(5, 0) - 0.1).abs() < 1e-6, "верхний правый угол ×1");
    // x = 1 из 5: доля 0,2 → 3·0,8 + 1·0,2 = 2,6.
    assert!((output.sample(1, 0) - 0.26).abs() < 1e-6, "{}", output.sample(1, 0));
}

#[test]
fn green_rows_use_their_own_map_channel() {
    // Канал G_even (индекс 1) ×2, G_odd (индекс 2) ×5, R и B ×1, по всей карте.
    let map = Arc::new(LensShadingGainMap::new(2, 2, vec![[1.0, 2.0, 5.0, 1.0]; 4]).unwrap());
    let (output, _) = apply_lens_shading(&flat_mosaic(2, 2, 0.1), &enabled_step(map));
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
        Err(InvalidGainGrid::GridTooSmall { columns: 1, rows: 5 })
    );
    assert_eq!(
        LensShadingGainMap::new(2, 2, vec![[1.0; 4], [1.0; 4], [1.0, 0.5, 1.0, 1.0], [1.0; 4]]),
        Err(InvalidGainGrid::InvalidGain { point: 2, gain: 0.5 })
    );
}

#[test]
fn map_channels_follow_camera2_order_not_cfa_position() {
    // Порядок каналов всегда [R, G_even, G_odd, B], какой бы ни была раскладка CFA. Все четыре
    // усиления разные, чтобы перестановка R и B (или зелёных) была видна.
    let map = Arc::new(LensShadingGainMap::new(2, 2, vec![[3.0, 5.0, 7.0, 11.0]; 4]).unwrap());
    for pattern in [
        BayerPattern::BGGR,
        BayerPattern::GRBG,
        BayerPattern::RGGB,
        BayerPattern::GBRG,
    ] {
        let mosaic =
            CameraMosaic::from_samples(PixelSize::new(2, 2), pattern, vec![1.0; 4], mosaic_stage::Normalized).unwrap();
        let (output, _) = apply_lens_shading(&mosaic, &enabled_step(Arc::clone(&map)));
        for (x, y) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
            let expected = match pattern.channel_at(x, y) {
                RgbChannel::Red => 3.0,
                RgbChannel::Green if y == 0 => 5.0,
                RgbChannel::Green => 7.0,
                RgbChannel::Blue => 11.0,
            };
            assert_eq!(output.sample(x, y), expected, "{pattern:?} ({x}, {y})");
        }
    }
}

// ---- карта потока из калибровки ----

#[test]
fn identity_stream_map_equals_the_calibration_exactly() {
    let calibration = asymmetric_calibration(33, 25);
    let map = LensShadingGainMap::for_stream(&calibration, &whole_sensor(), MAIN_ACTIVE_ARRAY, FULL).unwrap();
    assert_eq!((map.columns(), map.rows()), (33, 25));
    assert_eq!(map.points(), calibration.grid.points(), "4:3 на весь сенсор: бит в бит");
}

#[test]
fn widescreen_map_starts_at_the_calibration_value_of_the_band_top_left() {
    let calibration = asymmetric_calibration(33, 25);
    let valid = PixelSize::new(4096, 2304);
    let map = LensShadingGainMap::for_stream(&calibration, &widescreen_band(), valid, FULL).unwrap();
    // Левый верхний узел полосы — пиксель активной области (0, 384); нижний правый — (4095, 384 + 2303).
    for channel in 0..4 {
        let top_left = corner_field_gain(channel, 0.0, 384.0, MAIN_ACTIVE_ARRAY);
        let bottom_right = corner_field_gain(channel, 4095.0, 384.0 + 2303.0, MAIN_ACTIVE_ARRAY);
        let top_right = corner_field_gain(channel, 4095.0, 384.0, MAIN_ACTIVE_ARRAY);
        let bottom_left = corner_field_gain(channel, 0.0, 384.0 + 2303.0, MAIN_ACTIVE_ARRAY);
        let near = |actual: f32, expected: f64, label: &str| {
            assert!(
                (f64::from(actual) - expected).abs() < 2e-5 * expected,
                "канал {channel} {label}: {actual} против {expected}"
            );
        };
        near(gain(&map, 0, 0, channel), top_left, "верх-лево");
        near(gain(&map, 32, 0, channel), top_right, "верх-право");
        near(gain(&map, 0, 24, channel), bottom_left, "низ-лево");
        near(gain(&map, 32, 24, channel), bottom_right, "низ-право");
    }
    // Полоса вырезана из середины: верхний ряд светлее угла кадра 4:3 по вертикали (поле растёт вниз).
    assert!(gain(&map, 0, 0, 0) < calibration.grid.points()[0][0] * 1.0 + 1.0);
    assert_ne!(
        gain(&map, 0, 0, 0),
        calibration.grid.points()[0][0],
        "полоса не равна полному кадру"
    );
}

#[test]
fn binned_buffer_maps_pixel_centres_onto_the_active_array() {
    let calibration = asymmetric_calibration(33, 25);
    // Буфер 2048×1536 — биннинг 2×2 всего сенсора: центр пикселя i буфера лежит между
    // пикселями 2i и 2i + 1 активной области, то есть в 2·i + 0,5.
    let valid = PixelSize::new(2048, 1536);
    let map = LensShadingGainMap::for_stream(&calibration, &whole_sensor(), valid, FULL).unwrap();
    for channel in 0..4 {
        let expected = corner_field_gain(channel, 0.5, 0.5, MAIN_ACTIVE_ARRAY);
        assert!((f64::from(gain(&map, 0, 0, channel)) - expected).abs() < 2e-5 * expected);
        let expected = corner_field_gain(channel, 2.0 * 2047.0 + 0.5, 2.0 * 1535.0 + 0.5, MAIN_ACTIVE_ARRAY);
        assert!((f64::from(gain(&map, 32, 24, channel)) - expected).abs() < 2e-5 * expected);
    }
}

#[test]
fn stream_map_strength_zero_is_flat_and_half_is_midway() {
    let calibration = asymmetric_calibration(33, 25);
    let full = LensShadingGainMap::for_stream(&calibration, &whole_sensor(), MAIN_ACTIVE_ARRAY, FULL).unwrap();
    let off = LensShadingGainMap::for_stream(&calibration, &whole_sensor(), MAIN_ACTIVE_ARRAY, strength(0.0)).unwrap();
    let half = LensShadingGainMap::for_stream(&calibration, &whole_sensor(), MAIN_ACTIVE_ARRAY, strength(0.5)).unwrap();
    assert!(
        off.points().iter().all(|gains| *gains == [1.0; 4]),
        "сила 0 — все усиления 1"
    );
    for ((half_gains, full_gains), index) in half.points().iter().zip(full.points()).zip(0..) {
        for channel in 0..4 {
            let expected = 1.0 + (full_gains[channel] - 1.0) * 0.5;
            assert_eq!(half_gains[channel], expected, "точка {index} канал {channel}");
        }
    }
}

#[test]
fn stream_map_rejects_impossible_geometry() {
    let calibration = asymmetric_calibration(33, 25);
    let odd_top = ValidAreaOnSensor::new(
        MAIN_ACTIVE_ARRAY,
        PixelRect {
            left: 0,
            top: 385,
            size: PixelSize::new(4096, 2304),
        },
    )
    .unwrap();
    assert_eq!(
        LensShadingGainMap::for_stream(&calibration, &odd_top, PixelSize::new(4096, 2304), FULL),
        Err(StreamMapError::OddSensorTop { top: 385 })
    );
    let other_array = ValidAreaOnSensor::whole_active_array(PixelSize::new(4000, 3000));
    assert_eq!(
        LensShadingGainMap::for_stream(&calibration, &other_array, PixelSize::new(4000, 3000), FULL),
        Err(StreamMapError::ActiveArrayMismatch {
            calibration: MAIN_ACTIVE_ARRAY,
            stream: PixelSize::new(4000, 3000)
        })
    );
    assert_eq!(
        LensShadingGainMap::for_stream(&calibration, &whole_sensor(), PixelSize::new(1, 100), FULL),
        Err(StreamMapError::ValidAreaTooSmall {
            size: PixelSize::new(1, 100)
        })
    );
}

// ---- вырезки ----

fn ramp_mosaic(width: u32, height: u32) -> CameraMosaic {
    let samples = (0..width * height)
        .map(|index| 0.05 + (index % 97) as f32 * 0.009)
        .collect();
    CameraMosaic::from_samples(
        PixelSize::new(width, height),
        BayerPattern::GRBG,
        samples,
        mosaic_stage::Normalized,
    )
    .unwrap()
}

fn crop_of(mosaic: &CameraMosaic, left: u32, top: u32, width: u32, height: u32) -> CameraMosaic {
    let mut samples = Vec::new();
    for y in top..top + height {
        for x in left..left + width {
            samples.push(mosaic.sample(x, y));
        }
    }
    // Начало вырезки чётное — раскладка CFA вырезки та же.
    CameraMosaic::from_samples(
        PixelSize::new(width, height),
        mosaic.pattern(),
        samples,
        mosaic_stage::Normalized,
    )
    .unwrap()
}

#[test]
fn crop_application_is_bit_identical_to_the_matching_part_of_the_whole_frame() {
    let calibration = asymmetric_calibration(33, 25);
    let valid = PixelSize::new(64, 36);
    let map = LensShadingGainMap::for_stream(&calibration, &widescreen_band(), valid, FULL).unwrap();
    let step = enabled_step(Arc::new(map));
    let whole = ramp_mosaic(valid.width, valid.height);
    let (full_output, _) = apply_lens_shading(&whole, &step);
    for (left, top, width, height) in [(0, 0, 64, 36), (6, 4, 20, 10), (40, 30, 24, 6), (2, 0, 8, 36)] {
        let crop = crop_of(&whole, left, top, width, height);
        let placement = CropInValidArea::at(left, top, valid).unwrap();
        let (crop_output, outcome) = apply_lens_shading_at(&crop, &step, placement).unwrap();
        assert_eq!(outcome, LensShadingOutcome::Applied);
        for y in 0..height {
            for x in 0..width {
                assert_eq!(
                    crop_output.sample(x, y).to_bits(),
                    full_output.sample(left + x, top + y).to_bits(),
                    "вырезка ({left}, {top}) пиксель ({x}, {y})"
                );
            }
        }
    }
    // Вырезка действительно отличается от применения с нулевым началом (проверка, что смещение учтено).
    let crop = crop_of(&whole, 40, 30, 24, 6);
    let (shifted, _) = apply_lens_shading_at(&crop, &step, CropInValidArea::at(40, 30, valid).unwrap()).unwrap();
    let (unshifted, _) = apply_lens_shading(&crop, &step);
    assert_ne!(shifted.samples(), unshifted.samples());
}

#[test]
fn crop_placement_rejects_odd_origin_and_overflow() {
    let valid = PixelSize::new(64, 36);
    assert_eq!(
        CropInValidArea::at(3, 4, valid),
        Err(InvalidCrop::OddOrigin { left: 3, top: 4 })
    );
    let step = enabled_step(corner_map(2.0));
    let crop = ramp_mosaic(20, 10);
    let placement = CropInValidArea::at(50, 0, valid).unwrap();
    assert_eq!(
        apply_lens_shading_at(&crop, &step, placement),
        Err(InvalidCrop::OutsideValidArea {
            left: 50,
            top: 0,
            size: PixelSize::new(20, 10),
            valid_area_size: valid
        })
    );
}

#[test]
fn last_pixel_hits_the_last_grid_node_exactly_for_any_extent_and_node_count() {
    // Масштаб (узлов − 1)/(пикселей − 1) в f32 не обязан давать ровно «узлов − 1» на последнем пикселе
    // (на 1 ULP в любую сторону), поэтому последний пиксель обрабатывается явно: усиление — ровно значение
    // последнего узла. Узел `c` несёт усиление 1 + c во всех каналах, значит последний даёт `nodes`.
    for width in (2..=200).step_by(2) {
        for nodes in 2..=17_u32 {
            let points = (0..2 * nodes).map(|index| [1.0 + (index % nodes) as f32; 4]).collect();
            let map = Arc::new(LensShadingGainMap::new(nodes, 2, points).unwrap());
            let (output, _) = apply_lens_shading(&flat_mosaic(width, 2, 1.0), &enabled_step(map));
            assert_eq!(
                output.sample(width - 1, 0),
                nodes as f32,
                "ширина {width}, узлов {nodes}"
            );
            assert_eq!(output.sample(0, 0), 1.0, "первый пиксель — первый узел");
        }
    }
}

#[test]
fn grid_scales_are_node_spans_per_pixel_span_in_f32() {
    let map = LensShadingGainMap::new(5, 3, vec![[1.0; 4]; 15]).unwrap();
    let scales = map.grid_scales(PixelSize::new(101, 41));
    assert_eq!(scales.x, 4.0_f32 / 100.0_f32);
    assert_eq!(scales.y, 2.0_f32 / 40.0_f32);
}

// ---- выборка по ячейке (видоискатель) ----

#[test]
fn per_cell_gain_is_taken_at_the_cell_centre_and_shared_by_the_four_samples() {
    let step = enabled_step(corner_map(3.0));
    // Кадр 6×4: масштабы 1/5 и 1/3. Ячейка (0,0): центр (0,5; 0,5) → grid (0,1; 1/6).
    let (output, _) = apply_lens_shading_with(&flat_mosaic(6, 4, 0.1), &step, LensShadingSampling::PerCellCentre);
    // Усиление угла 3 весом (1 − 0,1)(1 − 1/6) = 0,75, остальные точки 1: g = 1 + 2·0,75 = 2,5.
    let expected = 0.1 * 2.5;
    for (x, y) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
        assert!(
            (output.sample(x, y) - expected).abs() < 1e-6,
            "({x}, {y}): {}",
            output.sample(x, y)
        );
    }
    // Последняя ячейка (4, 2): центр (4,5; 2,5) → grid (0,9; 5/6): вес угла 3 — 0,1 · (1/6) → g ≈ 1,0333.
    let last = 0.1 * (1.0 + 2.0 * 0.1 / 6.0);
    for (x, y) in [(4, 2), (5, 2), (4, 3), (5, 3)] {
        assert!(
            (output.sample(x, y) - last).abs() < 1e-6,
            "({x}, {y}): {}",
            output.sample(x, y)
        );
    }
    // Покадровая выборка в тех же пикселях различает отсчёты ячейки.
    let (per_sample, _) = apply_lens_shading_with(&flat_mosaic(6, 4, 0.1), &step, LensShadingSampling::PerSample);
    assert_ne!(per_sample.sample(0, 0), per_sample.sample(1, 1));
}

#[test]
fn per_cell_sampling_keeps_each_samples_own_map_channel_for_every_cfa_position() {
    // R ×1,5, G_even ×2, G_odd ×5, B ×3 по всей карте: усиление не зависит от координаты, видно только канал.
    let map = Arc::new(LensShadingGainMap::new(2, 2, vec![[1.5, 2.0, 5.0, 3.0]; 4]).unwrap());
    let (output, _) = apply_lens_shading_with(
        &flat_mosaic(4, 4, 0.1),
        &enabled_step(map),
        LensShadingSampling::PerCellCentre,
    );
    // BGGR: (0,0) синий, (1,0) зелёный чётной строки, (0,1) зелёный нечётной, (1,1) красный — в обеих ячейках по высоте.
    for cell_y in [0_u32, 2] {
        for cell_x in [0_u32, 2] {
            let at = |dx, dy| output.sample(cell_x + dx, cell_y + dy);
            assert!((at(0, 0) - 0.3).abs() < 1e-6);
            assert!((at(1, 0) - 0.2).abs() < 1e-6);
            assert!((at(0, 1) - 0.5).abs() < 1e-6);
            assert!((at(1, 1) - 0.15).abs() < 1e-6);
        }
    }
}

#[test]
fn per_cell_crop_application_is_bit_identical_to_the_matching_part_of_the_whole_frame() {
    let map = Arc::new(
        LensShadingGainMap::new(
            4,
            3,
            (0..12)
                .map(|index| [1.0 + 0.1 * index as f32, 1.2, 1.4 + 0.05 * index as f32, 1.9])
                .collect(),
        )
        .unwrap(),
    );
    let step = enabled_step(map);
    let whole = ramp_mosaic(40, 30);
    let (full_output, _) = apply_lens_shading_with(&whole, &step, LensShadingSampling::PerCellCentre);
    let crop = crop_of(&whole, 10, 6, 20, 16);
    let valid = PixelSize::new(40, 30);
    let placement = CropInValidArea::at(10, 6, valid).unwrap();
    let (crop_output, _) =
        apply_lens_shading_sampled(&crop, &step, placement, LensShadingSampling::PerCellCentre).unwrap();
    for y in 0..16 {
        for x in 0..20 {
            assert_eq!(
                crop_output.sample(x, y),
                full_output.sample(10 + x, 6 + y),
                "({x}, {y})"
            );
        }
    }
}

#[test]
fn preview_reference_uses_cell_centres_and_quality_uses_samples() {
    use crate::demosaic::DemosaicMethod;
    assert_eq!(
        LensShadingSampling::for_demosaic(DemosaicMethod::Preview),
        LensShadingSampling::PerCellCentre
    );
    assert_eq!(
        LensShadingSampling::for_demosaic(DemosaicMethod::Quality),
        LensShadingSampling::PerSample
    );
}

#[test]
fn gain_for_sample_is_exactly_the_factor_the_stage_applies_to_that_sample() {
    // Карта с разными усилениями по каналам и углам: перепутанный канал, строка или точка выборки видны сразу.
    let map = Arc::new(
        LensShadingGainMap::new(
            2,
            2,
            vec![
                [4.0, 3.5, 3.25, 3.0],
                [2.5, 2.25, 2.0, 1.75],
                [2.75, 2.375, 2.125, 1.875],
                [1.0, 1.125, 1.25, 1.5],
            ],
        )
        .unwrap(),
    );
    let step = enabled_step(map);
    let size = PixelSize::new(8, 6);
    let mosaic = flat_mosaic(8, 6, 1.0);
    for sampling in [LensShadingSampling::PerSample, LensShadingSampling::PerCellCentre] {
        let (corrected, _) = apply_lens_shading_with(&mosaic, &step, sampling);
        for y in 0..size.height {
            for x in 0..size.width {
                let channel = mosaic.pattern().channel_at(x, y);
                let gain = step.gain_for_sample((x, y), size, channel, sampling);
                assert_eq!(gain, corrected.sample(x, y), "({x}, {y}) {sampling:?}");
            }
        }
    }
}

#[test]
fn gain_for_sample_of_a_skipped_stage_is_one() {
    let step = LensShadingStep::Skip(LensShadingSkipReason::DisabledByControl);
    let gain = step.gain_for_sample(
        (3, 2),
        PixelSize::new(8, 6),
        RgbChannel::Green,
        LensShadingSampling::PerCellCentre,
    );
    assert_eq!(gain, 1.0);
}

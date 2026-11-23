//! Положение сетки статистики: 4:3 и 16:9 основной камеры, зум, и главное — что тап в нужной точке
//! сцены даёт вес именно тем плиткам, под которыми эта точка лежит.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "в тестах падение — сигнал провала"
)]

use camera_contract::capabilities::{LensShading, SensorLevels, VendorProcessingFlags};
use camera_contract::fixtures::camera_id;
use camera_contract::geometry::SensorOrientation;
use camera_contract::raw_format::{CfaLayout, MosaicKind, MosaicPattern, RawFormat};
use camera_contract::stream::{FrameRateMechanism, TargetFrameRate};
use camera_contract::three_a::{MeteringRegion, SensorPoint};
use isp_model::statistics::TileGrid;
use scene_estimation::exposure::{MeteringArea, MeteringConfig, MeteringWeights};

use super::*;

const BUFFER: PixelSize = PixelSize::new(4096, 3072);
const GRID: TileGrid = TileGrid { columns: 32, rows: 24 };

fn passport(readout: ReadoutAspect, valid_height: u32) -> StreamPassport {
    StreamPassport {
        camera: camera_id("0"),
        raw_format: RawFormat::Raw16,
        buffer_size: BUFFER,
        readout,
        valid_area: PixelRect {
            left: 0,
            top: 0,
            size: PixelSize::new(BUFFER.width, valid_height),
        },
        sensor_area: camera_contract::lens_shading::ValidAreaOnSensor::whole_active_array(BUFFER),
        lens_shading: camera_contract::lens_shading::LensShadingCalibrationState::NotCalibrated(
            camera_contract::lens_shading::NotCalibratedReason::NoCalibrationForCamera,
        ),
        mosaic: MosaicPattern {
            kind: MosaicKind::Bayer,
            layout: CfaLayout::Rggb,
        },
        static_levels: SensorLevels {
            black_level_pattern: [64; 4],
            white_level: 1023,
        },
        vendor_processing: VendorProcessingFlags {
            lens_shading: LensShading::AppliedByVendor,
        },
        sensor_orientation: SensorOrientation::Degrees90,
        frame_rate: TargetFrameRate::new(30).unwrap(),
        frame_rate_mechanism: FrameRateMechanism::HalDefault,
        color_calibration: Vec::new(),
    }
}

fn full_crop(valid_height: u32) -> PixelRect {
    PixelRect {
        left: 0,
        top: 0,
        size: PixelSize::new(BUFFER.width, valid_height),
    }
}

fn spot(x: f32, y: f32) -> MeteringArea {
    MeteringArea::Spot(MeteringRegion::new(SensorPoint::new(x, y).unwrap(), 0.01).unwrap())
}

/// Индекс плитки с наибольшим весом.
fn heaviest_tile(weights: &MeteringWeights) -> (u32, u32) {
    let (index, _) = weights
        .weights()
        .iter()
        .enumerate()
        .max_by(|left, right| left.1.total_cmp(right.1))
        .unwrap();
    let index = u32::try_from(index).unwrap();
    (index % GRID.columns, index / GRID.columns)
}

#[test]
fn four_by_three_grid_covers_the_whole_active_area() {
    let placement = tile_placement(&passport(ReadoutAspect::FourByThree, 3072), full_crop(3072)).unwrap();
    let expected = TilePlacement::new(0.0, 0.0, 1.0, 1.0, 4096.0 / 3072.0).unwrap();
    assert_eq!(placement, expected);
}

#[test]
fn sixteen_by_nine_grid_is_the_central_band_of_the_active_area() {
    // Строки 0–2303 буфера = строки 384–2687 активной области = доли 0,125…0,875.
    let placement = tile_placement(&passport(ReadoutAspect::SixteenByNine, 2304), full_crop(2304)).unwrap();
    let expected = TilePlacement::new(0.0, 0.125, 1.0, 0.875, 4096.0 / 3072.0).unwrap();
    assert_eq!(placement, expected);
}

#[test]
fn a_tap_lands_on_the_tile_under_it_in_both_readout_modes() {
    let config = MeteringConfig::default();
    // 4:3: центр плитки (столбец 8, строка 12) лежит в (8,5/32; 12,5/24) активной области.
    let four = tile_placement(&passport(ReadoutAspect::FourByThree, 3072), full_crop(3072)).unwrap();
    let weights = MeteringWeights::build(GRID, four, spot(8.5 / 32.0, 12.5 / 24.0), &config).unwrap();
    assert_eq!(heaviest_tile(&weights), (8, 12));
    // 16:9: центр строки 4 сетки лежит на 0,125 + 0,75 · 4,5/24 = 0,265625 высоты активной области.
    // Без перевода осей (по высоте буфера) та же точка попала бы в строку 6.
    let sixteen = tile_placement(&passport(ReadoutAspect::SixteenByNine, 2304), full_crop(2304)).unwrap();
    let weights = MeteringWeights::build(GRID, sixteen, spot(8.5 / 32.0, 0.265_625), &config).unwrap();
    assert_eq!(heaviest_tile(&weights), (8, 4));
}

#[test]
fn a_tap_outside_the_sixteen_by_nine_band_is_reported_not_guessed() {
    // Верхние 12 % активной области в режиме 16:9 сенсор не читает: регион туда не попадает в сетку.
    let placement = tile_placement(&passport(ReadoutAspect::SixteenByNine, 2304), full_crop(2304)).unwrap();
    let region = MeteringRegion::new(SensorPoint::new(0.5, 0.02).unwrap(), 0.02).unwrap();
    let result = MeteringWeights::build(GRID, placement, MeteringArea::Spot(region), &MeteringConfig::default());
    assert_eq!(result, Err(MeteringWeightsError::RegionOutsideGrid));
}

#[test]
fn a_zoomed_crop_shifts_the_grid_so_the_tap_still_hits_the_right_scene_point() {
    // Зум 2×: кадрирование — центральная четверть буфера. Точка (0,5; 0,5) — центр, плитка в середине сетки.
    let crop = PixelRect {
        left: 1024,
        top: 768,
        size: PixelSize::new(2048, 1536),
    };
    let placement = tile_placement(&passport(ReadoutAspect::FourByThree, 3072), crop).unwrap();
    let expected = TilePlacement::new(0.25, 0.25, 0.75, 0.75, 4096.0 / 3072.0).unwrap();
    assert_eq!(placement, expected);
    // Центр плитки (столбец 8, строка 12) кадрирования: x = 0,25 + 0,5 · 8,5/32, y = 0,25 + 0,5 · 12,5/24.
    let (x, y) = (0.25 + 0.5 * 8.5 / 32.0, 0.25 + 0.5 * 12.5 / 24.0);
    let weights = MeteringWeights::build(GRID, placement, spot(x, y), &MeteringConfig::default()).unwrap();
    assert_eq!(heaviest_tile(&weights), (8, 12));
}

#[test]
fn degenerate_buffer_is_an_error() {
    let mut broken = passport(ReadoutAspect::FourByThree, 3072);
    broken.buffer_size = PixelSize::new(0, 0);
    assert_eq!(
        tile_placement(&broken, full_crop(3072)),
        Err(MeteringWeightsError::InvalidPlacement)
    );
}

//! Линейный замер для вспышки: линейность по выдержке, совпадение с обычным замером и ложь пересвета.

#![allow(clippy::unwrap_used, reason = "в тестах падение — сигнал провала")]

use super::*;
use crate::exposure::config::MeteringConfig;
use crate::exposure::measurement::measure_exposure;
use crate::exposure::metering_weights::{MeteringArea, MeteringWeights, TilePlacement};
use crate::exposure::test_support::{
    GRID, partly_clipped_tile, plain_luminance, product, stats_from_tiles, target, tile_of, uniform_gray,
};
use camera_contract::three_a::{MeteringRegion, SensorPoint};
use isp_model::statistics::AwbStatistics;

fn weights() -> MeteringWeights {
    MeteringWeights::build(
        GRID,
        TilePlacement::full_frame(4.0 / 3.0).unwrap(),
        MeteringArea::Matrix,
        &MeteringConfig::default(),
    )
    .unwrap()
}

fn measure(stats: &AwbStatistics, frame: ExposureProduct) -> ExposureMeasurement {
    measure_exposure(
        stats,
        &weights(),
        plain_luminance(),
        frame,
        target(),
        &MeteringConfig::default(),
    )
    .unwrap()
}

#[test]
fn the_same_scene_at_half_and_full_exposure_gives_the_same_mid_gray_product() {
    // Сцена та же; кадр с половинной выдержкой вдвое темнее (линейная шкала): статистика вдвое меньше.
    let full = measure(&uniform_gray(0.2), product(0.02));
    let half = measure(&uniform_gray(0.1), product(0.01));
    let of_full = linear_mid_gray_product(&full, product(0.02), target(), 0.0).unwrap();
    let of_half = linear_mid_gray_product(&half, product(0.01), target(), 0.0).unwrap();
    let ev_gap = of_full.ev_above(of_half).abs();
    assert!(ev_gap < 0.01, "расхождение {ev_gap} EV");
}

#[test]
fn on_an_unclipped_scene_without_protection_it_equals_the_regular_required_product() {
    let frame = product(0.01);
    let measurement = measure(&uniform_gray(0.3), frame);
    assert_eq!(measurement.protection_ev, 0.0);
    let linear = linear_mid_gray_product(&measurement, frame, target(), 0.0).unwrap();
    assert!(linear.ev_above(measurement.required_product).abs() < 0.01);
    // Цель тона 0,075 у серой карты 0,075 даёт ровно произведение кадра.
    let gray = measure(&uniform_gray(target().scene_middle_gray()), frame);
    let at_target = linear_mid_gray_product(&gray, frame, target(), 0.0).unwrap();
    assert!(at_target.ev_above(frame).abs() < 0.01);
}

#[test]
fn key_offset_shifts_the_result_by_exactly_that_many_ev() {
    let frame = product(0.01);
    let measurement = measure(&uniform_gray(0.3), frame);
    let plain = linear_mid_gray_product(&measurement, frame, target(), 0.0).unwrap();
    let shifted = linear_mid_gray_product(&measurement, frame, target(), 1.0).unwrap();
    assert!((shifted.ev_above(plain) - 1.0).abs() < 1e-6);
}

#[test]
fn clipped_highlights_lower_the_linear_estimate_and_expose_the_green_level_for_the_lit_check() {
    let frame = product(0.01);
    let clean = measure(&uniform_gray(0.3), frame);
    assert!(
        (clean.mean_green_level - 0.3).abs() < 0.01,
        "{}",
        clean.mean_green_level
    );
    let clipped = measure(&stats_from_tiles(|_, _| partly_clipped_tile(0.3, 0.5)), frame);
    assert!(clipped.mean_green_level > 0.5, "пересвеченные ячейки считаются белыми");
    // Линейная яркость учитывает пересвет по нижней оценке (1,0): кадр ярче чистого.
    assert!(clipped.mean_luminance > clean.mean_luminance);
}

#[test]
fn the_green_level_ignores_the_metering_weights_and_averages_every_cell_of_the_grid() {
    // Яркое пятно в центре на тёмном фоне: веса «центр» и «точка тапа» дают яркость выше равномерной.
    let is_bright = |column: u32, row: u32| column.abs_diff(GRID.columns / 2) <= 1 && row.abs_diff(GRID.rows / 2) <= 1;
    let scene = stats_from_tiles(|column, row| {
        // У части плиток фона ячейки ещё и пересвечены: они тоже входят в равномерное среднее.
        if is_bright(column, row) {
            tile_of([0.8; 3])
        } else if row == 0 {
            partly_clipped_tile(0.05, 0.25)
        } else {
            tile_of([0.05; 3])
        }
    });
    let spot = MeteringWeights::build(
        GRID,
        TilePlacement::full_frame(4.0 / 3.0).unwrap(),
        MeteringArea::Spot(MeteringRegion::new(SensorPoint::CENTER, 0.2).unwrap()),
        &MeteringConfig::default(),
    )
    .unwrap();
    let frame = product(0.01);
    let by_matrix = measure(&scene, frame);
    let by_spot = measure_exposure(
        &scene,
        &spot,
        plain_luminance(),
        frame,
        target(),
        &MeteringConfig::default(),
    )
    .unwrap();
    // Эталон: каждая ячейка сетки с одинаковым весом (пересвеченная = 1,0).
    let bright_tiles = (0..GRID.rows)
        .flat_map(|row| (0..GRID.columns).map(move |column| (column, row)))
        .filter(|&(column, row)| is_bright(column, row))
        .count() as f32;
    let tiles = (GRID.rows * GRID.columns) as f32;
    let clipped_row_tiles = GRID.columns as f32;
    let dark_tiles = tiles - bright_tiles - clipped_row_tiles;
    let expected = (bright_tiles * 0.8 + clipped_row_tiles * (0.25 + 0.75 * 0.05) + dark_tiles * 0.05) / tiles;
    // Замер экспозиции действительно видит разное: иначе тест ничего не доказывает.
    assert!(by_spot.mean_luminance > 2.0 * by_matrix.mean_luminance);
    assert!(
        (by_matrix.mean_green_level - expected).abs() < 0.005,
        "{} вместо {expected}",
        by_matrix.mean_green_level
    );
    assert_eq!(by_spot.mean_green_level, by_matrix.mean_green_level);
}

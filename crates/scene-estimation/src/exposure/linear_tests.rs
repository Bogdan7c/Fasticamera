//! Линейный замер для вспышки: линейность по выдержке, совпадение с обычным замером и ложь пересвета.

#![allow(clippy::unwrap_used, reason = "в тестах падение — сигнал провала")]

use super::*;
use crate::exposure::config::MeteringConfig;
use crate::exposure::measurement::measure_exposure;
use crate::exposure::metering_weights::{MeteringArea, MeteringWeights, TilePlacement};
use crate::exposure::test_support::{
    GRID, partly_clipped_tile, plain_luminance, product, stats_from_tiles, target, uniform_gray,
};
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

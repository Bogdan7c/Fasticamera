//! Таблица весов: нормировка, центр против краёв, регион у края, регион вне сетки.

#![allow(clippy::unwrap_used, reason = "в тестах падение — сигнал провала")]

use camera_contract::three_a::SensorPoint;

use super::*;
use crate::exposure::test_support::GRID;

fn full_frame() -> TilePlacement {
    TilePlacement::full_frame(4.0 / 3.0).unwrap()
}

fn region(x: f32, y: f32, size: f32) -> MeteringRegion {
    MeteringRegion::new(SensorPoint::new(x, y).unwrap(), size).unwrap()
}

fn weight_at(weights: &MeteringWeights, column: u32, row: u32) -> f32 {
    weights.weights()[(row * GRID.columns + column) as usize]
}

/// Сумма весов в f64: сложение 768 чисел в f32 копит собственную ошибку, а не ошибку таблицы.
fn weight_sum(weights: &MeteringWeights) -> f64 {
    weights.weights().iter().map(|&weight| f64::from(weight)).sum()
}

fn build(area: MeteringArea) -> Result<MeteringWeights, MeteringWeightsError> {
    MeteringWeights::build(GRID, full_frame(), area, &MeteringConfig::default())
}

#[test]
fn weights_sum_to_one_for_both_areas() {
    for area in [MeteringArea::Matrix, MeteringArea::Spot(region(0.3, 0.6, 0.2))] {
        let weights = build(area).unwrap();
        let sum = weight_sum(&weights);
        assert!((sum - 1.0).abs() < 1e-6, "{area:?}: сумма {sum}");
        assert_eq!(weights.weights().len(), 32 * 24);
    }
}

#[test]
fn matrix_weights_the_centre_above_the_corner_and_is_symmetric() {
    let weights = build(MeteringArea::Matrix).unwrap();
    assert!(weight_at(&weights, 16, 12) > 3.0 * weight_at(&weights, 0, 0));
    assert!((weight_at(&weights, 3, 5) - weight_at(&weights, 28, 18)).abs() < 1e-9);
    assert!(weights.applies_highlight_protection());
}

#[test]
fn spot_weight_follows_the_covered_area_of_each_tile() {
    // Регион 0,25 меньшей стороны (высоты) вокруг центра: по y ±0,125, по x ±0,125·3/4.
    let weights = build(MeteringArea::Spot(region(0.5, 0.5, 0.25))).unwrap();
    assert!(!weights.applies_highlight_protection());
    let inside = weight_at(&weights, 16, 12);
    let outside = weight_at(&weights, 2, 2);
    assert!(inside > 100.0 * outside, "внутри {inside}, вне {outside}");
    // Квадрат в пикселях: узкий по нормированному x. Плитка в 5 плиток слева от центра — уже вне региона.
    assert!(weight_at(&weights, 16, 12) > 100.0 * weight_at(&weights, 11, 12));
    // По y регион шире (в плитках), чем по x: 0,25 высоты = 6 строк, 0,25·0,75 ширины ≈ 6 столбцов — равны в пикселях.
    let rows_inside = (0..GRID.rows)
        .filter(|&row| weight_at(&weights, 16, row) > 10.0 * outside)
        .count();
    let columns_inside = (0..GRID.columns)
        .filter(|&column| weight_at(&weights, column, 12) > 10.0 * outside)
        .count();
    assert!((5..=8).contains(&rows_inside), "строк {rows_inside}");
    assert!((5..=8).contains(&columns_inside), "столбцов {columns_inside}");
}

#[test]
fn spot_at_the_edge_is_clipped_not_shifted() {
    let weights = build(MeteringArea::Spot(region(0.0, 0.0, 0.3))).unwrap();
    assert!((weight_sum(&weights) - 1.0).abs() < 1e-6);
    assert!(
        weights
            .weights()
            .iter()
            .all(|weight| weight.is_finite() && *weight >= 0.0)
    );
    // Центр региона — угол кадра: тяжелее всего угловая плитка, а плитка за «сдвинутым» регионом лёгкая.
    assert!(weight_at(&weights, 0, 0) > weight_at(&weights, 0, 12) * 50.0);
}

#[test]
fn spot_outside_the_placed_grid_is_an_explicit_error() {
    // Сетка лежит только в левой половине кадра, тап — в правой.
    let left_half = TilePlacement::new(0.0, 0.0, 0.4, 1.0, 4.0 / 3.0).unwrap();
    let result = MeteringWeights::build(
        GRID,
        left_half,
        MeteringArea::Spot(region(0.9, 0.5, 0.1)),
        &MeteringConfig::default(),
    );
    assert_eq!(result, Err(MeteringWeightsError::RegionOutsideGrid));
}

#[test]
fn invalid_inputs_are_errors() {
    assert_eq!(
        TilePlacement::new(0.5, 0.0, 0.5, 1.0, 1.0),
        Err(MeteringWeightsError::InvalidPlacement)
    );
    assert_eq!(
        TilePlacement::full_frame(f32::NAN),
        Err(MeteringWeightsError::InvalidPlacement)
    );
    let empty = MeteringWeights::build(
        TileGrid { columns: 0, rows: 3 },
        full_frame(),
        MeteringArea::Matrix,
        &MeteringConfig::default(),
    );
    assert!(matches!(
        empty,
        Err(MeteringWeightsError::EmptyGrid { columns: 0, rows: 3 })
    ));
    let broken = MeteringConfig {
        matrix_center_sigma: f32::NAN,
        ..MeteringConfig::default()
    };
    assert!(matches!(
        MeteringWeights::build(GRID, full_frame(), MeteringArea::Matrix, &broken),
        Err(MeteringWeightsError::Config(_))
    ));
}

#[test]
fn placement_maps_tiles_into_the_covered_part_of_the_frame() {
    // Сетка лежит в центральной половине по x; тап в центре попадает ровно в неё, тап у левого края — вне.
    let centered = TilePlacement::new(0.25, 0.0, 0.75, 1.0, 4.0 / 3.0).unwrap();
    let config = MeteringConfig::default();
    assert!(MeteringWeights::build(GRID, centered, MeteringArea::Spot(region(0.5, 0.5, 0.2)), &config).is_ok());
    assert_eq!(
        MeteringWeights::build(GRID, centered, MeteringArea::Spot(region(0.02, 0.5, 0.05)), &config),
        Err(MeteringWeightsError::RegionOutsideGrid)
    );
}

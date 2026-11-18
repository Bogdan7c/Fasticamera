//! Замер: серая карта, среднее в логарифме, пересвеченные ячейки, качество, свет разной температуры.

#![allow(clippy::unwrap_used, reason = "в тестах падение — сигнал провала")]

use camera_contract::three_a::{MeteringRegion, SensorPoint};

use isp_model::statistics::TileGrid;

use super::*;
use crate::exposure::metering_weights::{MeteringArea, TilePlacement};
use crate::exposure::test_support::{
    GRID, isp_params_for, neutral_for, partly_clipped_tile, plain_luminance, product, stats_from, stats_from_tiles,
    target, uniform_gray,
};

fn matrix_weights() -> MeteringWeights {
    weights_for(MeteringArea::Matrix)
}

fn weights_for(area: MeteringArea) -> MeteringWeights {
    MeteringWeights::build(
        GRID,
        TilePlacement::full_frame(4.0 / 3.0).unwrap(),
        area,
        &MeteringConfig::default(),
    )
    .unwrap()
}

fn center_spot() -> MeteringWeights {
    let region = MeteringRegion::new(SensorPoint::CENTER, 0.3).unwrap();
    weights_for(MeteringArea::Spot(region))
}

/// Разница `required` и `frame` в EV.
fn required_ev(measurement: &ExposureMeasurement, frame: ExposureProduct) -> f64 {
    measurement.required_product.ev_above(frame)
}

fn measure(stats: &AwbStatistics, weights: &MeteringWeights, frame: ExposureProduct) -> ExposureMeasurement {
    measure_exposure(
        stats,
        weights,
        plain_luminance(),
        frame,
        target(),
        &MeteringConfig::default(),
    )
    .unwrap()
}

#[test]
fn middle_gray_scene_needs_exactly_the_exposure_it_already_has() {
    let frame = product(0.01);
    let measurement = measure(&uniform_gray(0.075), &matrix_weights(), frame);
    assert_eq!(measurement.quality, MeasurementQuality::Measured);
    assert!(required_ev(&measurement, frame).abs() < 0.01, "{measurement:?}");
    assert!((measurement.used_weight - 1.0).abs() < 1e-4);
}

#[test]
fn brighter_scene_asks_for_less_light_by_the_same_number_of_stops() {
    let frame = product(0.01);
    let twice_brighter = measure(&uniform_gray(0.15), &matrix_weights(), frame);
    assert!((required_ev(&twice_brighter, frame) + 1.0).abs() < 0.01);
    let darker = measure(&uniform_gray(0.075 / 4.0), &matrix_weights(), frame);
    assert!((required_ev(&darker, frame) - 2.0).abs() < 0.01);
}

#[test]
fn mean_is_taken_in_log_space_not_in_linear_luminance() {
    // Левая половина 0,0375, правая 0,15: среднее логарифмов = log2(0,075); линейное среднее
    // (0,09375) дало бы другое число. Гаусс симметричен, поэтому половины весят поровну.
    let stats = stats_from(|column, _| if column < 16 { [0.0375; 3] } else { [0.15; 3] });
    let frame = product(0.01);
    let measurement = measure(&stats, &matrix_weights(), frame);
    assert!(required_ev(&measurement, frame).abs() < 0.01, "{measurement:?}");
}

#[test]
fn clipped_cells_count_as_clip_luminance_and_protection_applies_to_matrix_only() {
    // В каждой плитке 20 % ячеек пересвечено, остальные — серые 0,05: плитка Y = 0,8·0,05 + 0,2·1,0 = 0,24.
    let stats = stats_from_tiles(|_, _| partly_clipped_tile(0.05, 0.2));
    let frame = product(0.01);
    let config = MeteringConfig::default();
    let spot = measure(&stats, &center_spot(), frame);
    assert_eq!(spot.quality, MeasurementQuality::Measured);
    let expected = (0.075_f64 / 0.24).log2();
    assert!((required_ev(&spot, frame) - expected).abs() < 0.02, "{spot:?}");
    assert_eq!(spot.protection_ev, 0.0, "точечный замер без защиты светов");
    // Матричный: тот же кадр плюс линейная защита: (0,2 − 0,02) / (0,35 − 0,02) · 1,5 = 0,82 EV вниз.
    let matrix = measure(&stats, &matrix_weights(), frame);
    let protection = -f64::from(config.highlight_protection_max_ev)
        * f64::from(
            (0.2 - config.highlight_protect_start) / (config.clipped_fraction_limit - config.highlight_protect_start),
        );
    assert!(
        (f64::from(matrix.protection_ev) - protection).abs() < 0.01,
        "{matrix:?}"
    );
    assert!((required_ev(&matrix, frame) - (expected + protection)).abs() < 0.02);
    assert!((matrix.clipped_fraction - 0.2).abs() < 0.01);
    // Без учёта пересвеченных ячеек сцена выглядела бы тёмной (Y = 0,05) и замер просил бы СВЕТЛЕЕ.
    assert!(required_ev(&spot, frame) < 0.0);
}

#[test]
fn severe_clipping_gives_a_bounded_step_down_from_the_frame_product() {
    // Почти весь кадр пересвечен целиком — точной оценки света нет.
    let stats = stats_from(|column, _| if column < 30 { [1.0; 3] } else { [0.05; 3] });
    let frame = product(0.02);
    let measurement = measure(&stats, &matrix_weights(), frame);
    assert_eq!(measurement.quality, MeasurementQuality::ClippedHighlights);
    let step = f64::from(MeteringConfig::default().max_step_down_ev);
    assert!(
        (required_ev(&measurement, frame) + step).abs() < 1e-9,
        "{measurement:?}"
    );
}

#[test]
fn near_black_scene_gives_a_bounded_step_up_and_is_reported_too_dark() {
    let frame = product(0.002);
    let measurement = measure(&uniform_gray(0.0005), &matrix_weights(), frame);
    assert_eq!(measurement.quality, MeasurementQuality::TooDark);
    let step = f64::from(MeteringConfig::default().max_step_up_ev);
    assert!(
        (required_ev(&measurement, frame) - step).abs() < 1e-9,
        "{measurement:?}"
    );
    assert!(measurement.dark_fraction > 0.99);
}

#[test]
fn same_gray_card_under_2800k_and_d65_asks_for_the_same_exposure() {
    // Серая карта под двумя светами: мозаика t·N, а веса яркости берутся из настоящего ISP-кадра.
    let frame = product(0.01);
    let mut results = Vec::new();
    for kelvin in [2800.0, 6504.0] {
        let neutral = neutral_for(kelvin);
        let params = isp_params_for(neutral);
        let [n_r, n_g, n_b] = neutral.values().map(|value| value as f32);
        let level = 0.1_f32;
        let stats = stats_from(|_, _| [level * n_r, level * n_g, level * n_b]);
        let measurement = measure_exposure(
            &stats,
            &matrix_weights(),
            params.scene_luminance_weights(),
            frame,
            target(),
            &MeteringConfig::default(),
        )
        .unwrap();
        assert_eq!(measurement.quality, MeasurementQuality::Measured, "{kelvin} К");
        results.push(required_ev(&measurement, frame));
    }
    assert!((results[0] - results[1]).abs() <= 0.05, "{results:?}");
    // Это не «оба ноль»: серая карта t·max(N) с t = 0,1 ярче цели, и оба замера просят темнее.
    let expected = (0.075_f64 / (0.1 * color_max(2800.0))).log2();
    assert!((results[0] - expected).abs() < 0.05, "{results:?} против {expected}");
}

fn color_max(kelvin: f64) -> f64 {
    neutral_for(kelvin).values().into_iter().fold(f64::MIN, f64::max)
}

#[test]
fn bright_window_outside_the_spot_does_not_move_the_spot_but_moves_the_matrix() {
    // Центр — серый 0,075 (на цели); справа окно в +4 EV (0,6), не пересвечено.
    let stats = stats_from(|column, _| if column >= 22 { [0.6; 3] } else { [0.075; 3] });
    let frame = product(0.01);
    let spot = measure(&stats, &center_spot(), frame);
    assert!(
        required_ev(&spot, frame).abs() < 0.1,
        "точка не должна уходить: {spot:?}"
    );
    let matrix = measure(&stats, &matrix_weights(), frame);
    assert!(
        required_ev(&matrix, frame) < -0.3,
        "матрица обязана учесть окно: {matrix:?}"
    );
}

#[test]
fn window_clipping_triggers_highlight_protection_in_matrix_metering() {
    // Окно слева-сверху пересвечено на ~8 %: матрица опускает экспозицию, точка — нет.
    let stats = stats_from(|column, row| if column < 8 && row < 8 { [1.0; 3] } else { [0.075; 3] });
    let frame = product(0.01);
    let matrix = measure(&stats, &matrix_weights(), frame);
    let spot = measure(&stats, &center_spot(), frame);
    assert_eq!(matrix.quality, MeasurementQuality::Measured);
    assert!(matrix.protection_ev < -0.1, "{matrix:?}");
    assert!(required_ev(&matrix, frame) < required_ev(&spot, frame) - 0.2);
}

#[test]
fn mismatched_grid_and_empty_statistics_are_errors() {
    let other_grid = TileGrid { columns: 4, rows: 3 };
    let small = AwbStatistics::from_tiles(other_grid, vec![AwbTile::default(); 12]).unwrap();
    let result = measure_exposure(
        &small,
        &matrix_weights(),
        plain_luminance(),
        product(0.01),
        target(),
        &MeteringConfig::default(),
    );
    assert!(matches!(result, Err(ExposureMeasurementError::GridMismatch { .. })));
    let empty = stats_from_tiles(|_, _| AwbTile::default());
    let result = measure_exposure(
        &empty,
        &matrix_weights(),
        plain_luminance(),
        product(0.01),
        target(),
        &MeteringConfig::default(),
    );
    assert!(matches!(result, Err(ExposureMeasurementError::NoUsableTiles { .. })));
    assert!(ExposureTarget::new(1.5).is_err());
    assert!(ExposureTarget::new(f32::NAN).is_err());
}

#[test]
fn partial_clipping_never_asks_for_more_light_but_follows_the_measurement_down() {
    let frame = product(0.02);
    // Окно в центре (~45 % веса) пересвечено, остальное очень тёмное: измерение просит ВЫШЕ кадра.
    let backlit = stats_from(|column, _| {
        if (10..22).contains(&column) {
            [1.0; 3]
        } else {
            [0.004; 3]
        }
    });
    let measurement = measure(&backlit, &matrix_weights(), frame);
    assert_eq!(measurement.quality, MeasurementQuality::ClippedHighlights);
    assert!(measurement.clipped_fraction > 0.35 && measurement.clipped_fraction < 0.8);
    assert!(
        required_ev(&measurement, frame) <= 1e-9,
        "вверх нельзя: {measurement:?}"
    );
    // Светлое окно и умеренно яркий остаток: измерение просит ниже кадра, и вниз идём ровно по нему.
    let bright = stats_from(|column, _| if (10..22).contains(&column) { [1.0; 3] } else { [0.5; 3] });
    let down = measure(&bright, &matrix_weights(), frame);
    assert_eq!(down.quality, MeasurementQuality::ClippedHighlights);
    let step = f64::from(MeteringConfig::default().max_step_down_ev);
    assert!(
        required_ev(&down, frame) < -0.5 && (required_ev(&down, frame) + step).abs() > 0.05,
        "{down:?}"
    );
}

#[test]
fn cold_start_phase_uses_the_larger_steps_and_established_phase_the_usual_ones() {
    let config = MeteringConfig::default();
    let run = |stats: &AwbStatistics, frame, phase| {
        measure_exposure_in_phase(
            stats,
            &matrix_weights(),
            plain_luminance(),
            frame,
            target(),
            &config,
            phase,
        )
        .unwrap()
    };
    // Почти весь кадр за белым: шаг вниз 5 EV на холодном старте, 2 EV потом.
    let blown = stats_from(|column, _| if column < 30 { [1.0; 3] } else { [0.05; 3] });
    let frame = product(0.02);
    let cold = run(&blown, frame, MeasurementPhase::ColdStart);
    let warm = run(&blown, frame, MeasurementPhase::Established);
    assert!((required_ev(&cold, frame) + 5.0).abs() < 1e-9, "{cold:?}");
    assert!((required_ev(&warm, frame) + 2.0).abs() < 1e-9, "{warm:?}");
    // Почти чёрный кадр: 5 EV вверх на холодном старте, 3 EV потом.
    let dark_frame = product(0.002);
    let cold = run(&uniform_gray(0.0005), dark_frame, MeasurementPhase::ColdStart);
    let warm = run(&uniform_gray(0.0005), dark_frame, MeasurementPhase::Established);
    assert!((required_ev(&cold, dark_frame) - 5.0).abs() < 1e-9, "{cold:?}");
    assert!((required_ev(&warm, dark_frame) - 3.0).abs() < 1e-9, "{warm:?}");
}

#[test]
fn cold_start_does_not_change_the_exact_measurements() {
    // Обычная сцена и частичный пересвет считаются по замеру, шаги тут ни при чём.
    let frame = product(0.01);
    let stats = uniform_gray(0.0375);
    let cold = measure_exposure_in_phase(
        &stats,
        &matrix_weights(),
        plain_luminance(),
        frame,
        target(),
        &MeteringConfig::default(),
        MeasurementPhase::ColdStart,
    )
    .unwrap();
    assert_eq!(cold, measure(&stats, &matrix_weights(), frame));
}

//! Замер: серая карта, обычное среднее и лог-среднее, пересвеченные ячейки и их предел, качество, свет разной
//! температуры.

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
fn mean_is_taken_in_linear_light_by_default_and_in_log_space_only_by_choice() {
    // Левая половина 0,0375, правая 0,15 (без пересвета). Гаусс симметричен, поэтому половины весят поровну:
    // обычное среднее 0,09375 → темнее на log2(0,075/0,09375) = −0,32 EV; среднее логарифмов = log2(0,075) → 0 EV.
    let stats = stats_from(|column, _| if column < 16 { [0.0375; 3] } else { [0.15; 3] });
    let frame = product(0.01);
    let linear = measure(&stats, &matrix_weights(), frame);
    let expected_linear = (0.075_f64 / 0.09375).log2();
    assert!(
        (required_ev(&linear, frame) - expected_linear).abs() < 0.01,
        "по умолчанию — обычное среднее: {linear:?}"
    );
    let log = measure_with(&stats, &matrix_weights(), frame, &log_luminance_config());
    assert!(
        required_ev(&log, frame).abs() < 0.01,
        "лог-среднее — только по явной настройке: {log:?}"
    );
}

/// Настройки замера по умолчанию, но с прежним лог-средним (08a…13b).
fn log_luminance_config() -> MeteringConfig {
    MeteringConfig {
        scene_average: SceneAverage::LogLuminance,
        ..MeteringConfig::default()
    }
}

fn measure_with(
    stats: &AwbStatistics,
    weights: &MeteringWeights,
    frame: ExposureProduct,
    config: &MeteringConfig,
) -> ExposureMeasurement {
    measure_exposure(stats, weights, plain_luminance(), frame, target(), config).unwrap()
}

/// Взвешенное обычное среднее яркостей плиток (без пересвета): эталон, посчитанный независимо от замера.
fn weighted_linear_mean(weights: &MeteringWeights, luminance_of: impl Fn(u32, u32) -> f64) -> f64 {
    let columns = GRID.columns;
    weights
        .weights()
        .iter()
        .enumerate()
        .map(|(index, &weight)| {
            let index = u32::try_from(index).unwrap();
            f64::from(weight) * luminance_of(index % columns, index / columns)
        })
        .sum::<f64>()
        / weights.weights().iter().map(|&weight| f64::from(weight)).sum::<f64>()
}

#[test]
fn lamp_lit_table_in_a_dark_room_is_metered_by_average_light_not_pulled_up_by_the_dark() {
    // Сцена 13c в миниатюре: стол под лампой (центр, 10×8 плиток) в 32 раза светлее тёмной комнаты вокруг.
    // Ничего не пересвечено, поэтому ответ — ровно обычное среднее света по весам матрицы.
    let luminance_of = |column: u32, row: u32| {
        if (11..21).contains(&column) && (8..16).contains(&row) {
            0.32
        } else {
            0.01
        }
    };
    let stats = stats_from(|column, row| [luminance_of(column, row) as f32; 3]);
    let frame = product(0.01);
    let weights = matrix_weights();
    let measurement = measure(&stats, &weights, frame);
    let expected = (0.075 / weighted_linear_mean(&weights, luminance_of)).log2();
    assert!(
        (required_ev(&measurement, frame) - expected).abs() < 0.02,
        "{measurement:?} против {expected}"
    );
    // Прежнее лог-среднее на этой сцене тянуло экспозицию вверх тёмной комнатой — причина «×12» из 13c.
    let log = measure_with(&stats, &weights, frame, &log_luminance_config());
    assert!(
        required_ev(&log, frame) - required_ev(&measurement, frame) > 1.5,
        "лог-среднее просило бы заметно больше света: {log:?} против {measurement:?}"
    );
}

#[test]
fn uniform_scene_is_metered_the_same_by_both_averages() {
    // На ровной сцене способы совпадают — поэтому цель тона (0,075) от выбора не зависит.
    for level in [0.01_f32, 0.075, 0.3] {
        let frame = product(0.01);
        let stats = uniform_gray(level);
        let linear = measure(&stats, &matrix_weights(), frame);
        let log = measure_with(&stats, &matrix_weights(), frame, &log_luminance_config());
        assert!(
            (required_ev(&linear, frame) - required_ev(&log, frame)).abs() < 0.01,
            "уровень {level}: {linear:?} против {log:?}"
        );
    }
}

#[test]
fn clipped_cells_count_as_clip_luminance_and_protection_applies_to_matrix_only() {
    // В каждой плитке 20 % ячеек пересвечено, остальные — серые 0,05: вклад принятых 0,8·0,05 = 0,04,
    // вклад пересвета 0,2·1,0 = 0,2 (нижняя оценка).
    let stats = stats_from_tiles(|_, _| partly_clipped_tile(0.05, 0.2));
    let frame = product(0.01);
    let config = MeteringConfig::default();
    let share = f64::from(config.clipped_max_share_of_target);
    // Точка: защиты нет, пересвет ограничен долей цели (0,5·0,075 = 0,0375) и вычитается из цели, а растёт с
    // выдержкой только измеримая часть: 0,04 · (P*/P) + 0,0375 = 0,075.
    let spot = measure(&stats, &center_spot(), frame);
    assert_eq!(spot.quality, MeasurementQuality::Measured);
    assert_eq!(spot.protection_ev, 0.0, "точечный замер без защиты светов");
    let spot_expected = ((0.075_f64 - share * 0.075) / 0.04).log2();
    assert!((required_ev(&spot, frame) - spot_expected).abs() < 0.02, "{spot:?}");
    // Матрица: защита (0,2 − 0,02) / (0,35 − 0,02) · 1,5 = 0,82 EV вниз, и предел пересвета — от защищённой цели.
    let matrix = measure(&stats, &matrix_weights(), frame);
    let protection = -f64::from(config.highlight_protection_max_ev)
        * f64::from(
            (0.2 - config.highlight_protect_start) / (config.clipped_fraction_limit - config.highlight_protect_start),
        );
    assert!(
        (f64::from(matrix.protection_ev) - protection).abs() < 0.01,
        "{matrix:?}"
    );
    let protected_target = 0.075 * protection.exp2();
    let matrix_expected = ((protected_target - share * protected_target) / 0.04).log2();
    assert!(
        (required_ev(&matrix, frame) - matrix_expected).abs() < 0.02,
        "{matrix:?}"
    );
    assert!((matrix.clipped_fraction - 0.2).abs() < 0.01);
    // Без учёта пересвеченных ячеек сцена выглядела бы тёмной (Y = 0,04) и замер просил бы СВЕТЛЕЕ.
    assert!(required_ev(&spot, frame) < 0.0);
    // Прежнее лог-среднее берёт пересвет без предела: плитка Y = 0,04 + 0,2 = 0,24.
    let log_spot = measure_with(&stats, &center_spot(), frame, &log_luminance_config());
    assert!((required_ev(&log_spot, frame) - (0.075_f64 / 0.24).log2()).abs() < 0.02);
}

#[test]
fn the_same_scene_metered_at_half_and_double_exposure_asks_for_the_same_product() {
    // Компенсация EV держится на том, что замер не зависит от выдержки кадра (13c: без этого EV +1 давал ×3,3,
    // EV −1 — ×0,2). Сцена: серая стена (растёт с выдержкой) и всегда-белое окно (белое при любой выдержке).
    // Маленькое окно — пересвет ниже предела, большое — упирается в предел; для точки — окно под тапом.
    let wall_at_reference = 0.03_f32;
    let reference = product(0.01);
    let cases: [(&str, std::ops::Range<u32>, MeteringWeights); 3] = [
        ("малое окно, матрица", 15..17, matrix_weights()),
        ("большое окно, матрица", 13..19, matrix_weights()),
        ("окно под точкой", 15..17, center_spot()),
    ];
    for (name, window, weights) in cases {
        let required: Vec<f64> = [0.5_f64, 1.0, 2.0]
            .into_iter()
            .map(|scale| {
                let wall = wall_at_reference * scale as f32;
                let stats = stats_from(|column, _| if window.contains(&column) { [1.0; 3] } else { [wall; 3] });
                let frame = product(0.01 * scale);
                let measurement = measure(&stats, &weights, frame);
                assert_eq!(
                    measurement.quality,
                    MeasurementQuality::Measured,
                    "{name}: {measurement:?}"
                );
                measurement.required_product.ev_above(reference)
            })
            .collect();
        let spread = required.iter().fold(f64::MIN, |high, &value| high.max(value))
            - required.iter().fold(f64::MAX, |low, &value| low.min(value));
        assert!(
            spread < 0.03,
            "{name}: нужная экспозиция зависит от выдержки кадра: {required:?} EV"
        );
    }
}

#[test]
fn an_always_white_window_takes_at_most_its_share_of_the_target_so_the_rest_still_answers_exposure() {
    // Окно (столбцы 13..19, ≈ четверть веса матрицы — там, где защита светов уже глубокая, но полоса ещё «норма») пересвечено при ЛЮБОЙ экспозиции, остальное — серая стена.
    // Без предела одно окно давало бы среднее ≥ 0,3 и замер всегда просил бы темнее (экспозиция уходила в ноль).
    // С пределом замер отвечает на яркость стены: стена вдвое темнее — просит больше света.
    let wall = |level: f32| {
        stats_from(move |column, _| {
            if (13..19).contains(&column) {
                [1.0; 3]
            } else {
                [level; 3]
            }
        })
    };
    let frame = product(0.01);
    let bright_wall = measure(&wall(0.06), &matrix_weights(), frame);
    let dark_wall = measure(&wall(0.03), &matrix_weights(), frame);
    assert_eq!(bright_wall.quality, MeasurementQuality::Measured, "{bright_wall:?}");
    assert!(
        required_ev(&dark_wall, frame) > required_ev(&bright_wall, frame) + 0.3,
        "стена темнее — света нужно больше: {dark_wall:?} против {bright_wall:?}"
    );
    // Равновесие существует: при стене, дающей ровно «остаток» цели, замер не просит ничего менять.
    let config = MeteringConfig::default();
    let clipped_share = f64::from(bright_wall.clipped_fraction);
    let protected_target = 0.075 * f64::from(bright_wall.protection_ev).exp2();
    let wall_for_balance =
        (1.0 - f64::from(config.clipped_max_share_of_target)) * protected_target / (1.0 - clipped_share);
    let balanced = measure(&wall(wall_for_balance as f32), &matrix_weights(), frame);
    assert!(
        required_ev(&balanced, frame).abs() < 0.03,
        "в равновесии менять нечего: {balanced:?} (стена {wall_for_balance})"
    );
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

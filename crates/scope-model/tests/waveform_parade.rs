//! Waveform и RGB parade: формулы на синтетических картинках, через общее накопление.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::{code_of, flat, picture};
use scope_model::trace_rules::{ParadeRule, WaveformRule};
use scope_model::{HorizontalOrder, SampleStride, ShownCodes, accumulate};

#[test]
fn horizontal_gradient_makes_a_rising_diagonal_on_the_waveform() {
    let rule = WaveformRule::new(16, 16).unwrap();
    // Столбец картинки k — серый код, растущий слева направо.
    let gradient = picture(16, 4, |x, _| [code_of(f64::from(x) / 15.0); 3]);
    let counters = accumulate(&gradient, SampleStride::EVERY_PIXEL, &rule).unwrap();
    assert_eq!(counters.total(), 64);
    let mut previous_row = u32::MAX;
    for column in 0..16u32 {
        let code = u32::from(code_of(f64::from(column) / 15.0));
        let expected_row = 15 - code * 16 / 1024;
        // Все 4 отсчёта столбца — в одной ячейке, и в ней одной.
        assert_eq!(counters.count(0, expected_row, column), Some(4), "столбец {column}");
        // Строка не растёт: уровень поднимается вместе со столбцом (строка 0 — верх).
        assert!(expected_row <= previous_row);
        previous_row = expected_row;
    }
    assert_eq!(counters.count(0, 15, 0), Some(4));
    assert_eq!(counters.count(0, 0, 15), Some(4));
}

#[test]
fn white_lands_in_the_top_row_and_black_in_the_bottom_row() {
    let rule = WaveformRule::new(8, 256).unwrap();
    let white = accumulate(&flat(40, 6, [1023; 3]), SampleStride::EVERY_PIXEL, &rule).unwrap();
    let black = accumulate(&flat(40, 6, [0; 3]), SampleStride::EVERY_PIXEL, &rule).unwrap();
    let top_row_total: u32 = (0..8).map(|column| white.count(0, 0, column).unwrap()).sum();
    let bottom_row_total: u32 = (0..8).map(|column| black.count(0, 255, column).unwrap()).sum();
    assert_eq!(top_row_total, 240);
    assert_eq!(bottom_row_total, 240);
    assert_eq!(white.total(), 240);
    assert_eq!(black.total(), 240);
}

#[test]
fn color_luma_uses_bt709_weights_on_codes() {
    let rule = WaveformRule::new(1, 256).unwrap();
    for (code, luma) in [
        ([1023u16, 0, 0], 0.2126),
        ([0, 1023, 0], 0.7152),
        ([0, 0, 1023], 0.0722),
    ] {
        let counters = accumulate(&flat(1, 1, code), SampleStride::EVERY_PIXEL, &rule).unwrap();
        let expected_bin = (luma * 1023.0 * 256.0 / 1024.0_f64).floor() as u32;
        assert_eq!(counters.count(0, 255 - expected_bin, 0), Some(1), "код {code:?}");
    }
}

#[test]
fn picture_columns_map_to_grid_columns_by_integer_proportion() {
    let rule = WaveformRule::new(4, 4).unwrap();
    let counters = accumulate(&flat(10, 1, [0; 3]), SampleStride::EVERY_PIXEL, &rule).unwrap();
    // column = x * 4 / 10: x = 0..9 → 0 0 0 1 1 2 2 2 3 3.
    let per_column: Vec<u32> = (0..4).map(|column| counters.count(0, 3, column).unwrap()).collect();
    assert_eq!(per_column, vec![3, 2, 3, 2]);
}

#[test]
fn gray_gives_three_identical_parade_planes() {
    let rule = ParadeRule::new(8, 64).unwrap();
    let gradient = picture(32, 3, |x, _| [code_of(f64::from(x) / 31.0); 3]);
    let counters = accumulate(&gradient, SampleStride::EVERY_PIXEL, &rule).unwrap();
    assert_eq!(counters.total(), 32 * 3 * 3);
    for row in 0..64 {
        for column in 0..8 {
            let red = counters.count(0, row, column);
            assert_eq!(counters.count(1, row, column), red);
            assert_eq!(counters.count(2, row, column), red);
        }
    }
}

#[test]
fn parade_separates_channels() {
    let rule = ParadeRule::new(1, 256).unwrap();
    // Красный на максимуме, зелёный посередине, синий в нуле.
    let counters = accumulate(&flat(2, 2, [1023, 512, 0]), SampleStride::EVERY_PIXEL, &rule).unwrap();
    assert_eq!(counters.count(0, 0, 0), Some(4));
    assert_eq!(counters.count(1, 255 - 512 * 256 / 1024, 0), Some(4));
    assert_eq!(counters.count(2, 255, 0), Some(4));
    assert_eq!(counters.total(), 12);
}

#[test]
fn mirrored_input_flips_waveform_columns() {
    let rule = WaveformRule::new(4, 4).unwrap();
    // Левая половина белая, правая чёрная (в буфере).
    let values: Vec<[f32; 3]> = (0..8).map(|x| if x % 4 < 2 { [1.0; 3] } else { [0.0; 3] }).collect();
    let stored = ShownCodes::from_display_rgb(4, 2, &values, HorizontalOrder::AsStored).unwrap();
    let mirrored = ShownCodes::from_display_rgb(4, 2, &values, HorizontalOrder::Mirrored).unwrap();
    let stored_counters = accumulate(&stored, SampleStride::EVERY_PIXEL, &rule).unwrap();
    let mirrored_counters = accumulate(&mirrored, SampleStride::EVERY_PIXEL, &rule).unwrap();
    assert_eq!(stored_counters.count(0, 0, 0), Some(2));
    assert_eq!(stored_counters.count(0, 0, 3), Some(0));
    assert_eq!(stored_counters.count(0, 3, 3), Some(2));
    assert_eq!(mirrored_counters.count(0, 0, 0), Some(0));
    assert_eq!(mirrored_counters.count(0, 0, 3), Some(2));
    assert_eq!(mirrored_counters.count(0, 3, 0), Some(2));
}

#[test]
fn stride_counts_exactly_the_documented_samples() {
    let rule = WaveformRule::new(10, 4).unwrap();
    let stride = SampleStride::try_new(4).unwrap();
    // Отсчёты: x, y ∈ {1, 5, 9}. Белые — только они; остальные пиксели чёрные.
    let marked = |coordinate: u32| matches!(coordinate, 1 | 5 | 9);
    let picture = picture(10, 10, |x, y| if marked(x) && marked(y) { [1023; 3] } else { [0; 3] });
    let counters = accumulate(&picture, stride, &rule).unwrap();
    assert_eq!(counters.total(), 9);
    let white_total: u32 = (0..10).map(|column| counters.count(0, 0, column).unwrap()).sum();
    assert_eq!(white_total, 9);
    // По одному белому отсчёту на столбец картинки 1, 5, 9 по 3 строки: столбцы сетки 1, 5, 9.
    for column in [1, 5, 9] {
        assert_eq!(counters.count(0, 0, column), Some(3));
    }
}

#[test]
fn rule_exposes_integer_projection_for_the_gpu_packer() {
    let rule = WaveformRule::new(256, 256).unwrap();
    let projection = rule.level_projection();
    assert_eq!(projection.shift(), 16);
    // Сумма весов — ровно levels · 2^16 / 1024, поэтому белый — последний уровень.
    assert_eq!(projection.weights().iter().sum::<i32>(), 256 * 64);
    assert_eq!(projection.apply([1023; 3]), 255);
    assert_eq!(projection.apply([0; 3]), 0);
}

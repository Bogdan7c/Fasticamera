//! RGB parade и RGB waveform: у них одно правило ([`ParadeRule`], три плоскости R', G', B'); формулы — на
//! синтетических картинках, через общее накопление.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::{code_of, flat, picture};
use scope_model::trace_rules::ParadeRule;
use scope_model::{HorizontalOrder, SampleStride, ShownCodes, accumulate};

#[test]
fn horizontal_gradient_makes_a_rising_diagonal_in_every_plane() {
    let rule = ParadeRule::new(16, 16).unwrap();
    // Столбец картинки k — серый код, растущий слева направо.
    let gradient = picture(16, 4, |x, _| [code_of(f64::from(x) / 15.0); 3]);
    let counters = accumulate(&gradient, SampleStride::EVERY_PIXEL, &rule).unwrap();
    assert_eq!(
        counters.total(),
        64 * 3,
        "каждый отсчёт попадает в каждую из трёх плоскостей"
    );
    let mut previous_row = u32::MAX;
    for column in 0..16u32 {
        let code = u32::from(code_of(f64::from(column) / 15.0));
        let expected_row = 15 - code * 16 / 1024;
        for plane in 0..3 {
            // Все 4 отсчёта столбца — в одной ячейке плоскости, и в ней одной.
            assert_eq!(
                counters.count(plane, expected_row, column),
                Some(4),
                "столбец {column}, плоскость {plane}"
            );
        }
        // Строка не растёт: уровень поднимается вместе со столбцом (строка 0 — верх).
        assert!(expected_row <= previous_row);
        previous_row = expected_row;
    }
    assert_eq!(counters.count(0, 15, 0), Some(4));
    assert_eq!(counters.count(0, 0, 15), Some(4));
}

#[test]
fn white_lands_in_the_top_row_and_black_in_the_bottom_row() {
    let rule = ParadeRule::new(40, 256).unwrap();
    let white = accumulate(&flat(40, 6, [1023; 3]), SampleStride::EVERY_PIXEL, &rule).unwrap();
    let black = accumulate(&flat(40, 6, [0; 3]), SampleStride::EVERY_PIXEL, &rule).unwrap();
    for plane in 0..3 {
        let top_row_total: u32 = (0..40).map(|column| white.count(plane, 0, column).unwrap()).sum();
        let bottom_row_total: u32 = (0..40).map(|column| black.count(plane, 255, column).unwrap()).sum();
        assert_eq!(top_row_total, 240, "плоскость {plane}");
        assert_eq!(bottom_row_total, 240, "плоскость {plane}");
    }
    assert_eq!(white.total(), 240 * 3);
    assert_eq!(black.total(), 240 * 3);
}

#[test]
fn every_sample_column_gets_its_own_grid_column_with_the_same_number_of_samples() {
    // Решение 322: столбец сетки = номер отсчёта вдоль взгляда. Окно 10 × 3 при шаге 1 — 10 столбцов по 3 отсчёта;
    // при шаге 2 — 5 столбцов (x = 0, 2, 4, 6, 8) по 2 отсчёта. Ни пустых, ни двойных столбцов.
    let rule = ParadeRule::new(64, 4).unwrap();
    for (stride, columns, per_column) in [
        (SampleStride::EVERY_PIXEL, 10, 3),
        (SampleStride::EVERY_SECOND_PIXEL, 5, 2),
    ] {
        let counters = accumulate(&flat(10, 3, [0; 3]), stride, &rule).unwrap();
        assert_eq!(counters.shape().columns(), columns);
        for plane in 0..3 {
            let counts: Vec<u32> = (0..columns)
                .map(|column| counters.count(plane, 3, column).unwrap())
                .collect();
            assert_eq!(
                counts,
                vec![per_column; columns as usize],
                "плоскость {plane}, шаг {}",
                stride.get()
            );
        }
    }
}

#[test]
fn gray_gives_three_identical_parade_planes() {
    let rule = ParadeRule::new(32, 64).unwrap();
    let gradient = picture(32, 3, |x, _| [code_of(f64::from(x) / 31.0); 3]);
    let counters = accumulate(&gradient, SampleStride::EVERY_PIXEL, &rule).unwrap();
    assert_eq!(counters.total(), 32 * 3 * 3);
    for row in 0..64 {
        for column in 0..32 {
            let red = counters.count(0, row, column);
            assert_eq!(counters.count(1, row, column), red);
            assert_eq!(counters.count(2, row, column), red);
        }
    }
}

#[test]
fn parade_separates_channels() {
    let rule = ParadeRule::new(1, 256).unwrap();
    // Красный на максимуме, зелёный посередине, синий в нуле. Окно в один столбец: 4 отсчёта в одном столбце сетки.
    let counters = accumulate(&flat(1, 4, [1023, 512, 0]), SampleStride::EVERY_PIXEL, &rule).unwrap();
    assert_eq!(counters.count(0, 0, 0), Some(4));
    assert_eq!(counters.count(1, 255 - 512 * 256 / 1024, 0), Some(4));
    assert_eq!(counters.count(2, 255, 0), Some(4));
    assert_eq!(counters.total(), 12);
}

#[test]
fn mirrored_input_flips_columns() {
    let rule = ParadeRule::new(4, 4).unwrap();
    // Левая половина белая, правая чёрная (в буфере).
    let values: Vec<[f32; 3]> = (0..8).map(|x| if x % 4 < 2 { [1.0; 3] } else { [0.0; 3] }).collect();
    let stored = ShownCodes::from_display_rgb(4, 2, &values, HorizontalOrder::AsStored).unwrap();
    let mirrored = ShownCodes::from_display_rgb(4, 2, &values, HorizontalOrder::Mirrored).unwrap();
    let stored_counters = accumulate(&stored, SampleStride::EVERY_PIXEL, &rule).unwrap();
    let mirrored_counters = accumulate(&mirrored, SampleStride::EVERY_PIXEL, &rule).unwrap();
    for plane in 0..3 {
        assert_eq!(stored_counters.count(plane, 0, 0), Some(2));
        assert_eq!(stored_counters.count(plane, 0, 3), Some(0));
        assert_eq!(stored_counters.count(plane, 3, 3), Some(2));
        assert_eq!(mirrored_counters.count(plane, 0, 0), Some(0));
        assert_eq!(mirrored_counters.count(plane, 0, 3), Some(2));
        assert_eq!(mirrored_counters.count(plane, 3, 0), Some(2));
    }
}

#[test]
fn stride_counts_exactly_the_documented_samples() {
    let rule = ParadeRule::new(10, 4).unwrap();
    let stride = SampleStride::try_new(4).unwrap();
    // Отсчёты: x, y ∈ {1, 5, 9}. Белые — только они; остальные пиксели чёрные.
    let marked = |coordinate: u32| matches!(coordinate, 1 | 5 | 9);
    let picture = picture(10, 10, |x, y| if marked(x) && marked(y) { [1023; 3] } else { [0; 3] });
    let counters = accumulate(&picture, stride, &rule).unwrap();
    assert_eq!(counters.total(), 9 * 3);
    // Отсчёты x = 1, 5, 9 — три столбца сетки (номера отсчётов 0, 1, 2), каждый по 3 белых отсчёта.
    assert_eq!(counters.shape().columns(), 3);
    for plane in 0..3 {
        let white_total: u32 = (0..3).map(|column| counters.count(plane, 0, column).unwrap()).sum();
        assert_eq!(white_total, 9, "плоскость {plane}");
        for column in 0..3 {
            assert_eq!(counters.count(plane, 0, column), Some(3));
        }
    }
}

#[test]
fn rule_exposes_one_integer_projection_per_channel_for_the_gpu_packer() {
    let rule = ParadeRule::new(256, 256).unwrap();
    let [red, green, blue] = rule.level_projections();
    assert_eq!(red.shift(), 16);
    // Белый канал — последний уровень, чёрный — нулевой.
    assert_eq!(red.apply([1023, 0, 0]), 255);
    assert_eq!(red.apply([0; 3]), 0);
    // Проекция смотрит ТОЛЬКО на свой канал.
    assert_eq!(red.apply([0, 1023, 1023]), 0);
    assert_eq!(green.apply([1023, 1023, 0]), 255);
    assert_eq!(green.apply([1023, 0, 1023]), 0);
    assert_eq!(blue.apply([0, 0, 1023]), 255);
    assert_eq!(blue.apply([1023, 1023, 0]), 0);
}

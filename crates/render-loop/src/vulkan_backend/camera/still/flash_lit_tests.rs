//! Проверка света вспышки по пикселям: чтение зелёного решёткой и решение «освещён / нет / нерешаемо».

#![allow(clippy::unwrap_used, reason = "в тестах падение — сигнал провала")]

use std::sync::Arc;

use camera_contract::geometry::{PixelRect, PixelSize};
use still_contract::shot::OwnedPixels;

use super::*;

const SIZE: PixelSize = PixelSize::new(64, 48);
const THRESHOLDS: LitThresholds = LitThresholds {
    min_fraction_of_predicted: 0.5,
    min_predicted_share: 0.2,
};

/// Отсчёты размером `SIZE`: значение пикселя задаёт `value_at(x, y)`.
fn samples_with(value_at: impl Fn(u32, u32) -> u16) -> RawSamples16 {
    let mut bytes = Vec::new();
    for y in 0..SIZE.height {
        for x in 0..SIZE.width {
            bytes.extend_from_slice(&value_at(x, y).to_le_bytes());
        }
    }
    RawSamples16::new(SIZE, Arc::new(OwnedPixels(bytes))).unwrap()
}

fn levels() -> NormalizationLevels {
    NormalizationLevels::new([64.0; 4], 1064.0).unwrap()
}

fn full_area() -> PixelRect {
    PixelRect {
        left: 0,
        top: 0,
        size: SIZE,
    }
}

fn mean(samples: &RawSamples16, area: PixelRect, stride_cells: u32) -> Option<f64> {
    mean_green_level(GreenSampling {
        samples,
        pattern: BayerPattern::BGGR,
        levels: &levels(),
        area,
        stride_cells,
    })
}

#[test]
fn mean_reads_only_the_green_samples_with_the_black_level_removed() {
    // BGGR: зелёные — позиции (1,0) и (0,1) квадрата. Зелёные = 564 (→ 0,5), красные и синие = 1064 (→ 1,0).
    let samples = samples_with(|x, y| if (x + y) % 2 == 1 { 564 } else { 1064 });
    let level = mean(&samples, full_area(), 1).unwrap();
    assert!((level - 0.5).abs() < 1e-6, "{level}");
}

#[test]
fn clipped_samples_count_as_white_and_a_sparse_grid_agrees_with_the_full_one_on_a_flat_scene() {
    let samples = samples_with(|x, y| if (x + y) % 2 == 1 { 5000 } else { 64 });
    assert!((mean(&samples, full_area(), 1).unwrap() - 1.0).abs() < 1e-6);
    assert!((mean(&samples, full_area(), 4).unwrap() - 1.0).abs() < 1e-6);
}

#[test]
fn the_area_limits_where_samples_are_read() {
    // Левая половина тёмная, правая яркая; область — правая половина.
    let samples = samples_with(|x, y| {
        if (x + y) % 2 == 1 {
            if x < 32 { 64 } else { 1064 }
        } else {
            64
        }
    });
    let right = PixelRect {
        left: 32,
        top: 0,
        size: PixelSize::new(32, 48),
    };
    assert!((mean(&samples, right, 1).unwrap() - 1.0).abs() < 1e-6);
    let left = PixelRect {
        left: 0,
        top: 0,
        size: PixelSize::new(32, 48),
    };
    assert!(mean(&samples, left, 1).unwrap().abs() < 1e-6);
}

#[test]
fn an_empty_or_outside_area_gives_nothing_instead_of_a_made_up_number() {
    let samples = samples_with(|_, _| 500);
    let outside = PixelRect {
        left: 100,
        top: 100,
        size: PixelSize::new(8, 8),
    };
    assert_eq!(mean(&samples, outside, 1), None);
}

#[test]
fn a_frame_much_brighter_per_unit_product_than_the_scene_is_lit() {
    // Сцена 0,1 на единицу произведения; кадр 0,4: вспышка дала 75 % света. Предсказывали 60 %.
    let verdict = judge_lit(0.6, Some(0.1), 0.4, THRESHOLDS);
    assert!(matches!(verdict, LitVerdict::Lit { measured_share } if (measured_share - 0.75).abs() < 1e-9));
}

#[test]
fn a_frame_as_bright_as_the_scene_is_not_lit_even_when_the_hal_said_fired() {
    let verdict = judge_lit(0.6, Some(0.1), 0.1, THRESHOLDS);
    assert!(matches!(verdict, LitVerdict::NotLit { measured_share } if measured_share.abs() < 1e-9));
    // Ниже порога от предсказанного: измерили 20 % при 0,5 × 60 % = 30 %.
    assert!(matches!(
        judge_lit(0.6, Some(0.1), 0.125, THRESHOLDS),
        LitVerdict::NotLit { .. }
    ));
    // Чёрный кадр: света нет вовсе.
    assert!(matches!(
        judge_lit(0.6, Some(0.1), 0.0, THRESHOLDS),
        LitVerdict::NotLit { measured_share } if measured_share == 0.0
    ));
}

#[test]
fn a_dim_flash_or_a_missing_scene_level_makes_the_check_undecidable_not_failed() {
    assert_eq!(
        judge_lit(0.1, Some(0.1), 0.1, THRESHOLDS),
        LitVerdict::Undecidable(LitUndecidable::PredictedShareTooLow)
    );
    assert_eq!(
        judge_lit(0.6, None, 0.4, THRESHOLDS),
        LitVerdict::Undecidable(LitUndecidable::NoAmbientLevel)
    );
}

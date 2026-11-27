//! Новый прибор добавляется без правки соседей: подставное правило проходит через общее накопление.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::picture;
use scope_model::{AccumulateError, GridShape, Hit, SampleCount, SamplePosition, SampleStride, ScopeRule, accumulate};

/// Гистограмма красного канала: одна строка, 16 корзин по 64 кода.
struct RedHistogramRule;

impl ScopeRule for RedHistogramRule {
    fn shape(&self, _samples: SampleCount) -> GridShape {
        GridShape::try_new(16, 1, 1).unwrap()
    }

    fn hits(&self, _position: SamplePosition, code: [u16; 3], report: &mut dyn FnMut(Hit)) {
        report(Hit {
            plane: 0,
            row: 0,
            column: u32::from(code[0] / 64),
        });
    }
}

/// Правило с ошибкой: сообщает ячейку вне своей сетки.
struct BrokenRule;

impl ScopeRule for BrokenRule {
    fn shape(&self, _samples: SampleCount) -> GridShape {
        GridShape::try_new(2, 2, 1).unwrap()
    }

    fn hits(&self, _position: SamplePosition, _code: [u16; 3], report: &mut dyn FnMut(Hit)) {
        report(Hit {
            plane: 0,
            row: 2,
            column: 0,
        });
    }
}

#[test]
fn custom_histogram_rule_accumulates_through_the_generic_pass() {
    let picture = picture(4, 4, |x, _| [u16::try_from(x).unwrap() * 300, 7, 7]);
    let counters = accumulate(&picture, SampleStride::EVERY_PIXEL, &RedHistogramRule).unwrap();
    // Красный: 0, 300, 600, 900 → корзины 0, 4, 9, 14 по 4 пикселя.
    for bin in [0u32, 4, 9, 14] {
        assert_eq!(counters.count(0, 0, bin), Some(4), "корзина {bin}");
    }
    assert_eq!(counters.total(), 16);
}

#[test]
fn hit_outside_the_rule_grid_is_an_error_not_silently_dropped() {
    let picture = picture(2, 2, |_, _| [0; 3]);
    let error = accumulate(&picture, SampleStride::EVERY_PIXEL, &BrokenRule).unwrap_err();
    assert_eq!(
        error,
        AccumulateError::HitOutsideGrid(Hit {
            plane: 0,
            row: 2,
            column: 0
        })
    );
}

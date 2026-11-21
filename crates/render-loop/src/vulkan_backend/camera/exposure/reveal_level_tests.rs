//! Правило достижимой цели без runner: границы допуска, компенсация, чужой отрезок.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение — сигнал провала"
)]

use camera_contract::auto_exposure::ExposureProduct;

use super::*;

const TOLERANCE_EV: f64 = 0.1;

fn product(seconds_iso: f64) -> ExposureProduct {
    ExposureProduct::from_seconds_iso(seconds_iso).expect("конечное положительное произведение")
}

/// Отрезок достижимого `[2^darkest_log2, 2^brightest_log2]` в указании.
fn applied_between(darkest_log2: f64, brightest_log2: f64) -> Option<AppliedExposure> {
    Some(AppliedExposure {
        product: product(brightest_log2.exp2()),
        shortfall_ev: 0.0,
        reach: ProductReach::Between {
            darkest: product(darkest_log2.exp2()),
            brightest: product(brightest_log2.exp2()),
        },
    })
}

fn inputs(frame_log2: f64, target_log2: f64, applied: Option<AppliedExposure>) -> DisplayedLevelInputs {
    DisplayedLevelInputs {
        frame_log2,
        gain_ev: 0.0,
        target_log2,
        bias_ev: 0.0,
        applied,
        reach_tolerance_ev: TOLERANCE_EV,
    }
}

#[test]
fn target_beyond_the_brightest_edge_is_cut_to_that_edge() {
    // Кадр на самом светлом краю, цель на 4 EV выше: достижимая цель = край, ошибки нет.
    let error = displayed_level_error_ev(inputs(6.0, 10.0, applied_between(-7.0, 6.0)));
    assert!(error.abs() < 1e-9, "кадр на краю достижимого, ошибка {error}");
}

#[test]
fn target_beyond_the_darkest_edge_is_cut_symmetrically() {
    let error = displayed_level_error_ev(inputs(0.0, -4.0, applied_between(0.0, 6.0)));
    assert!(error.abs() < 1e-9, "кадр на самом тёмном краю, ошибка {error}");
}

#[test]
fn target_inside_the_reach_keeps_the_full_error() {
    // Цель достижима, кадр на 2 EV темнее: AE ещё сходится — ошибка полная.
    let error = displayed_level_error_ev(inputs(1.0, 3.0, applied_between(-7.0, 6.0)));
    assert!((error + 2.0).abs() < 1e-9, "ожидали −2 EV, получили {error}");
}

#[test]
fn gain_and_bias_shift_the_error() {
    let mut shifted = inputs(1.0, 3.0, applied_between(-7.0, 6.0));
    shifted.gain_ev = 0.5;
    shifted.bias_ev = 1.0;
    // Показано 1,5, достижимая цель 4,0.
    let error = displayed_level_error_ev(shifted);
    assert!((error + 2.5).abs() < 1e-9, "ожидали −2,5 EV, получили {error}");
}

#[test]
fn reach_not_containing_the_frame_is_foreign_and_not_trusted() {
    // Отрезок старой камеры [.., 4]; кадр нового потока на 6 — вне отрезка даже с допуском.
    let foreign = displayed_level_error_ev(inputs(6.0, 10.0, applied_between(-7.0, 4.0)));
    assert!((foreign + 4.0).abs() < 1e-9, "судим по полной цели: {foreign}");
    // Допуск на округление: кадр чуть за краем отрезка своего потока — отрезку верим.
    let rounding = displayed_level_error_ev(inputs(6.05, 10.0, applied_between(-7.0, 6.0)));
    assert!(
        (rounding - 0.05).abs() < 1e-9,
        "отрезок свой, цель срезана до 6: {rounding}"
    );
}

#[test]
fn missing_or_unbounded_reach_judges_by_the_full_target() {
    let none = displayed_level_error_ev(inputs(0.0, 3.0, None));
    assert!((none + 3.0).abs() < 1e-9);
    let unbounded = Some(AppliedExposure {
        product: product(1.0),
        shortfall_ev: 0.0,
        reach: ProductReach::Unbounded,
    });
    let error = displayed_level_error_ev(inputs(0.0, 3.0, unbounded));
    assert!((error + 3.0).abs() < 1e-9);
}

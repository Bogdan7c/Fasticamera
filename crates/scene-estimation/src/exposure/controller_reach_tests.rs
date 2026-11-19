//! Подтяжка ISP на ступеньке света за предел сенсора: цель ограничивается ДОСТИЖИМЫМ отрезком из
//! указания движка, а не нехваткой, которая приходит с запозданием и относится к цели прошлого поколения.

#![allow(clippy::unwrap_used, reason = "в тестах падение — сигнал провала")]

use camera_contract::auto_exposure::{AppliedExposure, ExposureDirective, ProductReach};

use super::tests::{applied, directive, run_frames, started_at, ts, whole_frame_measurement};
use super::*;
use crate::exposure::test_support::product;

/// Самое светлое произведение, которое умеет сенсор в этих тестах, с·ISO.
const SENSOR_BRIGHTEST: f64 = 0.00668;
/// Цель светлой сцены, достижимой сенсором (на 0,74 EV темнее предела).
const BRIGHT_SCENE: f64 = 0.004;
/// Цель тёмной сцены, недостижимой сенсором (на 1,4 EV ярче предела).
const DARK_SCENE: f64 = 0.0176;
/// Допуск уровня экрана относительно конечного, EV.
const TOLERANCE_EV: f64 = 0.05;

fn reach() -> ProductReach {
    ProductReach::Between {
        darkest: product(1.0e-4),
        brightest: product(SENSOR_BRIGHTEST),
    }
}

/// Указание: в запросе `applied_seconds_iso` с нехваткой `shortfall_ev` и пределами сенсора.
fn directive_with_reach(applied_seconds_iso: f64, shortfall_ev: f32) -> ExposureDirective {
    ExposureDirective {
        applied: Some(AppliedExposure {
            product: product(applied_seconds_iso),
            shortfall_ev,
            reach: reach(),
        }),
        ..directive()
    }
}

/// Уровень экрана кадра с произведением `frame_seconds_iso`: кадр плюс подтяжка, log2 с·ISO.
fn screen_log2(controller: &AutoExposureController, frame_seconds_iso: f64) -> f64 {
    let gain = controller.display_gain(Some(product(frame_seconds_iso)));
    frame_seconds_iso.log2() + f64::from(gain.ev())
}

#[test]
fn target_jumping_beyond_the_sensor_limit_does_not_pull_the_screen_past_the_limit() {
    let mut controller = started_at(directive_with_reach(BRIGHT_SCENE, 0.0), BRIGHT_SCENE);
    // Сцена потемнела на 2,1 EV; сенсор уже на пределе, а указание всё ещё с нехваткой 0 прошлого поколения.
    controller.ingest(&whole_frame_measurement(1, DARK_SCENE));
    let mut highest_screen = f64::MIN;
    for frame in 1..120 {
        controller.advance(ts(frame));
        highest_screen = highest_screen.max(screen_log2(&controller, SENSOR_BRIGHTEST));
    }
    assert!(
        highest_screen <= SENSOR_BRIGHTEST.log2() + TOLERANCE_EV,
        "экран ушёл выше предела сенсора на {} EV",
        highest_screen - SENSOR_BRIGHTEST.log2()
    );
}

#[test]
fn target_returning_from_beyond_the_limit_does_not_pull_the_screen_below_the_final_level() {
    // Сенсор стоял на пределе, нехватка -1,4 EV; сцена посветлела, указание ещё старое.
    let mut controller = started_at(directive_with_reach(SENSOR_BRIGHTEST, -1.4), DARK_SCENE);
    controller.ingest(&whole_frame_measurement(1, BRIGHT_SCENE));
    let mut lowest_screen = f64::MAX;
    for frame in 1..200 {
        controller.advance(ts(frame));
        // Сенсор догнал новую цель на середине (кадры 60+ уже на уровне светлой сцены).
        let frame_product = if frame < 60 { SENSOR_BRIGHTEST } else { BRIGHT_SCENE };
        lowest_screen = lowest_screen.min(screen_log2(&controller, frame_product));
    }
    assert!(
        lowest_screen >= BRIGHT_SCENE.log2() - TOLERANCE_EV,
        "экран ушёл ниже конечного уровня на {} EV",
        BRIGHT_SCENE.log2() - lowest_screen
    );
}

#[test]
fn unreachable_target_held_at_the_limit_gets_no_catch_up_gain() {
    let mut controller = started_at(directive_with_reach(SENSOR_BRIGHTEST, -1.4), DARK_SCENE);
    run_frames(&mut controller, 1, 200);
    // Недостижимая цель стоит долго: кадр на пределе показан как есть, подтяжка не «дотягивает».
    let gain = controller.display_gain(Some(product(SENSOR_BRIGHTEST)));
    assert!(gain.ev().abs() < 1e-4, "{}", gain.ev());
}

#[test]
fn reachable_target_still_gets_the_catch_up_gain_for_a_lagging_frame() {
    let controller = started_at(directive_with_reach(BRIGHT_SCENE, 0.0), BRIGHT_SCENE);
    // Кадр темнее достижимой цели на 0,3 EV: подтяжка по-прежнему закрывает разрыв.
    let gain = controller.display_gain(Some(product(BRIGHT_SCENE * (-0.3_f64).exp2())));
    assert!((f64::from(gain.ev()) - 0.3).abs() < 1e-4, "{}", gain.ev());
}

#[test]
fn unknown_reach_does_not_limit_the_target() {
    let mut controller = started_at(directive(), DARK_SCENE);
    controller.set_directive(ExposureDirective {
        applied: applied(SENSOR_BRIGHTEST, 0.0),
        ..directive()
    });
    // Предел неизвестен (вендорский AE, тесты): D = T, подтяжка упирается только в зажим ±0,5 EV.
    let gain = controller.display_gain(Some(product(SENSOR_BRIGHTEST)));
    assert!((gain.ev() - 0.5).abs() < 1e-4, "{}", gain.ev());
}

//! Белая кадра со вспышкой: постоянный цвет светодиода активной камеры из конфига (WB11, WB41).
//! Проверяется весь путь «конфиг движка → предвспышка → план, опубликованный рендеру».

use camera_contract::control::{ColorTemperature, Tint, WhiteBalanceSetting};
use camera_contract::flash::FlashUnit;
use camera_contract::ids::{CameraTarget, LensRole};

use super::flash_support::*;
use crate::config::CameraEngineConfig;
use crate::flash_config::LedWhite;
use crate::flash_status::FlashPhase;

/// Белая теста: параметры называем, чтобы не путать температуру с оттенком.
fn setting(kelvin: f64, duv: f64) -> WhiteBalanceSetting {
    WhiteBalanceSetting {
        temperature: ColorTemperature::from_kelvin(kelvin).expect("температура"),
        tint: Tint::from_duv(duv).expect("оттенок"),
    }
}

/// Конфиг с тремя РАЗНЫМИ белыми: по ним видно, чья именно попала в план.
fn config_with_distinct_whites() -> CameraEngineConfig {
    let mut config = CameraEngineConfig::default();
    let led = &mut config.three_a.flash.led_white;
    led.wide = LedWhite {
        kelvin: 3_000.0,
        duv: 0.010,
    };
    led.ultra_wide = LedWhite {
        kelvin: 5_000.0,
        duv: -0.010,
    };
    led.other = LedWhite {
        kelvin: 7_000.0,
        duv: 0.020,
    };
    config
}

/// Довести снимок на камере `target` до плана и отдать белую из плана.
fn planned_led_white(camera: &str, role: LensRole, torch: f64) -> WhiteBalanceSetting {
    let mut flash = FlashRig::on_camera(
        config_with_distinct_whites(),
        camera,
        CameraTarget::Role(role),
        |descriptor| {
            // У фронта вспышки нет; у теста «камера без своего замера» она нужна, у остальных это no-op.
            descriptor.capabilities.flash = FlashUnit::Present;
        },
    );
    flash.capture(SHOT);
    flash.publish_good_samples(8.0, torch);
    assert_eq!(flash.phase(), FlashPhase::Gap { shot: SHOT }, "план посчитан");
    flash.directive().flash.planned.expect("план опубликован").led_white
}

#[test]
fn plan_carries_the_wide_led_white_on_the_wide_camera() {
    let white = planned_led_white("2", LensRole::Wide, 4.0);
    assert_eq!(white, setting(3_000.0, 0.010));
}

#[test]
fn plan_carries_the_ultra_wide_led_white_after_the_lens_changes() {
    let white = planned_led_white("3", LensRole::UltraWide, 4.0);
    assert_eq!(
        white,
        setting(5_000.0, -0.010),
        "другая линза — другая белая, не белая основной"
    );
}

#[test]
fn camera_without_own_measurement_gets_the_other_white() {
    let white = planned_led_white("1", LensRole::Front, 4.0);
    assert_eq!(white, setting(7_000.0, 0.020));
}

#[test]
fn led_white_does_not_depend_on_the_flash_share() {
    // Доля вспышки в свете разная (фонарик почти не добавил / добавил много), белая — одна и та же:
    // по доле вспышки цвет не смешивается (WB41).
    let weak_flash = planned_led_white("2", LensRole::Wide, 7.5);
    let strong_flash = planned_led_white("2", LensRole::Wide, 1.0);
    assert_eq!(weak_flash, strong_flash);
    assert_eq!(strong_flash, setting(3_000.0, 0.010));
}

#[test]
fn default_config_plans_the_measured_white_of_the_active_camera() {
    // Умолчания — измеренные на телефоне числа: основная 3980 К / +0,0068, ширик 3815 К / +0,0035.
    let mut flash = FlashRig::measured(8.0);
    flash.capture(SHOT);
    flash.publish_good_samples(8.0, 4.0);
    let white = flash.directive().flash.planned.expect("план опубликован").led_white;
    assert_eq!(white, setting(3_980.0, 0.0068));

    let mut ultra = FlashRig::on_camera(
        CameraEngineConfig::default(),
        "3",
        CameraTarget::Role(LensRole::UltraWide),
        |_| {},
    );
    ultra.capture(SHOT);
    ultra.publish_good_samples(8.0, 4.0);
    let white = ultra.directive().flash.planned.expect("план опубликован").led_white;
    assert_eq!(white, setting(3_815.0, 0.0035));
}

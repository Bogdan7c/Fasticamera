//! Тесты расчёта кадра со вспышкой на известных числах.

use std::time::Duration;

use camera_contract::control::{ColorTemperature, Tint};

use super::*;

fn product(seconds_iso: f64) -> ExposureProduct {
    ExposureProduct::from_seconds_iso(seconds_iso).expect("произведение")
}

/// Пределы как у основной камеры фикстур при 60 к/с: ISO 100…6400, выдержка 10 мкс…1/60 с.
fn limits() -> SplitLimits {
    SplitLimits {
        iso_min: 100,
        iso_max: 6400,
        shutter_shortest: Duration::from_micros(10),
        shutter_cap: Duration::from_nanos(16_666_667),
    }
}

fn inputs(ambient: f64, torch: f64) -> FlashPlanInputs {
    FlashPlanInputs {
        shot: FlashShotId(7),
        ambient_frame: SensorTimestamp::from_nanos(123),
        ambient: product(ambient),
        torch: product(torch),
        torch_to_flash_ratio: 1.87,
        bias_ev: 0.0,
        limits: limits(),
        led_white: WhiteBalanceSetting {
            temperature: ColorTemperature::from_kelvin(4_200.0).expect("температура"),
            tint: Tint::from_duv(0.004).expect("оттенок"),
        },
    }
}

#[test]
fn torch_doubling_the_light_gives_flash_light_of_ratio_added_to_ambient() {
    // Фон: свет 1, с фонариком: свет 2 → вклад фонарика 1, вклад вспышки 1,87, всего 2,87.
    let planned = plan_flash_frame(&inputs(1.0, 0.5)).expect("план");
    let expected_product = 1.0 / 2.87;
    assert!((planned.plan.mid_gray_product.seconds_iso() - expected_product).abs() < 1e-9);
    assert!((planned.plan.flash_share.value() - 1.87 / 2.87).abs() < 1e-9);
    // 0,3484 с·ISO при ISO 100 — выдержка 3,484 мс.
    assert_eq!(planned.iso.value(), 100);
    let shutter_ms = planned.exposure_time.duration().as_secs_f64() * 1000.0;
    assert!((shutter_ms - 3.484).abs() < 0.01, "выдержка {shutter_ms} мс");
    assert_eq!(planned.plan.shot, FlashShotId(7));
    // Кадр, по которому посчитан свет сцены, доезжает до плана для рендера.
    assert_eq!(planned.plan.ambient_frame, SensorTimestamp::from_nanos(123));
}

#[test]
fn plan_carries_the_given_led_white_untouched_whatever_the_flash_share() {
    // Белая светодиода — вход плана как есть: ни доля вспышки, ни свет сцены её не смешивают (WB41).
    let expected = inputs(1.0, 0.5).led_white;
    for (ambient, torch) in [(1.0, 0.5), (0.8, 0.8), (8.0, 0.5)] {
        let planned = plan_flash_frame(&inputs(ambient, torch)).expect("план");
        assert_eq!(planned.plan.led_white, expected, "фон {ambient}, фонарик {torch}");
    }
    let mut other_white = inputs(1.0, 0.5);
    other_white.led_white = WhiteBalanceSetting {
        temperature: ColorTemperature::from_kelvin(3_300.0).expect("температура"),
        tint: Tint::NEUTRAL,
    };
    let planned = plan_flash_frame(&other_white).expect("план");
    assert_eq!(planned.plan.led_white, other_white.led_white);
}

#[test]
fn torch_that_adds_nothing_leaves_the_scene_light_and_zero_share() {
    // Замер с фонариком не ярче фона (дальний объект): вспышка ничего не добавит.
    let planned = plan_flash_frame(&inputs(0.8, 0.8)).expect("план");
    assert!((planned.plan.mid_gray_product.seconds_iso() - 0.8).abs() < 1e-9);
    assert_eq!(planned.plan.flash_share.value(), 0.0);
    // Замер с фонариком ТЕМНЕЕ фона — тоже ноль, а не отрицательный вклад.
    let noisy = plan_flash_frame(&inputs(0.8, 1.2)).expect("план");
    assert!((noisy.plan.mid_gray_product.seconds_iso() - 0.8).abs() < 1e-9);
    assert_eq!(noisy.plan.flash_share.value(), 0.0);
}

#[test]
fn user_bias_scales_the_product_but_not_the_flash_share() {
    let neutral = plan_flash_frame(&inputs(1.0, 0.5)).expect("план");
    let mut brighter_inputs = inputs(1.0, 0.5);
    brighter_inputs.bias_ev = 1.0;
    let brighter = plan_flash_frame(&brighter_inputs).expect("план");
    let ratio = brighter.plan.mid_gray_product.seconds_iso() / neutral.plan.mid_gray_product.seconds_iso();
    assert!((ratio - 2.0).abs() < 1e-9, "+1 EV удваивает произведение");
    assert_eq!(brighter.plan.flash_share, neutral.plan.flash_share);
}

#[test]
fn very_bright_scene_hits_the_shortest_shutter_at_minimum_iso() {
    // Произведение 1e-4 с·ISO: при ISO 100 нужна выдержка 1 мкс, короче предела камеры 10 мкс.
    let planned = plan_flash_frame(&inputs(1.0e-4, 1.0e-4)).expect("план");
    assert_eq!(planned.exposure_time.duration(), Duration::from_micros(10));
    assert_eq!(planned.iso.value(), 100);
}

#[test]
fn dark_scene_is_capped_by_the_frame_duration_and_paid_with_iso() {
    // Свет 1/60 с·ISO·… : нужно 8 с·ISO — выдержка упрётся в кадр 1/60 с, остальное платит ISO.
    let planned = plan_flash_frame(&inputs(8.0, 8.0)).expect("план");
    assert_eq!(planned.exposure_time.duration(), Duration::from_nanos(16_666_667));
    assert_eq!(planned.iso.value(), 480);
}

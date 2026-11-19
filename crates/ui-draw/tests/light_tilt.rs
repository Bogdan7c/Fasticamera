#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::field_reassign_with_default
)]
//! Свет, следящий за наклоном телефона.

use shell_contract::clock::BoottimeNanos;
use shell_contract::gravity::{GravityReading, GravitySample, GravityVector};
use ui_draw::light::{LightDirection, LightError, TiltLight, TiltLightConfig, TiltUpdate};

const G: f32 = 9.81;
const REST_DEG: f32 = 45.0;

fn light() -> TiltLight {
    TiltLight::new(TiltLightConfig::default(), LightDirection::from_angle_degrees(REST_DEG)).unwrap()
}

fn sample(time_ms: i64, x: f32, y: f32, z: f32) -> GravitySample {
    GravitySample {
        time: BoottimeNanos::from_nanos(time_ms * 1_000_000),
        vector: GravityVector { x, y, z },
    }
}

/// Показание телефона, повёрнутого по часовой стрелке (верх вправо) на `roll_deg` в портрете.
fn rolled(time_ms: i64, roll_deg: f32) -> GravitySample {
    let (sin, cos) = roll_deg.to_radians().sin_cos();
    sample(time_ms, -G * sin, G * cos, 0.0)
}

#[test]
fn without_readings_light_stays_at_rest() {
    let mut light = light();
    assert_eq!(
        light.apply_reading(GravityReading::NoSampleYet),
        TiltUpdate::NoSampleYet
    );
    assert!((light.direction().angle_degrees() - REST_DEG).abs() < 1e-4);
}

#[test]
fn upright_phone_keeps_rest_direction() {
    let mut light = light();
    light.apply_sample(rolled(0, 0.0));
    assert!((light.direction().angle_degrees() - REST_DEG).abs() < 1e-3);
}

#[test]
fn tilting_right_turns_the_light_counterclockwise_on_screen_and_stays_world_fixed() {
    let mut light = light();
    light.apply_sample(rolled(0, 0.0));
    // 3 секунды при τ = 0,12 с: сглаживание практически закончено.
    for ms in (10..=3000).step_by(10) {
        light.apply_sample(rolled(ms, 30.0));
    }
    // Крен 30° по часовой: угол света на экране 45° − 30° = 15° (свет остался на месте, уехал экран).
    let angle = light.direction().angle_degrees();
    assert!((angle - 15.0).abs() < 0.1, "угол {angle}");
    // Влево — в другую сторону.
    for ms in (3010..=6000).step_by(10) {
        light.apply_sample(rolled(ms, -30.0));
    }
    let angle = light.direction().angle_degrees();
    assert!((angle - 75.0).abs() < 0.1, "угол {angle}");
}

#[test]
fn tilt_is_clamped_and_flat_phone_returns_to_rest() {
    let mut light = light();
    light.apply_sample(rolled(0, 0.0));
    for ms in (10..=3000).step_by(10) {
        light.apply_sample(rolled(ms, 80.0));
    }
    let max = TiltLightConfig::default().max_tilt_deg;
    assert!(
        (light.direction().angle_degrees() - (REST_DEG - max)).abs() < 0.1,
        "{}",
        light.direction().angle_degrees()
    );
    // Телефон лёг экраном вверх: крен не определён → свет плавно возвращается в покой.
    for ms in (3010..=6000).step_by(10) {
        light.apply_sample(sample(ms, 0.2, 0.3, G));
    }
    assert!((light.direction().angle_degrees() - REST_DEG).abs() < 0.1);
}

/// Детерминированный «шум» без внешнего генератора: сумма синусов разной частоты, амплитуда ≈ 1.
fn noise(index: i64) -> f32 {
    let t = index as f32;
    ((t * 12.9898).sin() * 0.6 + (t * 78.233).sin() * 0.4).clamp(-1.0, 1.0)
}

#[test]
fn still_phone_with_sensor_noise_does_not_make_the_light_jitter() {
    let mut smoothed = light();
    let raw_target = |index: i64| rolled(index * 10, 0.0).vector.x + 0.3 * noise(index);
    // Сырой шум: отклонение направления «вверх мира» от вертикали до 0,3/9,81 рад ≈ 1,75°.
    let mut raw_max = 0.0_f32;
    let mut smoothed_max = 0.0_f32;
    smoothed.apply_sample(sample(0, 0.0, G, 0.0));
    for index in 1..=400_i64 {
        let x = raw_target(index);
        let y = G + 0.3 * noise(index + 1000);
        smoothed.apply_sample(sample(index * 10, x, y, 0.0)); // 100 Гц
        raw_max = raw_max.max((-x).atan2(y).to_degrees().abs());
        if index > 50 {
            smoothed_max = smoothed_max.max((smoothed.direction().angle_degrees() - REST_DEG).abs());
        }
    }
    assert!(raw_max > 1.0, "тест вырожден: шум слишком мал ({raw_max}°)");
    assert!(
        smoothed_max < 0.5,
        "свет дрожит на {smoothed_max}° при сыром шуме {raw_max}°"
    );
}

#[test]
fn sixty_and_one_twenty_hertz_give_the_same_angle_at_equal_time() {
    let run = |period_us: i64| {
        let mut light = light();
        light.apply_sample(rolled(0, 0.0));
        let mut time_us = period_us;
        while time_us <= 1_000_000 {
            let mut reading = rolled(0, 40.0);
            reading.time = BoottimeNanos::from_nanos(time_us * 1000);
            light.apply_sample(reading);
            time_us += period_us;
        }
        light.direction().angle_degrees()
    };
    // 60 Гц: 1 000 000 / 60 ≈ 16 667 мкс; берём периоды, дающие ровно 1 с суммарно.
    let at_60 = run(1_000_000 / 50); // 50 Гц: ровные 20 мс
    let at_120 = run(1_000_000 / 125); // 125 Гц: ровные 8 мс
    assert!((at_60 - at_120).abs() < 1e-3, "{at_60} против {at_120}");
    // И результат — не «снап» к цели и не «покой»: за 1 с при τ=0,12 пройдено 1−e^(−1/0,12)≈99,98 %,
    // а за 0,1 с — лишь 56 %; проверим по короткому окну.
    let mut short = light();
    short.apply_sample(rolled(0, 0.0));
    for ms in (10..=100).step_by(10) {
        short.apply_sample(rolled(ms, 40.0));
    }
    let progress = (REST_DEG - short.direction().angle_degrees()) / 40.0;
    assert!(
        (progress - (1.0 - (-0.1_f32 / 0.12).exp())).abs() < 1e-3,
        "доля пути {progress}"
    );
}

#[test]
fn duplicate_and_backwards_time_are_not_smoothed_and_are_counted() {
    let mut light = light();
    light.apply_sample(rolled(100, 0.0));
    assert_eq!(light.apply_sample(rolled(100, 30.0)), TiltUpdate::DuplicateSample);
    assert!(
        (light.direction().angle_degrees() - REST_DEG).abs() < 1e-4,
        "дубликат ничего не двигает"
    );
    assert_eq!(light.apply_sample(rolled(50, 30.0)), TiltUpdate::TimeWentBackwards);
    assert_eq!(light.time_regressions(), 1);
    assert!(
        (light.direction().angle_degrees() - REST_DEG).abs() < 1e-4,
        "обратное время ничего не двигает"
    );
    // После сбоя сглаживание продолжается от новой точки отсчёта.
    assert_eq!(light.apply_sample(rolled(60, 30.0)), TiltUpdate::Smoothed);
    assert!(light.direction().angle_degrees() < REST_DEG);
}

#[test]
fn invalid_config_is_rejected() {
    let bad = |config: TiltLightConfig| TiltLight::new(config, LightDirection::from_angle_degrees(45.0)).unwrap_err();
    let base = TiltLightConfig::default();
    assert_eq!(
        bad(TiltLightConfig {
            max_tilt_deg: 0.0,
            ..base
        }),
        LightError::InvalidMaxTilt(0.0)
    );
    assert_eq!(
        bad(TiltLightConfig {
            smoothing_time_constant_s: 0.0,
            ..base
        }),
        LightError::InvalidTimeConstant(0.0)
    );
    assert_eq!(
        bad(TiltLightConfig {
            flat_threshold: 1.0,
            ..base
        }),
        LightError::InvalidFlatThreshold(1.0)
    );
}

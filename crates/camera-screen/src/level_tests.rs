//! Тесты уровня и поворота значков.

use shell_contract::gravity::GravityVector;
use shell_contract::orientation::DeviceOrientation;

use crate::level::{LevelConfig, LevelConfigError, LevelReading, level_reading};
use crate::orientation::icon_rotation_clockwise_degrees;

const G: f32 = 9.81;

/// Гравитация для телефона, повёрнутого в плоскости экрана на `roll` градусов по часовой.
fn gravity_for_roll(roll_degrees: f32) -> GravityVector {
    let roll = roll_degrees.to_radians();
    GravityVector {
        x: -G * roll.sin(),
        y: G * roll.cos(),
        z: 0.0,
    }
}

fn tilted(reading: LevelReading) -> (f32, f32) {
    match reading {
        LevelReading::Tilted {
            roll_clockwise_degrees,
            deviation_degrees,
        } => (roll_clockwise_degrees, deviation_degrees),
        other => panic!("ожидался наклонённый телефон, получено {other:?}"),
    }
}

#[test]
fn upright_phone_has_zero_roll_and_zero_deviation() {
    let (roll, deviation) = tilted(level_reading(gravity_for_roll(0.0), &LevelConfig::default()));
    assert!(roll.abs() < 1e-4 && deviation.abs() < 1e-4);
}

#[test]
fn roll_is_positive_clockwise_and_matches_the_input_angle() {
    for angle in [-170.0_f32, -100.0, -30.0, 10.0, 44.0, 90.0, 135.0] {
        let (roll, _) = tilted(level_reading(gravity_for_roll(angle), &LevelConfig::default()));
        assert!((roll - angle).abs() < 1e-2, "ожидали {angle}, получили {roll}");
    }
}

#[test]
fn deviation_is_measured_from_the_nearest_axis() {
    let config = LevelConfig::default();
    let cases = [
        (0.0, 0.0),
        (10.0, 10.0),
        (-10.0, -10.0),
        (80.0, -10.0),
        (100.0, 10.0),
        (-100.0, -10.0),
        (180.0, 0.0),
    ];
    for (roll, expected) in cases {
        let (_, deviation) = tilted(level_reading(gravity_for_roll(roll), &config));
        assert!(
            (deviation - expected).abs() < 1e-2,
            "крен {roll}: ожидали {expected}, получили {deviation}"
        );
    }
}

#[test]
fn landscape_right_phone_reads_plus_90_per_gravity_axes_doc() {
    // LandscapeRight: верх телефона смотрит вправо, «вверх мира» в осях телефона — (−g, 0, 0).
    let gravity = GravityVector { x: -G, y: 0.0, z: 0.0 };
    let (roll, deviation) = tilted(level_reading(gravity, &LevelConfig::default()));
    assert!((roll - 90.0).abs() < 1e-3 && deviation.abs() < 1e-3);
}

#[test]
fn phone_lying_flat_shows_no_level() {
    let config = LevelConfig::default();
    assert_eq!(
        level_reading(GravityVector { x: 0.0, y: 0.0, z: G }, &config),
        LevelReading::Flat
    );
    assert_eq!(
        level_reading(GravityVector { x: 0.0, y: 0.0, z: -G }, &config),
        LevelReading::Flat
    );
    // Граница порога 20°: 19° от нормали — ещё «плашмя», 21° — уже стоит.
    let near = |degrees: f32| GravityVector {
        x: 0.0,
        y: G * degrees.to_radians().sin(),
        z: G * degrees.to_radians().cos(),
    };
    assert_eq!(level_reading(near(19.0), &config), LevelReading::Flat);
    assert!(matches!(
        level_reading(near(21.0), &config),
        LevelReading::Tilted { .. }
    ));
}

#[test]
fn unusable_gravity_vectors_are_unknown_not_a_guess() {
    let config = LevelConfig::default();
    assert_eq!(
        level_reading(GravityVector { x: 0.0, y: 0.0, z: 0.0 }, &config),
        LevelReading::Unknown
    );
    assert_eq!(
        level_reading(
            GravityVector {
                x: f32::NAN,
                y: 1.0,
                z: 0.0
            },
            &config
        ),
        LevelReading::Unknown
    );
}

#[test]
fn invalid_flat_threshold_is_rejected() {
    for bad in [0.0, 90.0, -1.0, f32::NAN] {
        let config = LevelConfig {
            flat_threshold_degrees: bad,
        };
        assert!(matches!(config.validate(), Err(LevelConfigError(_))), "{bad}");
    }
    assert!(LevelConfig::default().validate().is_ok());
}

#[test]
fn icons_counter_rotate_against_the_phone() {
    assert_eq!(icon_rotation_clockwise_degrees(DeviceOrientation::Portrait), 0.0);
    // Телефон повернули против часовой (верх влево) — значки докручиваются по часовой.
    assert_eq!(icon_rotation_clockwise_degrees(DeviceOrientation::LandscapeLeft), 90.0);
    assert_eq!(
        icon_rotation_clockwise_degrees(DeviceOrientation::LandscapeRight),
        -90.0
    );
    assert_eq!(
        icon_rotation_clockwise_degrees(DeviceOrientation::PortraitUpsideDown),
        180.0
    );
}

use super::*;

fn ring(value: f32) -> LookKnob {
    LookKnob::new(value).unwrap()
}

#[test]
fn push_inside_and_on_the_circle_is_accepted() {
    let inside = WheelPush::new(0.3, -0.4).unwrap();
    assert_eq!((inside.a(), inside.b()), (0.3, -0.4));
    assert!((inside.strength() - 0.5).abs() < 1e-6);
    assert!(WheelPush::new(1.0, 0.0).is_ok());
    assert!(WheelPush::new(0.0, -1.0).is_ok());
}

#[test]
fn push_outside_circle_reports_strength() {
    match WheelPush::new(0.8, 0.8) {
        Err(WheelPushError::OutsideCircle { strength }) => assert!((strength - 1.131_370_9).abs() < 1e-5),
        other => panic!("ожидали OutsideCircle, получили {other:?}"),
    }
}

#[test]
fn push_rejects_non_finite() {
    for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        assert_eq!(WheelPush::new(bad, 0.0), Err(WheelPushError::NotANumber));
        assert_eq!(WheelPush::new(0.0, bad), Err(WheelPushError::NotANumber));
        assert_eq!(WheelPush::from_hue_degrees(bad, 0.5), Err(WheelPushError::NotANumber));
        assert_eq!(WheelPush::from_hue_degrees(10.0, bad), Err(WheelPushError::NotANumber));
    }
}

#[test]
fn negative_zero_is_the_one_canonical_neutral() {
    let negative = WheelPush::new(-0.0, -0.0).unwrap();
    assert!(negative.is_neutral());
    // Одна запись нейтрали и в файле: «-0.0» не должен попасть на диск.
    assert_eq!(serde_json::to_string(&negative).unwrap(), "[0.0,0.0]");
    assert!(negative.a().is_sign_positive() && negative.b().is_sign_positive());
}

#[test]
fn zero_strength_is_neutral_at_any_hue() {
    for hue in [0.0, 37.0, 180.0, 359.0, -720.0] {
        assert_eq!(WheelPush::from_hue_degrees(hue, 0.0), Ok(WheelPush::NEUTRAL));
    }
}

#[test]
fn hue_and_strength_land_on_expected_axes() {
    // 90° = к жёлтому (+b), 265° ≈ к синему (−b, чуть −a).
    let yellow = WheelPush::from_hue_degrees(90.0, 0.5).unwrap();
    assert!(yellow.a().abs() < 1e-6 && (yellow.b() - 0.5).abs() < 1e-6, "{yellow:?}");
    let blue = WheelPush::from_hue_degrees(265.0, 1.0).unwrap();
    assert!(blue.b() < -0.99 && blue.a() < 0.0, "{blue:?}");
}

#[test]
fn full_strength_at_every_hue_stays_inside_circle() {
    // Округление cos/sin не должно рождать «вне круга» на полной силе (иначе экран получил бы отказ).
    for step in 0..3600 {
        let hue = step as f32 * 0.1;
        let push = WheelPush::from_hue_degrees(hue, 1.0).unwrap();
        assert!(push.a() * push.a() + push.b() * push.b() <= 1.0, "{hue}");
        // И то же значение проходит проверку чтения из файла.
        let json = serde_json::to_string(&push).unwrap();
        assert_eq!(serde_json::from_str::<WheelPush>(&json).unwrap(), push);
    }
}

#[test]
fn clamped_inside_point_is_kept_as_is() {
    let push = WheelPush::clamped_to_circle(0.3, -0.4).unwrap();
    assert_eq!((push.a(), push.b()), (0.3, -0.4));
    assert_eq!(WheelPush::clamped_to_circle(0.0, 0.0).unwrap(), WheelPush::NEUTRAL);
}

#[test]
fn clamped_negative_zero_becomes_the_canonical_neutral() {
    let push = WheelPush::clamped_to_circle(-0.0, -0.0).unwrap();
    assert!(push.a().is_sign_positive() && push.b().is_sign_positive());
    assert_eq!(serde_json::to_string(&push).unwrap(), "[0.0,0.0]");
}

#[test]
fn clamped_point_exactly_on_the_edge_is_kept() {
    for (a, b) in [(1.0, 0.0), (0.0, -1.0), (-1.0, 0.0)] {
        let push = WheelPush::clamped_to_circle(a, b).unwrap();
        assert_eq!((push.a(), push.b()), (a, b));
    }
}

#[test]
fn clamped_point_outside_lands_on_the_edge_with_the_same_direction() {
    // Любые направления, включая диагонали и оси; за краем в 1.1x, 3x и 100x.
    for degrees in (0..360).step_by(7) {
        let (sin, cos) = (degrees as f32).to_radians().sin_cos();
        for scale in [1.1_f32, 3.0, 100.0] {
            let push = WheelPush::clamped_to_circle(cos * scale, sin * scale).unwrap();
            assert!(push.a() * push.a() + push.b() * push.b() <= 1.0, "{degrees}° x{scale}");
            assert!((push.strength() - 1.0).abs() < 1e-5, "на краю: {degrees}° x{scale}");
            assert!(
                (push.a() - cos).abs() < 1e-5 && (push.b() - sin).abs() < 1e-5,
                "угол: {degrees}° x{scale}"
            );
        }
    }
}

#[test]
fn clamped_huge_values_do_not_overflow_and_keep_the_direction() {
    // Квадрат 1e30 в f32 переполняется в бесконечность: прижатие обязано пережить это.
    let push = WheelPush::clamped_to_circle(3.0e30, 4.0e30).unwrap();
    assert!((push.a() - 0.6).abs() < 1e-5 && (push.b() - 0.8).abs() < 1e-5);
    let largest = WheelPush::clamped_to_circle(f32::MAX, -f32::MAX).unwrap();
    assert!(largest.strength() <= 1.0 && (largest.a() + largest.b()).abs() < 1e-5);
    assert!(largest.a() > 0.0 && largest.b() < 0.0);
}

#[test]
fn clamped_rejects_non_finite() {
    for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        assert_eq!(WheelPush::clamped_to_circle(bad, 0.0), Err(WheelPushError::NotANumber));
        assert_eq!(WheelPush::clamped_to_circle(0.5, bad), Err(WheelPushError::NotANumber));
    }
}

#[test]
fn clamped_point_survives_serde_roundtrip() {
    let push = WheelPush::clamped_to_circle(5.0, 5.0).unwrap();
    let json = serde_json::to_string(&push).unwrap();
    assert_eq!(serde_json::from_str::<WheelPush>(&json).unwrap(), push);
}

#[test]
fn strength_outside_unit_range_is_rejected() {
    assert!(matches!(
        WheelPush::from_hue_degrees(0.0, 1.01),
        Err(WheelPushError::OutsideCircle { .. })
    ));
    assert!(matches!(
        WheelPush::from_hue_degrees(0.0, -0.1),
        Err(WheelPushError::OutsideCircle { .. })
    ));
}

#[test]
fn push_serde_roundtrip_and_rejects_bad_shapes() {
    let push = WheelPush::new(0.25, -0.5).unwrap();
    let json = serde_json::to_string(&push).unwrap();
    assert_eq!(json, "[0.25,-0.5]");
    assert_eq!(serde_json::from_str::<WheelPush>(&json).unwrap(), push);
    let error = serde_json::from_str::<WheelPush>("[0.9,0.9]").unwrap_err();
    assert!(error.to_string().contains("вне единичного круга"), "{error}");
    assert!(serde_json::from_str::<WheelPush>("[0.1]").is_err());
    assert!(serde_json::from_str::<WheelPush>("{\"a\":0.1,\"b\":0}").is_err());
}

#[test]
fn wheel_is_neutral_only_when_both_knobs_are_zero() {
    assert!(ColorWheel::NEUTRAL.is_neutral());
    let pushed = ColorWheel {
        push: WheelPush::new(0.1, 0.0).unwrap(),
        ..ColorWheel::NEUTRAL
    };
    let ringed = ColorWheel {
        ring: ring(-0.2),
        ..ColorWheel::NEUTRAL
    };
    assert!(!pushed.is_neutral());
    assert!(!ringed.is_neutral());
}

#[test]
fn with_wheel_replaces_exactly_one_zone() {
    let touched = ColorWheel {
        ring: ring(0.5),
        ..ColorWheel::NEUTRAL
    };
    for zone in WheelZone::ALL {
        let wheels = ColorWheels::NEUTRAL.with_wheel(zone, touched);
        assert!(!wheels.is_neutral(), "{zone:?}");
        for other in WheelZone::ALL {
            let expected = if other == zone { touched } else { ColorWheel::NEUTRAL };
            assert_eq!(wheels.wheel(other), expected, "{zone:?} → {other:?}");
        }
    }
    assert!(ColorWheels::NEUTRAL.is_neutral());
    assert_eq!(ColorWheels::default(), ColorWheels::NEUTRAL);
}

#[test]
fn wheels_serde_roundtrip_and_strict_shape() {
    let wheels = ColorWheels::NEUTRAL
        .with_wheel(
            WheelZone::Lift,
            ColorWheel {
                push: WheelPush::from_hue_degrees(265.0, 0.6).unwrap(),
                ring: ring(-0.1),
            },
        )
        .with_wheel(
            WheelZone::Gain,
            ColorWheel {
                push: WheelPush::from_hue_degrees(60.0, 0.4).unwrap(),
                ring: ring(0.2),
            },
        );
    let json = serde_json::to_string(&wheels).unwrap();
    assert_eq!(serde_json::from_str::<ColorWheels>(&json).unwrap(), wheels);
    // Нет колеса — ошибка: умолчаний в словаре нет, старый файл переводит миграция схемы (look-presets).
    let missing = r#"{"lift":{"push":[0,0],"ring":0},"gamma":{"push":[0,0],"ring":0},
        "gain":{"push":[0,0],"ring":0}}"#;
    assert!(serde_json::from_str::<ColorWheels>(missing).is_err());
    let unknown = r#"{"push":[0,0],"ring":0,"spin":1}"#;
    assert!(serde_json::from_str::<ColorWheel>(unknown).is_err());
}

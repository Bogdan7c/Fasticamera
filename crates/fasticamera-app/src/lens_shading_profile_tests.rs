use camera_contract::geometry::PixelSize;
use camera_contract::ids::CameraId;
use camera_contract::lens_shading::{LensShadingCalibrationState, LensShadingCalibrations, NotCalibratedReason};
use serde_json::{Value, json};

use super::{LensShadingProfileError, embedded_lens_shading, parse_lens_shading_profile};
use crate::device_config::DeviceConfig;

fn oneplus() -> DeviceConfig {
    DeviceConfig {
        make: Some("OnePlus".to_owned()),
        model: Some("CPH2447".to_owned()),
    }
}

fn camera(id: &str) -> CameraId {
    CameraId::new(id).unwrap()
}

/// Корректный профиль из двух камер с сеткой 2×2 (поля провенанса — свободной формы).
fn sample_profile() -> Value {
    let points = json!([
        [1.0, 1.1, 1.2, 1.3],
        [2.0, 2.1, 2.2, 2.3],
        [3.0, 3.1, 3.2, 3.3],
        [4.0, 4.1, 4.2, 4.3]
    ]);
    json!({
        "schema": "fasticamera.lens-shading-calibration/1",
        "device": {"manufacturer": "OnePlus", "model": "CPH2447"},
        "cameras": [
            {
                "camera_id": "2",
                "active_array": {"width": 4096, "height": 3072},
                "grid": {"columns": 2, "rows": 2, "points": points},
                "provenance": {"anything": [1, 2, {"nested": true}]}
            },
            {
                "camera_id": "3",
                "active_array": {"width": 4000, "height": 3000},
                "grid": {"columns": 2, "rows": 2, "points": points}
            }
        ]
    })
}

fn parse(profile: &Value, device: &DeviceConfig) -> Result<LensShadingCalibrations, LensShadingProfileError> {
    parse_lens_shading_profile(&profile.to_string(), device)
}

#[test]
fn sample_profile_yields_calibrations_attached_to_the_matching_cameras() {
    let calibrations = parse(&sample_profile(), &oneplus()).unwrap();
    let state = calibrations.state_for(&camera("2"), PixelSize::new(4096, 3072));
    let attached = state.calibration().expect("камера 2 откалибрована");
    assert_eq!(
        attached.grid.points()[2],
        [3.0, 3.1, 3.2, 3.3],
        "точки пришли из файла как есть"
    );
    assert_eq!(attached.active_array, PixelSize::new(4096, 3072));
    assert!(
        calibrations
            .state_for(&camera("3"), PixelSize::new(4000, 3000))
            .calibration()
            .is_some()
    );
}

#[test]
fn manufacturer_is_matched_case_insensitively_and_model_exactly() {
    let shouting = DeviceConfig {
        make: Some(" ONEPLUS ".to_owned()),
        model: Some("CPH2447".to_owned()),
    };
    assert!(!parse(&sample_profile(), &shouting).unwrap().is_empty());
    let other_model = DeviceConfig {
        make: Some("OnePlus".to_owned()),
        model: Some("CPH2449".to_owned()),
    };
    assert!(parse(&sample_profile(), &other_model).unwrap().is_empty());
}

#[test]
fn another_or_unknown_device_gets_no_calibration_with_the_reason() {
    for device in [
        DeviceConfig {
            make: Some("Google".to_owned()),
            model: Some("Pixel 9".to_owned()),
        },
        DeviceConfig::default(),
    ] {
        let calibrations = parse(&sample_profile(), &device).unwrap();
        assert!(calibrations.is_empty());
        assert_eq!(
            calibrations.state_for(&camera("2"), PixelSize::new(4096, 3072)),
            LensShadingCalibrationState::NotCalibrated(NotCalibratedReason::DeviceNotInProfile)
        );
    }
}

#[test]
fn wrong_schema_is_rejected_by_name() {
    let mut profile = sample_profile();
    profile["schema"] = json!("fasticamera.lens-shading-calibration/2");
    assert_eq!(
        parse(&profile, &oneplus()),
        Err(LensShadingProfileError::UnsupportedSchema {
            found: "fasticamera.lens-shading-calibration/2".to_owned()
        })
    );
}

#[test]
fn broken_profile_fails_even_for_another_device() {
    let mut profile = sample_profile();
    profile["cameras"][0]["grid"]["points"] = json!([[1.0, 1.0, 1.0, 1.0]]);
    let foreign = DeviceConfig {
        make: Some("Google".to_owned()),
        model: Some("Pixel 9".to_owned()),
    };
    for device in [oneplus(), foreign] {
        let error = parse(&profile, &device).unwrap_err();
        assert!(
            matches!(&error, LensShadingProfileError::InvalidGrid { camera, .. } if camera == "2"),
            "{error:?}"
        );
    }
}

#[test]
fn gain_below_one_and_missing_fields_and_garbage_are_explicit_errors() {
    let mut profile = sample_profile();
    profile["cameras"][1]["grid"]["points"][1] = json!([1.0, 0.5, 1.0, 1.0]);
    assert!(matches!(
        parse(&profile, &oneplus()),
        Err(LensShadingProfileError::InvalidGrid { camera, .. }) if camera == "3"
    ));

    let mut profile = sample_profile();
    profile["cameras"][0]["grid"]["points"][0] = json!([1.0, 1.0, 1.0]);
    assert!(matches!(
        parse(&profile, &oneplus()),
        Err(LensShadingProfileError::Malformed(_))
    ));

    assert!(matches!(
        parse_lens_shading_profile("not json", &oneplus()),
        Err(LensShadingProfileError::Malformed(_))
    ));
}

#[test]
fn unknown_fields_are_rejected_except_inside_provenance() {
    let mut profile = sample_profile();
    profile["cameras"][0]["extra_field"] = json!(1);
    assert!(matches!(
        parse(&profile, &oneplus()),
        Err(LensShadingProfileError::Malformed(_))
    ));
    let mut profile = sample_profile();
    profile["surprise"] = json!(true);
    assert!(matches!(
        parse(&profile, &oneplus()),
        Err(LensShadingProfileError::Malformed(_))
    ));
    // Внутри provenance что угодно допустимо (sample_profile уже кладёт туда вложенный мусор).
    assert!(parse(&sample_profile(), &oneplus()).is_ok());
}

#[test]
fn duplicate_empty_and_degenerate_cameras_are_rejected() {
    let mut profile = sample_profile();
    profile["cameras"][1]["camera_id"] = json!("2");
    assert_eq!(
        parse(&profile, &oneplus()),
        Err(LensShadingProfileError::DuplicateCamera { camera: "2".to_owned() })
    );
    let mut profile = sample_profile();
    profile["cameras"][0]["camera_id"] = json!("");
    assert!(matches!(
        parse(&profile, &oneplus()),
        Err(LensShadingProfileError::EmptyCameraId(_))
    ));
    let mut profile = sample_profile();
    profile["cameras"][0]["active_array"]["width"] = json!(0);
    assert!(matches!(
        parse(&profile, &oneplus()),
        Err(LensShadingProfileError::EmptyActiveArray { width: 0, .. })
    ));
}

#[test]
fn embedded_file_gives_cameras_2_3_and_1_for_the_oneplus_profile() {
    let calibrations = embedded_lens_shading(&oneplus()).unwrap();
    let ids: Vec<&str> = calibrations.cameras().map(CameraId::as_str).collect();
    assert_eq!(ids, ["2", "3", "1"]);
    for (id, size) in [
        ("2", PixelSize::new(4096, 3072)),
        ("3", PixelSize::new(4000, 3000)),
        ("1", PixelSize::new(2304, 1728)),
    ] {
        let state = calibrations.state_for(&camera(id), size);
        let calibration = state
            .calibration()
            .unwrap_or_else(|| panic!("камера {id} откалибрована: {state:?}"));
        assert_eq!(
            (calibration.grid.columns(), calibration.grid.rows()),
            (33, 25),
            "камера {id}"
        );
        assert_eq!(calibration.grid.points().len(), 33 * 25);
        // Калибровка поднимает затенённые места, а самое светлое место канала даёт 1.
        let darkest_corner_gain = calibration.grid.points()[0][0];
        assert!(
            darkest_corner_gain > 2.0,
            "камера {id}: угол виден затенённым ({darkest_corner_gain})"
        );
    }
}

#[test]
fn embedded_file_is_not_applied_to_a_different_phone() {
    let pixel = DeviceConfig {
        make: Some("Google".to_owned()),
        model: Some("Pixel 9".to_owned()),
    };
    let calibrations = embedded_lens_shading(&pixel).unwrap();
    assert!(calibrations.is_empty());
    assert_eq!(
        calibrations.state_for(&camera("2"), PixelSize::new(4096, 3072)),
        LensShadingCalibrationState::NotCalibrated(NotCalibratedReason::DeviceNotInProfile)
    );
}

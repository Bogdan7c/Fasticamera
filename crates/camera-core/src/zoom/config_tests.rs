//! Проверка значений конфига зума: NaN, бесконечности и отрицательные числа не проходят.

use super::{ZoomConfig, ZoomConfigError};

fn broken(change: impl FnOnce(&mut ZoomConfig)) -> ZoomConfigError {
    let mut config = ZoomConfig::default();
    change(&mut config);
    config.validate().expect_err("конфиг должен быть отклонён")
}

#[test]
fn defaults_follow_the_owner_decisions_and_validate() {
    let config = ZoomConfig::default();
    assert_eq!(config.validate(), Ok(()));
    assert_eq!(config.digital_crop_stops, vec![2.0]);
    assert_eq!(config.back_max_ratio, 10.0);
    assert_eq!(config.front_max_ratio, 3.0);
    assert_eq!(config.calibration_max_principal_point_offset, 0.15);
}

#[test]
fn non_finite_and_negative_values_are_rejected() {
    for bad in [f32::NAN, f32::INFINITY, -1.0] {
        assert!(matches!(
            broken(|c| c.calibration_max_principal_point_offset = bad),
            ZoomConfigError::InvalidCalibrationOffset(_)
        ));
        assert!(matches!(
            broken(|c| c.switch_up_hysteresis = bad),
            ZoomConfigError::InvalidHysteresis(_)
        ));
        assert!(matches!(
            broken(|c| c.stop_dedupe_tolerance = bad),
            ZoomConfigError::InvalidDedupeTolerance(_)
        ));
        assert!(matches!(
            broken(|c| c.back_max_ratio = bad),
            ZoomConfigError::InvalidMaxRatio {
                field: "back_max_ratio",
                ..
            }
        ));
        assert!(matches!(
            broken(|c| c.front_max_ratio = bad),
            ZoomConfigError::InvalidMaxRatio {
                field: "front_max_ratio",
                ..
            }
        ));
        assert!(matches!(
            broken(|c| c.digital_crop_stops = vec![bad]),
            ZoomConfigError::InvalidCropStop(_)
        ));
    }
}

#[test]
fn out_of_range_values_are_rejected() {
    assert!(matches!(
        broken(|c| c.calibration_max_principal_point_offset = 0.0),
        ZoomConfigError::InvalidCalibrationOffset(_)
    ));
    assert!(matches!(
        broken(|c| c.calibration_max_principal_point_offset = 0.6),
        ZoomConfigError::InvalidCalibrationOffset(_)
    ));
    assert!(matches!(
        broken(|c| c.back_max_ratio = 0.5),
        ZoomConfigError::InvalidMaxRatio { .. }
    ));
    assert!(matches!(
        broken(|c| c.switch_up_hysteresis = 1.5),
        ZoomConfigError::InvalidHysteresis(_)
    ));
    assert!(matches!(
        broken(|c| c.stop_dedupe_tolerance = 0.5),
        ZoomConfigError::InvalidDedupeTolerance(_)
    ));
    assert!(matches!(
        broken(|c| c.digital_crop_stops = vec![1.0]),
        ZoomConfigError::InvalidCropStop(_)
    ));
}

#[test]
fn partial_json_keeps_defaults_and_unknown_keys_are_rejected() {
    let parsed: ZoomConfig = serde_json::from_str(r#"{"back_max_ratio": 8.0}"#).expect("частичный конфиг");
    assert_eq!(parsed.back_max_ratio, 8.0);
    assert_eq!(parsed.front_max_ratio, 3.0);
    assert!(serde_json::from_str::<ZoomConfig>(r#"{"unknown": 1}"#).is_err());
}

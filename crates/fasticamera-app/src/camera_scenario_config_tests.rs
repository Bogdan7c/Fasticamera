//! Проверка параметров сценариев камеры: умолчания несут матрицу решения 21, каждое неверное значение отвергается.

use camera_contract::raw_format::RawFormat;
use camera_contract::stream::ReadoutAspect;

use super::*;

#[test]
fn defaults_are_the_validated_session_00_matrix() {
    let config = CameraScenarioConfig::default();
    config.validate().expect("умолчания верны");
    let labels: Vec<(&str, u32, ExpectedLever)> = config
        .stream_steps
        .iter()
        .map(|configured| {
            (
                configured.camera_id.as_str(),
                configured.frame_rate,
                configured.expected_lever,
            )
        })
        .collect();
    assert_eq!(
        labels,
        [
            ("2", 60, ExpectedLever::PublicAeTargetRange),
            ("2", 60, ExpectedLever::VendorSensorFrameDurationOverride),
            ("3", 60, ExpectedLever::PublicAeTargetRange),
            ("1", 30, ExpectedLever::Any),
            ("0", 30, ExpectedLever::Any),
            ("2", 60, ExpectedLever::PublicAeTargetRange),
        ]
    );
    // Решение 21: матрица покрывает оба формата RAW.
    assert_eq!(
        config.stream_steps.last().map(|last| last.raw_format),
        Some(RawFormatSetting::Raw16)
    );
    assert_eq!(config.measure_ms, 5_000);
}

#[test]
fn step_resolves_to_contract_target_and_mode() {
    let resolved = CameraScenarioConfig::default().stream_steps[1].resolve().expect("шаг");
    assert_eq!(
        resolved.target,
        CameraTarget::Camera(CameraId::new("2").expect("id")),
        "шаг открывает конкретный ID, а не роль"
    );
    assert_eq!(resolved.mode.raw_format, RawFormat::Raw10);
    assert_eq!(resolved.mode.readout, ReadoutAspect::FourByThree);
    assert_eq!(resolved.mode.frame_rate.frames_per_second(), 60);
}

#[test]
fn evict_resolves_to_wide_role_raw10_four_by_three_30() {
    let resolved = EvictScenarioConfig::default().resolve().expect("вытеснение");
    assert_eq!(resolved.target, CameraTarget::Role(LensRole::Wide));
    assert_eq!(resolved.mode.frame_rate.frames_per_second(), 30);
    assert_eq!(resolved.mode.readout, ReadoutAspect::FourByThree);
}

fn rejected(change: impl FnOnce(&mut CameraScenarioConfig)) -> CameraScenarioConfigError {
    let mut config = CameraScenarioConfig::default();
    change(&mut config);
    config.validate().expect_err("значение должно отвергаться")
}

#[test]
fn zero_timeouts_and_empty_steps_are_rejected() {
    assert_eq!(
        rejected(|c| c.measure_ms = 0),
        CameraScenarioConfigError::Zero { field: "measure_ms" }
    );
    assert_eq!(
        rejected(|c| c.evict.recovery_timeout_ms = 0),
        CameraScenarioConfigError::Zero {
            field: "evict.recovery_timeout_ms"
        }
    );
    assert_eq!(
        rejected(|c| c.stream_steps.clear()),
        CameraScenarioConfigError::NoStreamSteps
    );
}

#[test]
fn bad_step_fields_are_rejected_with_the_step_index() {
    assert_eq!(
        rejected(|c| c.stream_steps[2].camera_id.clear()),
        CameraScenarioConfigError::EmptyCameraId { index: 2 }
    );
    assert!(matches!(
        rejected(|c| c.stream_steps[0].frame_rate = 0),
        CameraScenarioConfigError::InvalidFrameRate(_)
    ));
    assert!(matches!(
        rejected(|c| c.stream_steps[1].min_measured_fps = 61.0),
        CameraScenarioConfigError::InvalidMinimumRate { index: 1, .. }
    ));
    assert!(matches!(
        rejected(|c| c.stream_steps[0].min_measured_fps = f64::NAN),
        CameraScenarioConfigError::InvalidMinimumRate { index: 0, .. }
    ));
    assert!(matches!(
        rejected(|c| c.evict.frame_rate = 1000),
        CameraScenarioConfigError::InvalidFrameRate(_)
    ));
}

#[test]
fn lever_names_match_the_serde_names_of_the_setting() {
    assert_eq!(ExpectedLever::Any.name(), None);
    for lever in [
        ExpectedLever::PublicAeTargetRange,
        ExpectedLever::VendorSensorFrameDurationOverride,
    ] {
        assert_eq!(
            Some(serde_json::to_value(lever).expect("json"))
                .as_ref()
                .and_then(|value| value.as_str()),
            lever.name()
        );
    }
}

#[test]
fn only_the_logical_camera_zero_step_expects_no_final_results_by_default() {
    let config = CameraScenarioConfig::default();
    let modes: Vec<(&str, CaptureResultsExpectation)> = config
        .stream_steps
        .iter()
        .map(|configured| (configured.camera_id.as_str(), configured.capture_results))
        .collect();
    use CaptureResultsExpectation::{NotSentByHal, Required};
    assert_eq!(
        modes,
        [
            ("2", Required),
            ("2", Required),
            ("3", Required),
            ("1", Required),
            ("0", NotSentByHal),
            ("2", Required)
        ]
    );
}

#[test]
fn capture_results_defaults_to_required_when_absent_and_parses_snake_case() {
    let without: StreamStepConfig = serde_json::from_str(
        r#"{"camera_id":"2","raw_format":"raw10","readout":"four_by_three","frame_rate":60,"expected_lever":"any","min_measured_fps":55.0}"#,
    )
    .expect("шаг без поля");
    assert_eq!(without.capture_results, CaptureResultsExpectation::Required);
    let with: StreamStepConfig = serde_json::from_str(
        r#"{"camera_id":"0","raw_format":"raw10","readout":"four_by_three","frame_rate":30,"expected_lever":"any","min_measured_fps":28.0,"capture_results":"not_sent_by_hal"}"#,
    )
    .expect("шаг с полем");
    assert_eq!(with.capture_results, CaptureResultsExpectation::NotSentByHal);
    assert!(serde_json::from_str::<StreamStepConfig>(
        r#"{"camera_id":"0","raw_format":"raw10","readout":"four_by_three","frame_rate":30,"expected_lever":"any","min_measured_fps":28.0,"capture_results":"maybe"}"#
    )
    .is_err());
}

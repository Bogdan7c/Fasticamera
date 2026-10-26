use std::collections::HashMap;
use std::time::Duration;

use camera_contract::capabilities::{FpsRange, FrameDuration};
use camera_contract::fixtures::camera_id;

use super::*;
use crate::config::AndroidCameraConfig;
use crate::vendor::{TagResolution, TagResolver, VendorUnsupportedReason};

struct CamxDevice;

impl TagResolver for CamxDevice {
    fn resolve_tag(&self, name: &str) -> TagResolution {
        if name.ends_with("disableFPSLimits") {
            TagResolution::Found { tag: 0x804a_0000 }
        } else {
            TagResolution::Found { tag: 0x804a_0001 }
        }
    }
}

#[derive(Default)]
struct FakeRequest {
    entries: HashMap<u32, EntryValue>,
    reject_tag: Option<u32>,
}

impl RequestEntryWriter for FakeRequest {
    fn write_entry(&mut self, tag: u32, value: &EntryValue) -> Result<(), i32> {
        if self.reject_tag == Some(tag) {
            return Err(-10001);
        }
        self.entries.insert(tag, value.clone());
        Ok(())
    }
}

fn vendor(camera: &str) -> VendorCapabilities {
    VendorCapabilities::resolve(&camera_id(camera), &AndroidCameraConfig::default(), &CamxDevice)
}

fn sixty() -> TargetFrameRate {
    TargetFrameRate::new(60).unwrap()
}

fn inputs(mechanism: FrameRateMechanism, controls: &ManualControl) -> RequestInputs<'_> {
    RequestInputs {
        intent: CaptureIntent::Preview,
        controls,
        frame_rate: sixty(),
        mechanism,
        focus: FocusCapability::Adjustable {
            min_focus_distance_diopters: 10.0,
            hyperfocal_distance_diopters: Some(0.3),
            calibration: camera_contract::capabilities::FocusCalibration::Approximate,
        },
    }
}

fn ae_range_60() -> FrameRateMechanism {
    FrameRateMechanism::PublicAeTargetRange {
        range: FpsRange { min: 60, max: 60 },
    }
}

fn camx_60() -> FrameRateMechanism {
    FrameRateMechanism::VendorSensorFrameDurationOverride {
        frame_duration: FrameDuration::from_nanos(16_666_667),
    }
}

#[test]
fn template_follows_the_capture_intent() {
    assert_eq!(RequestTemplate::from(CaptureIntent::Preview).ndk_value(), 1);
    assert_eq!(RequestTemplate::from(CaptureIntent::VideoRecord).ndk_value(), 3);
}

#[test]
fn ae_range_lever_writes_one_int32_pair_and_applies_to_session_parameters_too() {
    let plan = plan_repeating_request(&inputs(ae_range_60(), &ManualControl::AUTOMATIC));
    assert_eq!(
        plan.entries[0],
        PlannedEntry::Public {
            tag: tags::AE_TARGET_FPS_RANGE,
            value: EntryValue::Int32s(vec![60, 60])
        }
    );
    let session = plan_session_parameters(CaptureIntent::Preview, ae_range_60()).expect("AE — сессионный ключ");
    assert_eq!(session.entries, vec![plan.entries[0].clone()]);
}

#[test]
fn camx_lever_writes_the_pair_to_the_request_without_ae_range_and_without_session_parameters() {
    let plan = plan_repeating_request(&inputs(camx_60(), &ManualControl::AUTOMATIC));
    assert_eq!(
        plan.entries[..2],
        [
            PlannedEntry::Vendor {
                key: VendorKey::DisableFpsLimits,
                value: VendorValue::Byte(1)
            },
            PlannedEntry::Vendor {
                key: VendorKey::OverrideSensorFrameDuration,
                value: VendorValue::Int64(16_666_667)
            },
        ]
    );
    // Диапазон AE с ключами CamX уводит HAL в 16:9 (00, попытка 4б): его быть не должно нигде.
    let has_ae_range = |entries: &[PlannedEntry]| {
        entries
            .iter()
            .any(|entry| matches!(entry, PlannedEntry::Public { tag, .. } if *tag == tags::AE_TARGET_FPS_RANGE))
    };
    assert!(!has_ae_range(&plan.entries));
    assert_eq!(plan_session_parameters(CaptureIntent::Preview, camx_60()), None);
}

#[test]
fn hal_default_lever_writes_nothing_for_frame_rate() {
    let plan = plan_repeating_request(&inputs(FrameRateMechanism::HalDefault, &ManualControl::AUTOMATIC));
    assert!(
        plan.entries
            .iter()
            .all(|entry| !matches!(entry, PlannedEntry::Vendor { .. }))
    );
    assert!(
        !plan
            .entries
            .iter()
            .any(|entry| matches!(entry, PlannedEntry::Public { tag, .. } if *tag == tags::AE_TARGET_FPS_RANGE))
    );
    assert_eq!(
        plan_session_parameters(CaptureIntent::Preview, FrameRateMechanism::HalDefault),
        None
    );
}

#[test]
fn automatic_controls_leave_exposure_to_the_vendor_and_pick_af_mode_by_intent() {
    let preview = plan_repeating_request(&inputs(FrameRateMechanism::HalDefault, &ManualControl::AUTOMATIC));
    assert_eq!(
        preview.entries,
        vec![PlannedEntry::Public {
            tag: tags::AF_MODE,
            value: EntryValue::Bytes(vec![tags::AF_MODE_CONTINUOUS_PICTURE])
        }]
    );
    let mut record_inputs = inputs(FrameRateMechanism::HalDefault, &ManualControl::AUTOMATIC);
    record_inputs.intent = CaptureIntent::VideoRecord;
    let record = plan_repeating_request(&record_inputs);
    assert_eq!(record.template, RequestTemplate::Record);
    assert_eq!(
        record.entries,
        vec![PlannedEntry::Public {
            tag: tags::AF_MODE,
            value: EntryValue::Bytes(vec![tags::AF_MODE_CONTINUOUS_VIDEO])
        }]
    );
}

#[test]
fn manual_exposure_turns_ae_off_and_sets_exposure_iso_and_frame_duration_like_the_probe() {
    // Зонд 00, попытка 4: выдержка 8 мс, ISO 800, длительность кадра 16,67 мс.
    let controls = ManualControl {
        exposure: ExposureControl::Manual {
            exposure_time: Duration::from_millis(8),
            iso: 800,
        },
        focus: FocusControl::ContinuousAuto,
    };
    let plan = plan_repeating_request(&inputs(camx_60(), &controls));
    let public: HashMap<&str, &EntryValue> = plan
        .entries
        .iter()
        .filter_map(|entry| match entry {
            PlannedEntry::Public { tag, value } => Some((tag.name, value)),
            PlannedEntry::Vendor { .. } => None,
        })
        .collect();
    assert_eq!(
        public["ACAMERA_CONTROL_AE_MODE"],
        &EntryValue::Bytes(vec![tags::AE_MODE_OFF])
    );
    assert_eq!(
        public["ACAMERA_SENSOR_EXPOSURE_TIME"],
        &EntryValue::Int64s(vec![8_000_000])
    );
    assert_eq!(public["ACAMERA_SENSOR_SENSITIVITY"], &EntryValue::Int32s(vec![800]));
    assert_eq!(
        public["ACAMERA_SENSOR_FRAME_DURATION"],
        &EntryValue::Int64s(vec![16_666_667])
    );
}

#[test]
fn frame_duration_is_never_shorter_than_a_long_exposure() {
    let controls = ManualControl {
        exposure: ExposureControl::Manual {
            exposure_time: Duration::from_millis(100),
            iso: 100,
        },
        focus: FocusControl::ContinuousAuto,
    };
    let plan = plan_repeating_request(&inputs(FrameRateMechanism::HalDefault, &controls));
    assert!(plan.entries.contains(&PlannedEntry::Public {
        tag: tags::SENSOR_FRAME_DURATION,
        value: EntryValue::Int64s(vec![100_000_000])
    }));
}

#[test]
fn manual_focus_switches_af_off_and_sets_the_distance() {
    let controls = ManualControl {
        exposure: ExposureControl::Auto,
        focus: FocusControl::Manual { distance_diopters: 2.5 },
    };
    let plan = plan_repeating_request(&inputs(FrameRateMechanism::HalDefault, &controls));
    assert_eq!(
        plan.entries,
        vec![
            PlannedEntry::Public {
                tag: tags::AF_MODE,
                value: EntryValue::Bytes(vec![tags::AF_MODE_OFF])
            },
            PlannedEntry::Public {
                tag: tags::LENS_FOCUS_DISTANCE,
                value: EntryValue::Floats(vec![2.5])
            },
        ]
    );
}

#[test]
fn fixed_focus_camera_gets_no_focus_entries_at_all() {
    let mut fixed = inputs(FrameRateMechanism::HalDefault, &ManualControl::AUTOMATIC);
    fixed.focus = FocusCapability::Fixed;
    assert!(plan_repeating_request(&fixed).entries.is_empty());
}

#[test]
fn applying_the_full_camx_plan_writes_all_entries_into_the_request_map() {
    let controls = ManualControl {
        exposure: ExposureControl::Manual {
            exposure_time: Duration::from_millis(8),
            iso: 800,
        },
        focus: FocusControl::ContinuousAuto,
    };
    let plan = plan_repeating_request(&inputs(camx_60(), &controls));
    let mut request = FakeRequest::default();
    apply_entries(&plan.entries, &vendor("2"), &mut request).unwrap();
    // 2 вендорских + AE_MODE, выдержка, ISO, длительность кадра + AF_MODE.
    assert_eq!(request.entries.len(), 7);
    assert_eq!(request.entries[&0x804a_0000], EntryValue::Bytes(vec![1]));
    assert_eq!(request.entries[&0x804a_0001], EntryValue::Int64s(vec![16_666_667]));
    assert_eq!(
        request.entries[&tags::SENSOR_EXPOSURE_TIME.code],
        EntryValue::Int64s(vec![8_000_000])
    );
}

#[test]
fn camx_plan_on_the_ultra_wide_is_refused_and_leaves_the_request_untouched_by_vendor_keys() {
    let plan = plan_repeating_request(&inputs(camx_60(), &ManualControl::AUTOMATIC));
    let mut request = FakeRequest::default();
    let error = apply_entries(&plan.entries, &vendor("3"), &mut request).unwrap_err();
    assert_eq!(
        error,
        ApplyError::Vendor(VendorWriteError::Unsupported {
            key: VendorKey::DisableFpsLimits,
            reason: VendorUnsupportedReason::NotAllowedForCamera
        })
    );
    assert!(request.entries.is_empty());
}

#[test]
fn public_rejection_stops_application_and_names_the_tag() {
    let plan = plan_repeating_request(&inputs(ae_range_60(), &ManualControl::AUTOMATIC));
    let mut request = FakeRequest {
        reject_tag: Some(tags::AE_TARGET_FPS_RANGE.code),
        ..FakeRequest::default()
    };
    assert_eq!(
        apply_entries(&plan.entries, &vendor("2"), &mut request),
        Err(ApplyError::PublicRejected {
            tag: "ACAMERA_CONTROL_AE_TARGET_FPS_RANGE",
            status: -10001
        })
    );
    assert!(request.entries.is_empty(), "после отказа остальные записи не пишутся");
}

#[test]
fn platform_status_is_kept_only_for_platform_rejections() {
    assert_eq!(
        ApplyError::PublicRejected {
            tag: "T",
            status: -10001
        }
        .platform_status(),
        -10001
    );
    assert_eq!(
        ApplyError::Vendor(VendorWriteError::Rejected {
            key: VendorKey::DisableFpsLimits,
            status: -10008
        })
        .platform_status(),
        -10008
    );
    assert_eq!(
        ApplyError::Vendor(VendorWriteError::Unsupported {
            key: VendorKey::DisableFpsLimits,
            reason: VendorUnsupportedReason::TagNotFound
        })
        .platform_status(),
        0
    );
}

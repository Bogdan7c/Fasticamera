use std::collections::HashMap;
use std::time::Duration;

use camera_contract::capabilities::{FpsRange, FrameDuration};
use camera_contract::control::{ExposureTime, FocusDistance, Iso};
use camera_contract::fixtures::{camera_id, front_camera, main_wide_camera};
use camera_contract::flash::FlashControl;
use camera_contract::three_a::{AeAfLock, AfModeSupport, BracketExposure, MeteringRegion, SensorPoint};
use camera_core::request_controls::FocusPlan;

use super::*;
use crate::config::AndroidCameraConfig;
use crate::metering_region::METERING_WEIGHT_MAX;
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

pub(super) fn ae_range_60() -> FrameRateMechanism {
    FrameRateMechanism::PublicAeTargetRange {
        range: FpsRange { min: 60, max: 60 },
    }
}

pub(super) fn camx_60() -> FrameRateMechanism {
    FrameRateMechanism::VendorSensorFrameDurationOverride {
        frame_duration: FrameDuration::from_nanos(16_666_667),
    }
}

pub(super) fn every_mechanism() -> [FrameRateMechanism; 3] {
    [FrameRateMechanism::HalDefault, ae_range_60(), camx_60()]
}

pub(super) fn auto_exposure() -> ExposurePlan {
    ExposurePlan::VendorAuto {
        compensation_steps: 0,
        lock: AeAfLock::Unlocked,
        region: None,
    }
}

pub(super) fn controls(exposure: ExposurePlan, focus: FocusPlan) -> RequestControls {
    RequestControls {
        exposure,
        focus,
        flash: FlashControl::Off,
    }
}

pub(super) fn sensor(exposure_ms: u64, iso: u32) -> ExposurePlan {
    ExposurePlan::Sensor {
        exposure_time: ExposureTime::new(Duration::from_millis(exposure_ms)).unwrap(),
        iso: Iso::new(iso).unwrap(),
    }
}

pub(super) fn region(x: f32, y: f32, size: f32) -> MeteringRegion {
    MeteringRegion::new(SensorPoint::new(x, y).unwrap(), size).unwrap()
}

pub(super) fn continuous() -> FocusPlan {
    FocusPlan::Continuous { region: None }
}

/// Входы для основной камеры OnePlus 11 (4096×3072, AE-регион 1, AF-регион 1, режимы AF 0–4).
pub(super) fn inputs<'a>(
    mechanism: FrameRateMechanism,
    controls: &'a RequestControls,
    camera: &'a CameraCapabilities,
) -> RequestInputs<'a> {
    RequestInputs {
        intent: CaptureIntent::Preview,
        controls,
        trigger: None,
        frame_rate: sixty(),
        mechanism,
        camera,
        mains: MainsFrequency::Hz50,
    }
}

pub(super) fn main_camera() -> CameraCapabilities {
    main_wide_camera().capabilities
}

/// Публичные записи плана по имени тега.
pub(super) fn public_entries(plan: &RequestPlan) -> HashMap<&'static str, &EntryValue> {
    plan.entries
        .iter()
        .filter_map(|entry| match entry {
            PlannedEntry::Public { tag, value } => Some((tag.name, value)),
            PlannedEntry::Vendor { .. } => None,
        })
        .collect()
}

pub(super) fn bytes(value: u8) -> EntryValue {
    EntryValue::Bytes(vec![value])
}

pub(super) fn plan_of(
    mechanism: FrameRateMechanism,
    controls: &RequestControls,
    camera: &CameraCapabilities,
) -> PlannedRequest {
    plan_request(&inputs(mechanism, controls, camera))
}

pub(super) fn has_vendor_pair(plan: &RequestPlan) -> bool {
    let has = |wanted: VendorKey| {
        plan.entries
            .iter()
            .any(|entry| matches!(entry, PlannedEntry::Vendor { key, .. } if *key == wanted))
    };
    has(VendorKey::DisableFpsLimits) && has(VendorKey::OverrideSensorFrameDuration)
}

pub(super) fn has_ae_range(plan: &RequestPlan) -> bool {
    plan.entries
        .iter()
        .any(|entry| matches!(entry, PlannedEntry::Public { tag, .. } if *tag == tags::AE_TARGET_FPS_RANGE))
}

pub(super) fn bracket_of(frames: &[(u64, u32)]) -> ExposureBracket {
    let frames = frames
        .iter()
        .map(|(exposure_ms, iso)| BracketExposure {
            exposure_time: ExposureTime::new(Duration::from_millis(*exposure_ms)).unwrap(),
            iso: Iso::new(*iso).unwrap(),
        })
        .collect();
    ExposureBracket::new(frames, 8).unwrap()
}

#[test]
fn template_follows_the_capture_intent() {
    assert_eq!(RequestTemplate::from(CaptureIntent::Preview).ndk_value(), 1);
    assert_eq!(RequestTemplate::from(CaptureIntent::VideoRecord).ndk_value(), 3);
}

#[test]
fn ae_range_lever_writes_one_int32_pair_and_applies_to_session_parameters_too() {
    let camera = main_camera();
    let plan = plan_of(ae_range_60(), &controls(auto_exposure(), continuous()), &camera).plan;
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
fn camx_lever_writes_the_pair_first_without_ae_range_and_without_session_parameters() {
    let camera = main_camera();
    let plan = plan_of(camx_60(), &controls(auto_exposure(), continuous()), &camera).plan;
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
    assert!(!has_ae_range(&plan));
    assert_eq!(plan_session_parameters(CaptureIntent::Preview, camx_60()), None);
}

#[test]
fn hal_default_lever_writes_nothing_for_frame_rate() {
    let camera = main_camera();
    let plan = plan_of(
        FrameRateMechanism::HalDefault,
        &controls(auto_exposure(), continuous()),
        &camera,
    )
    .plan;
    assert!(!has_ae_range(&plan));
    assert!(
        plan.entries
            .iter()
            .all(|entry| !matches!(entry, PlannedEntry::Vendor { .. }))
    );
    assert_eq!(
        plan_session_parameters(CaptureIntent::Preview, FrameRateMechanism::HalDefault),
        None
    );
}

#[test]
fn vendor_auto_ae_writes_the_full_table_and_continuous_picture_for_preview() {
    let camera = main_camera();
    let request = plan_of(
        FrameRateMechanism::HalDefault,
        &controls(auto_exposure(), continuous()),
        &camera,
    );
    assert_eq!(request.notes, Vec::new());
    assert_eq!(request.plan.template, RequestTemplate::Preview);
    let entries = public_entries(&request.plan);
    assert_eq!(entries["ACAMERA_CONTROL_MODE"], &bytes(tags::CONTROL_MODE_AUTO));
    assert_eq!(entries["ACAMERA_CONTROL_AWB_MODE"], &bytes(tags::AWB_MODE_AUTO));
    assert_eq!(entries["ACAMERA_CONTROL_AE_MODE"], &bytes(tags::AE_MODE_ON));
    assert_eq!(
        entries["ACAMERA_CONTROL_AE_EXPOSURE_COMPENSATION"],
        &EntryValue::Int32s(vec![0])
    );
    assert_eq!(entries["ACAMERA_CONTROL_AE_LOCK"], &bytes(tags::AE_LOCK_OFF));
    assert_eq!(
        entries["ACAMERA_CONTROL_AE_ANTIBANDING_MODE"],
        &bytes(tags::ANTIBANDING_MODE_50HZ),
        "частота сети по умолчанию — 50 Гц (решение 67)"
    );
    assert_eq!(
        entries["ACAMERA_CONTROL_AF_MODE"],
        &bytes(tags::AF_MODE_CONTINUOUS_PICTURE)
    );
    assert_eq!(
        entries["ACAMERA_FLASH_MODE"],
        &bytes(tags::FLASH_MODE_OFF),
        "светодиод явно выключен: ключи HAL липкие (14d)"
    );
    assert_eq!(entries.len(), 8, "лишних записей нет: {entries:?}");
    assert!(!entries.contains_key("ACAMERA_CONTROL_AE_REGIONS"));
    assert!(!entries.contains_key("ACAMERA_CONTROL_AF_TRIGGER"));
}

#[test]
fn video_intent_uses_continuous_video_and_the_record_template() {
    let camera = main_camera();
    let controls = controls(auto_exposure(), continuous());
    let mut video = inputs(FrameRateMechanism::HalDefault, &controls, &camera);
    video.intent = CaptureIntent::VideoRecord;
    let request = plan_request(&video);
    assert_eq!(request.plan.template, RequestTemplate::Record);
    assert_eq!(
        public_entries(&request.plan)["ACAMERA_CONTROL_AF_MODE"],
        &bytes(tags::AF_MODE_CONTINUOUS_VIDEO)
    );
}

#[test]
fn vendor_auto_carries_compensation_lock_region_and_the_configured_mains_frequency() {
    let camera = main_camera();
    let exposure = ExposurePlan::VendorAuto {
        compensation_steps: -6,
        lock: AeAfLock::Locked,
        region: Some(region(0.5, 0.5, 0.1)),
    };
    let controls = controls(exposure, continuous());
    let mut sixty_hertz = inputs(FrameRateMechanism::HalDefault, &controls, &camera);
    sixty_hertz.mains = MainsFrequency::Hz60;
    let request = plan_request(&sixty_hertz);
    assert_eq!(request.notes, Vec::new());
    let entries = public_entries(&request.plan);
    assert_eq!(
        entries["ACAMERA_CONTROL_AE_EXPOSURE_COMPENSATION"],
        &EntryValue::Int32s(vec![-6])
    );
    assert_eq!(entries["ACAMERA_CONTROL_AE_LOCK"], &bytes(tags::AE_LOCK_ON));
    assert_eq!(
        entries["ACAMERA_CONTROL_AE_REGIONS"],
        &EntryValue::Int32s(vec![1895, 1383, 2202, 1690, METERING_WEIGHT_MAX])
    );
    assert_eq!(
        entries["ACAMERA_CONTROL_AE_ANTIBANDING_MODE"],
        &bytes(tags::ANTIBANDING_MODE_60HZ)
    );
}

#[test]
fn compensation_outside_the_camera_range_is_clamped_with_a_note() {
    let camera = main_camera();
    let exposure = ExposurePlan::VendorAuto {
        compensation_steps: 40,
        lock: AeAfLock::Unlocked,
        region: None,
    };
    let request = plan_of(
        FrameRateMechanism::HalDefault,
        &controls(exposure, continuous()),
        &camera,
    );
    assert_eq!(
        public_entries(&request.plan)["ACAMERA_CONTROL_AE_EXPOSURE_COMPENSATION"],
        &EntryValue::Int32s(vec![18])
    );
    assert_eq!(
        request.notes,
        vec![PlanNote::CompensationClamped {
            requested_steps: 40,
            applied_steps: 18
        }]
    );
}

#[test]
fn unsupported_ae_features_are_dropped_with_notes_not_written() {
    let mut camera = main_camera();
    camera.three_a.max_regions.auto_exposure = 0;
    camera.three_a.ae_lock = camera_contract::three_a::LockSupport::NotAvailable;
    camera.three_a.exposure_compensation = camera_contract::three_a::ExposureCompensationSupport::NotSupported;
    camera.three_a.antibanding_modes.clear();
    let exposure = ExposurePlan::VendorAuto {
        compensation_steps: 3,
        lock: AeAfLock::Locked,
        region: Some(region(0.5, 0.5, 0.1)),
    };
    let request = plan_of(
        FrameRateMechanism::HalDefault,
        &controls(exposure, continuous()),
        &camera,
    );
    let entries = public_entries(&request.plan);
    for absent in [
        "ACAMERA_CONTROL_AE_REGIONS",
        "ACAMERA_CONTROL_AE_LOCK",
        "ACAMERA_CONTROL_AE_EXPOSURE_COMPENSATION",
        "ACAMERA_CONTROL_AE_ANTIBANDING_MODE",
    ] {
        assert!(!entries.contains_key(absent), "{absent} не должен писаться");
    }
    assert_eq!(
        request.notes,
        vec![
            PlanNote::CompensationUnsupported { requested_steps: 3 },
            PlanNote::AeLockUnavailable,
            PlanNote::AeRegionUnsupported,
            PlanNote::AntibandingUnsupported {
                requested: camera_contract::three_a::AntibandingModeSupport::Hz50
            },
        ]
    );
}

#[test]
pub(super) fn sensor_exposure_turns_ae_off_and_writes_only_manual_sensor_keys() {
    // Зонд 00, попытка 4: выдержка 8 мс, ISO 800, длительность кадра 16,67 мс.
    let camera = main_camera();
    let request = plan_of(camx_60(), &controls(sensor(8, 800), continuous()), &camera);
    let entries = public_entries(&request.plan);
    assert_eq!(entries["ACAMERA_CONTROL_AE_MODE"], &bytes(tags::AE_MODE_OFF));
    assert_eq!(
        entries["ACAMERA_SENSOR_EXPOSURE_TIME"],
        &EntryValue::Int64s(vec![8_000_000])
    );
    assert_eq!(entries["ACAMERA_SENSOR_SENSITIVITY"], &EntryValue::Int32s(vec![800]));
    assert_eq!(
        entries["ACAMERA_SENSOR_FRAME_DURATION"],
        &EntryValue::Int64s(vec![16_666_667])
    );
    // Ключи вендорского AE при ручном сенсоре не пишутся вообще.
    for vendor_ae in [
        "ACAMERA_CONTROL_AE_EXPOSURE_COMPENSATION",
        "ACAMERA_CONTROL_AE_LOCK",
        "ACAMERA_CONTROL_AE_REGIONS",
        "ACAMERA_CONTROL_AE_ANTIBANDING_MODE",
    ] {
        assert!(!entries.contains_key(vendor_ae), "{vendor_ae}");
    }
    // Баланс HAL — только диагностика, но режим 3A и AWB пишутся всегда.
    assert_eq!(entries["ACAMERA_CONTROL_MODE"], &bytes(tags::CONTROL_MODE_AUTO));
    assert_eq!(entries["ACAMERA_CONTROL_AWB_MODE"], &bytes(tags::AWB_MODE_AUTO));
}

#[test]
fn frame_duration_is_never_shorter_than_a_long_exposure() {
    let camera = main_camera();
    let request = plan_of(
        FrameRateMechanism::HalDefault,
        &controls(sensor(100, 100), continuous()),
        &camera,
    );
    assert!(request.plan.entries.contains(&PlannedEntry::Public {
        tag: tags::SENSOR_FRAME_DURATION,
        value: EntryValue::Int64s(vec![100_000_000])
    }));
}

#[test]
pub(super) fn continuous_focus_with_a_region_writes_af_regions_in_sensor_pixels() {
    let camera = main_camera();
    let focus = FocusPlan::Continuous {
        region: Some(region(0.0, 0.0, 0.2)),
    };
    let request = plan_of(
        FrameRateMechanism::HalDefault,
        &controls(auto_exposure(), focus),
        &camera,
    );
    assert_eq!(
        public_entries(&request.plan)["ACAMERA_CONTROL_AF_REGIONS"],
        &EntryValue::Int32s(vec![0, 0, 307, 307, METERING_WEIGHT_MAX])
    );
}

#[test]
fn single_shot_focus_uses_af_auto_with_the_tap_region() {
    let camera = main_camera();
    let focus = FocusPlan::SingleShot {
        region: region(0.5, 0.5, 0.1),
    };
    let request = plan_of(
        FrameRateMechanism::HalDefault,
        &controls(auto_exposure(), focus),
        &camera,
    );
    let entries = public_entries(&request.plan);
    assert_eq!(entries["ACAMERA_CONTROL_AF_MODE"], &bytes(tags::AF_MODE_AUTO));
    assert_eq!(
        entries["ACAMERA_CONTROL_AF_REGIONS"],
        &EntryValue::Int32s(vec![1895, 1383, 2202, 1690, METERING_WEIGHT_MAX])
    );
}

#[test]
fn manual_focus_switches_af_off_and_sets_the_distance() {
    let camera = main_camera();
    let focus = FocusPlan::Manual {
        distance: FocusDistance::from_diopters(2.5).unwrap(),
    };
    let request = plan_of(
        FrameRateMechanism::HalDefault,
        &controls(auto_exposure(), focus),
        &camera,
    );
    let entries = public_entries(&request.plan);
    assert_eq!(entries["ACAMERA_CONTROL_AF_MODE"], &bytes(tags::AF_MODE_OFF));
    assert_eq!(entries["ACAMERA_LENS_FOCUS_DISTANCE"], &EntryValue::Floats(vec![2.5]));
    assert!(!entries.contains_key("ACAMERA_CONTROL_AF_REGIONS"));
}

#[test]
fn fixed_lens_plan_and_fixed_focus_camera_get_no_focus_entries() {
    let camera = main_camera();
    let none = plan_of(
        FrameRateMechanism::HalDefault,
        &controls(auto_exposure(), FocusPlan::FixedLens),
        &camera,
    );
    assert!(!public_entries(&none.plan).contains_key("ACAMERA_CONTROL_AF_MODE"));
    assert_eq!(none.notes, Vec::new());

    // Фронтальная камера (фокус фиксирован) с планом «непрерывный AF»: теги AF не пишутся, заметка есть.
    let front = front_camera().capabilities;
    let request = plan_of(
        FrameRateMechanism::HalDefault,
        &controls(auto_exposure(), continuous()),
        &front,
    );
    assert!(!public_entries(&request.plan).contains_key("ACAMERA_CONTROL_AF_MODE"));
    assert_eq!(request.notes, vec![PlanNote::FocusIgnoredOnFixedLens]);
}

#[test]
fn af_mode_the_camera_did_not_declare_is_not_written() {
    let mut camera = main_camera();
    camera.three_a.af_modes = vec![AfModeSupport::Off, AfModeSupport::ContinuousVideo];
    let request = plan_of(
        FrameRateMechanism::HalDefault,
        &controls(auto_exposure(), continuous()),
        &camera,
    );
    assert!(!public_entries(&request.plan).contains_key("ACAMERA_CONTROL_AF_MODE"));
    assert_eq!(
        request.notes,
        vec![PlanNote::AfModeUnsupported {
            requested: AfModeSupport::ContinuousPicture
        }]
    );
}

#[test]
fn af_region_is_dropped_when_the_camera_takes_none() {
    let mut camera = main_camera();
    camera.three_a.max_regions.auto_focus = 0;
    let focus = FocusPlan::SingleShot {
        region: region(0.5, 0.5, 0.1),
    };
    let request = plan_of(
        FrameRateMechanism::HalDefault,
        &controls(auto_exposure(), focus),
        &camera,
    );
    let entries = public_entries(&request.plan);
    assert_eq!(entries["ACAMERA_CONTROL_AF_MODE"], &bytes(tags::AF_MODE_AUTO));
    assert!(!entries.contains_key("ACAMERA_CONTROL_AF_REGIONS"));
    assert_eq!(request.notes, vec![PlanNote::AfRegionUnsupported]);
}

#[test]
fn applying_the_full_camx_plan_writes_all_entries_into_the_request_map() {
    let camera = main_camera();
    let plan = plan_of(camx_60(), &controls(sensor(8, 800), continuous()), &camera).plan;
    let mut request = FakeRequest::default();
    apply_entries(&plan.entries, &vendor("2"), &mut request).unwrap();
    // 2 вендорских + CONTROL_MODE, AWB_MODE + AE_MODE, выдержка, ISO, длительность кадра + AF_MODE
    // + FLASH_MODE (явный OFF).
    assert_eq!(request.entries.len(), 10);
    assert_eq!(
        request.entries[&tags::FLASH_MODE.code],
        EntryValue::Bytes(vec![tags::FLASH_MODE_OFF])
    );
    assert_eq!(request.entries[&0x804a_0000], EntryValue::Bytes(vec![1]));
    assert_eq!(request.entries[&0x804a_0001], EntryValue::Int64s(vec![16_666_667]));
    assert_eq!(
        request.entries[&tags::SENSOR_EXPOSURE_TIME.code],
        EntryValue::Int64s(vec![8_000_000])
    );
}

#[test]
fn camx_plan_on_the_ultra_wide_is_refused_and_leaves_the_request_untouched_by_vendor_keys() {
    let camera = main_camera();
    let plan = plan_of(camx_60(), &controls(auto_exposure(), continuous()), &camera).plan;
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
    let camera = main_camera();
    let plan = plan_of(ae_range_60(), &controls(auto_exposure(), continuous()), &camera).plan;
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

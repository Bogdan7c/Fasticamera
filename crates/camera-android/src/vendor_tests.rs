use std::cell::RefCell;
use std::collections::HashMap;

use camera_contract::fixtures::camera_id;

use super::*;

const DISABLE_FPS_LIMITS_TAG: u32 = 0x804a_0000;
const OVERRIDE_FRAME_DURATION_TAG: u32 = 0x804a_0001;
const QCFA_TAG: u32 = 0x8066_0000;

/// Устройство OnePlus 11: знает оба тега CamX (номера из 00) и QCFA; помнит, какие имена у него спрашивали.
struct FakeResolver {
    known: HashMap<&'static str, TagResolution>,
    asked: RefCell<Vec<String>>,
}

impl FakeResolver {
    fn oneplus11() -> Self {
        let mut known = HashMap::new();
        known.insert(
            VENDOR_TAG_SPECS[0].name,
            TagResolution::Found {
                tag: DISABLE_FPS_LIMITS_TAG,
            },
        );
        known.insert(
            VENDOR_TAG_SPECS[1].name,
            TagResolution::Found {
                tag: OVERRIDE_FRAME_DURATION_TAG,
            },
        );
        known.insert(VENDOR_TAG_SPECS[2].name, TagResolution::Found { tag: QCFA_TAG });
        Self {
            known,
            asked: RefCell::new(Vec::new()),
        }
    }

    fn knowing_nothing() -> Self {
        Self {
            known: HashMap::new(),
            asked: RefCell::new(Vec::new()),
        }
    }
}

impl TagResolver for FakeResolver {
    fn resolve_tag(&self, name: &str) -> TagResolution {
        self.asked.borrow_mut().push(name.to_owned());
        self.known.get(name).copied().unwrap_or(TagResolution::NotFound)
    }
}

/// Запрос-карта: что записано, по тегам. Может отвергнуть один тег.
#[derive(Default)]
struct FakeRequest {
    entries: HashMap<u32, EntryValue>,
    reject_tag: Option<(u32, i32)>,
}

impl RequestEntryWriter for FakeRequest {
    fn write_entry(&mut self, tag: u32, value: &EntryValue) -> Result<(), i32> {
        if let Some((_, status)) = self.reject_tag.filter(|(rejected, _)| *rejected == tag) {
            return Err(status);
        }
        self.entries.insert(tag, value.clone());
        Ok(())
    }
}

fn resolve_for(camera: &str, resolver: &FakeResolver) -> VendorCapabilities {
    VendorCapabilities::resolve(&camera_id(camera), &AndroidCameraConfig::default(), resolver)
}

#[test]
fn main_camera_gets_both_camx_tags_with_numbers_from_the_device() {
    let vendor = resolve_for("2", &FakeResolver::oneplus11());
    assert_eq!(
        vendor.support(VendorKey::DisableFpsLimits),
        VendorSupport::Available {
            tag: DISABLE_FPS_LIMITS_TAG
        }
    );
    assert_eq!(
        vendor.support(VendorKey::OverrideSensorFrameDuration),
        VendorSupport::Available {
            tag: OVERRIDE_FRAME_DURATION_TAG
        }
    );
    assert_eq!(
        vendor.frame_duration_override_support(),
        FrameDurationOverrideSupport::Available
    );
    assert_eq!(vendor.characteristic_tag(VendorKey::IsQcfaSensor), Some(QCFA_TAG));
}

#[test]
fn camx_keys_are_forbidden_on_the_ultra_wide_front_and_logical_cameras_and_hal_is_not_even_asked() {
    for camera in ["3", "1", "0"] {
        let resolver = FakeResolver::oneplus11();
        let vendor = resolve_for(camera, &resolver);
        assert_eq!(
            vendor.support(VendorKey::DisableFpsLimits),
            VendorSupport::Unsupported(VendorUnsupportedReason::NotAllowedForCamera),
            "камера {camera}"
        );
        assert_eq!(
            vendor.frame_duration_override_support(),
            FrameDurationOverrideSupport::NotAllowedForCamera
        );
        let asked = resolver.asked.borrow();
        assert!(
            !asked.iter().any(|name| name.contains("manualExposure")),
            "камера {camera}: имена CamX запрашивались: {asked:?}"
        );
        // Чтение характеристики безопасно и разрешено на любой камере.
        assert_eq!(vendor.characteristic_tag(VendorKey::IsQcfaSensor), Some(QCFA_TAG));
    }
}

#[test]
fn a_device_without_the_tags_gives_tags_missing_not_a_panic() {
    let vendor = resolve_for("2", &FakeResolver::knowing_nothing());
    assert_eq!(
        vendor.support(VendorKey::DisableFpsLimits),
        VendorSupport::Unsupported(VendorUnsupportedReason::TagNotFound)
    );
    assert_eq!(
        vendor.frame_duration_override_support(),
        FrameDurationOverrideSupport::TagsMissing
    );
    assert_eq!(vendor.characteristic_tag(VendorKey::IsQcfaSensor), None);
}

#[test]
fn one_found_tag_out_of_two_is_still_not_enough_and_resolve_failure_keeps_its_status() {
    let mut resolver = FakeResolver::oneplus11();
    resolver
        .known
        .insert(VENDOR_TAG_SPECS[1].name, TagResolution::Failed { status: -10001 });
    let vendor = resolve_for("2", &resolver);
    assert_eq!(
        vendor.frame_duration_override_support(),
        FrameDurationOverrideSupport::TagsMissing
    );
    assert_eq!(
        vendor.support(VendorKey::OverrideSensorFrameDuration),
        VendorSupport::Unsupported(VendorUnsupportedReason::ResolveFailed { status: -10001 })
    );
}

#[test]
fn writing_the_camx_pair_puts_exactly_two_typed_entries_and_nothing_else() {
    let vendor = resolve_for("2", &FakeResolver::oneplus11());
    let mut request = FakeRequest::default();
    // Чужой ключ, уже лежащий в запросе (например, AE-режим), обязан остаться нетронутым.
    request.entries.insert(0x0001_0003, EntryValue::Bytes(vec![1]));

    vendor
        .write_value(VendorKey::DisableFpsLimits, VendorValue::Byte(1), &mut request)
        .unwrap();
    assert_eq!(request.entries.len(), 2, "запись одного ключа не трогает остальные");
    vendor
        .write_value(
            VendorKey::OverrideSensorFrameDuration,
            VendorValue::Int64(16_666_667),
            &mut request,
        )
        .unwrap();

    assert_eq!(request.entries.len(), 3);
    assert_eq!(request.entries[&DISABLE_FPS_LIMITS_TAG], EntryValue::Bytes(vec![1]));
    assert_eq!(
        request.entries[&OVERRIDE_FRAME_DURATION_TAG],
        EntryValue::Int64s(vec![16_666_667])
    );
    assert_eq!(request.entries[&0x0001_0003], EntryValue::Bytes(vec![1]));
}

#[test]
fn wrong_value_type_is_an_error_and_writes_nothing() {
    let vendor = resolve_for("2", &FakeResolver::oneplus11());
    let mut request = FakeRequest::default();
    let error = vendor
        .write_value(
            VendorKey::OverrideSensorFrameDuration,
            VendorValue::Int32(16_666_667),
            &mut request,
        )
        .unwrap_err();
    assert_eq!(
        error,
        VendorWriteError::TypeMismatch {
            key: VendorKey::OverrideSensorFrameDuration,
            expected: VendorValueKind::Int64,
            actual: VendorValueKind::Int32
        }
    );
    assert!(request.entries.is_empty());
}

#[test]
fn unsupported_key_write_is_refused_with_the_reason_and_writes_nothing() {
    let vendor = resolve_for("3", &FakeResolver::oneplus11());
    let mut request = FakeRequest::default();
    let error = vendor
        .write_value(VendorKey::DisableFpsLimits, VendorValue::Byte(1), &mut request)
        .unwrap_err();
    assert_eq!(
        error,
        VendorWriteError::Unsupported {
            key: VendorKey::DisableFpsLimits,
            reason: VendorUnsupportedReason::NotAllowedForCamera
        }
    );
    assert!(request.entries.is_empty(), "на ширике запрос остаётся чистым");
}

#[test]
fn request_rejection_is_reported_with_its_key_and_status() {
    let vendor = resolve_for("2", &FakeResolver::oneplus11());
    let mut request = FakeRequest {
        reject_tag: Some((DISABLE_FPS_LIMITS_TAG, -10001)),
        ..FakeRequest::default()
    };
    let error = vendor
        .write_value(VendorKey::DisableFpsLimits, VendorValue::Byte(1), &mut request)
        .unwrap_err();
    assert_eq!(
        error,
        VendorWriteError::Rejected {
            key: VendorKey::DisableFpsLimits,
            status: -10001
        }
    );
}

#[test]
fn report_lists_names_and_numbers_for_diagnostics() {
    let report = resolve_for("2", &FakeResolver::oneplus11()).report();
    assert_eq!(report.len(), VENDOR_TAG_SPECS.len());
    let json = serde_json::to_value(&report[0]).unwrap();
    assert_eq!(json["name"], "org.quic.camera.manualExposure.disableFPSLimits");
    assert_eq!(json["state"], "available");
    assert_eq!(json["tag"], DISABLE_FPS_LIMITS_TAG);
}

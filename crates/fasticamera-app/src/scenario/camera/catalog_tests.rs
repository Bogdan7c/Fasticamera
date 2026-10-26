//! Отчёт каталога и вердикт `camera-catalog`: форма отчёта (контракт со скриптами) и каждое правило провала.

use camera_android::BackendDiagnostics;
use camera_contract::capabilities::RequestCapability;
use camera_contract::catalog::RoleBinding;
use camera_contract::fixtures::{camera_id, oneplus11_visible_cameras};
use camera_contract::ids::LensRole;

use super::*;

fn catalog_with_roles() -> CameraCatalog {
    CameraCatalog::new(
        oneplus11_visible_cameras(),
        vec![
            RoleBinding {
                role: LensRole::Wide,
                camera: camera_id("2"),
            },
            RoleBinding {
                role: LensRole::UltraWide,
                camera: camera_id("3"),
            },
            RoleBinding {
                role: LensRole::Front,
                camera: camera_id("1"),
            },
        ],
    )
    .expect("каталог")
}

#[test]
fn healthy_catalog_has_no_violations() {
    assert_eq!(evaluate_catalog(&catalog_with_roles()), Vec::<String>::new());
}

#[test]
fn empty_catalog_is_a_violation() {
    let violations = evaluate_catalog(&CameraCatalog::empty());
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(violations[0].contains("каталог пуст"), "{violations:?}");
}

#[test]
fn camera_claiming_raw_without_raw_streams_is_a_violation() {
    let mut cameras = oneplus11_visible_cameras();
    cameras[2].capabilities.raw_streams.clear();
    let catalog = CameraCatalog::new(cameras, Vec::new()).expect("каталог");
    let violations = evaluate_catalog(&catalog);
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(
        violations[0].contains("камера 2") && violations[0].contains("RAW"),
        "{violations:?}"
    );
}

#[test]
fn camera_without_raw_capability_may_have_no_raw_streams() {
    let mut cameras = oneplus11_visible_cameras();
    cameras[1].capabilities.raw_streams.clear();
    cameras[1]
        .capabilities
        .request_capabilities
        .retain(|capability| *capability != RequestCapability::Raw);
    let catalog = CameraCatalog::new(cameras, Vec::new()).expect("каталог");
    assert_eq!(evaluate_catalog(&catalog), Vec::<String>::new());
}

#[test]
fn result_summary_has_the_fields_the_scripts_compare_against_dumpsys() {
    let catalog = catalog_with_roles();
    let backend = BackendDiagnostics::new(4).snapshot();
    let report = CatalogReport::build("closed", &catalog, &backend, camera_core::CameraCounters::default(), 3);
    let json = serde_json::to_value(&report).expect("json");

    let cameras = json["cameras"].as_array().expect("cameras");
    let ids: Vec<&str> = cameras
        .iter()
        .map(|camera| camera["id"].as_str().expect("id"))
        .collect();
    assert_eq!(ids, ["0", "1", "2", "3"]);
    let main = &cameras[2];
    assert_eq!(main["facing"], "back");
    assert_eq!(main["role"], "wide");
    assert_eq!(main["topology"]["kind"], "physical");
    assert_eq!(main["hardware_level"], "level3");
    assert_eq!(
        main["raw_streams"][0],
        serde_json::json!({"format": "raw10", "width": 4096, "height": 3072})
    );
    assert_eq!(main["raw_streams"][1]["format"], "raw16");
    assert!(main["widest_field_of_view_degrees"].as_f64().expect("fov") > 50.0);
    assert_eq!(cameras[0]["role"], serde_json::Value::Null, "у логической 0 роли нет");
    assert_eq!(cameras[0]["topology"]["kind"], "logical");
    assert_eq!(cameras[0]["topology"]["physical_ids"], serde_json::json!(["2", "3"]));
    assert_eq!(cameras[1]["facing"], "front");

    let roles: Vec<(&str, &str)> = json["roles"]
        .as_array()
        .expect("roles")
        .iter()
        .map(|entry| {
            (
                entry["role"].as_str().expect("role"),
                entry["camera"].as_str().expect("camera"),
            )
        })
        .collect();
    assert_eq!(roles, [("wide", "2"), ("ultra_wide", "3"), ("front", "1")]);
    assert_eq!(json["backend"]["callback_panics"], 0);
    assert_eq!(json["state"], "closed");
    assert_eq!(json["event_count"], 3);
}

#[test]
fn details_carry_the_full_characteristics_and_the_whole_backend_diagnostics() {
    let catalog = catalog_with_roles();
    let diagnostics = BackendDiagnostics::new(4);
    diagnostics.record_failure(
        "ACameraManager_getCameraCharacteristics",
        -3,
        "камера 9 не отдала характеристики",
    );
    diagnostics.record_skipped_camera("9", "нет характеристик".to_owned());
    diagnostics.record_camera(&camera_id("2"), Vec::new(), Vec::new());
    let details = CatalogDetails::build(&catalog, &diagnostics.snapshot());

    assert_eq!(details.cameras.len(), 4);
    let main = &details.cameras[2];
    assert_eq!(main["id"], "2");
    assert_eq!(main["raw"]["supported"], true);
    assert_eq!(
        main["raw"]["calibration_illuminants"].as_array().expect("наборы").len(),
        2
    );
    assert_eq!(main["lens_shading"], "applied_by_vendor");
    assert_eq!(
        main["sensor_physical_size"]["width_mm"].as_f64().map(|v| v > 0.0),
        Some(true)
    );
    assert!(main["raw_streams"][0]["min_frame_duration_ns"].as_u64().expect("ns") > 0);

    assert_eq!(details.backend.len(), 2, "общий элемент и разбор одной камеры");
    assert_eq!(
        details.backend[0]["recent_failures"][0]["operation"],
        "ACameraManager_getCameraCharacteristics"
    );
    assert_eq!(details.backend[0]["skipped_cameras"][0]["camera"], "9");
    assert_eq!(details.backend[1]["camera"], "2");
}

#[test]
fn backend_summary_counts_without_carrying_the_bulky_parts() {
    let diagnostics = BackendDiagnostics::new(4);
    diagnostics.record_failure("a", 1, "первый");
    diagnostics.record_failure("b", 2, "последний");
    diagnostics.record_camera(&camera_id("2"), Vec::new(), Vec::new());
    diagnostics.record_callback_panic();
    let snapshot = diagnostics.snapshot();
    let json = serde_json::to_value(BackendSummary::of(&snapshot)).expect("json");
    assert_eq!(json["callback_panics"], 1);
    assert_eq!(json["cameras_with_vendor_report"], 1);
    assert_eq!(json["failures_recorded"], 2);
    assert_eq!(json["last_failure"]["operation"], "b");
    assert!(json.get("cameras").is_none() && json.get("recent_failures").is_none());
}

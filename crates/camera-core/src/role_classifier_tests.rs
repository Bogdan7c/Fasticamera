//! Тесты назначения ролей на образцах OnePlus 11 и на сконструированных конфликтах.

use camera_contract::capabilities::RequestCapability;
use camera_contract::fixtures::{
    camera_id, front_camera, main_wide_camera, oneplus11_visible_cameras, ultra_wide_camera,
};
use camera_contract::geometry::PixelSize;
use camera_contract::ids::{LensFacing, LensRole};

use super::*;

fn config() -> RoleClassifierConfig {
    RoleClassifierConfig::default()
}

fn camera_for(role: LensRole, assignment: &RoleAssignment) -> Option<String> {
    assignment
        .bindings
        .iter()
        .find(|binding| binding.role == role)
        .map(|binding| binding.camera.to_string())
}

#[test]
fn oneplus11_gets_wide_on_2_ultra_wide_on_3_front_on_1_and_no_tele() {
    let assignment = assign_roles(&oneplus11_visible_cameras(), &config());
    assert_eq!(camera_for(LensRole::Wide, &assignment).as_deref(), Some("2"));
    assert_eq!(camera_for(LensRole::UltraWide, &assignment).as_deref(), Some("3"));
    assert_eq!(camera_for(LensRole::Front, &assignment).as_deref(), Some("1"));
    assert_eq!(camera_for(LensRole::Tele, &assignment), None);
    assert!(assignment.diagnostics.is_empty(), "{:?}", assignment.diagnostics);
    // Логическая камера 0 роли не получает.
    assert!(assignment.bindings.iter().all(|binding| binding.camera.as_str() != "0"));
}

#[test]
fn classified_catalog_resolves_roles_and_leaves_camera_0_addressable_by_id() {
    let classified = classify_catalog(oneplus11_visible_cameras(), &config()).expect("каталог собирается");
    let catalog = &classified.catalog;
    assert_eq!(catalog.role_of(&camera_id("2")), Some(LensRole::Wide));
    assert_eq!(catalog.role_of(&camera_id("0")), None);
    assert!(catalog.camera(&camera_id("0")).is_some());
    assert!(
        catalog
            .resolve(&camera_contract::ids::CameraTarget::Role(LensRole::Tele))
            .is_err()
    );
}

#[test]
fn system_camera_and_external_camera_get_no_role() {
    let mut system = main_wide_camera();
    system.id = camera_id("4");
    system
        .capabilities
        .request_capabilities
        .push(RequestCapability::SystemCamera);
    let mut external = ultra_wide_camera();
    external.id = camera_id("9");
    external.capabilities.facing = LensFacing::External;
    let assignment = assign_roles(&[system, external], &config());
    assert!(assignment.bindings.is_empty());
    assert!(assignment.diagnostics.is_empty());
}

#[test]
fn two_wide_cameras_conflict_and_the_larger_sensor_wins_with_a_diagnostic() {
    let big = main_wide_camera(); // 4096x3072, ID "2"
    let mut small = main_wide_camera();
    small.id = camera_id("7");
    small.capabilities.pixel_array = PixelSize::new(2000, 1500);
    let assignment = assign_roles(&[small, big], &config());
    assert_eq!(camera_for(LensRole::Wide, &assignment).as_deref(), Some("2"));
    assert_eq!(
        assignment.diagnostics,
        vec![RoleDiagnostic::RoleConflict {
            role: LensRole::Wide,
            winner: camera_id("2"),
            loser: camera_id("7"),
        }]
    );
}

#[test]
fn equal_sensors_conflict_is_resolved_by_smaller_id() {
    let first = main_wide_camera(); // "2"
    let mut second = main_wide_camera();
    second.id = camera_id("11");
    // "11" < "2" по строке: побеждает "11".
    let assignment = assign_roles(&[first, second], &config());
    assert_eq!(camera_for(LensRole::Wide, &assignment).as_deref(), Some("11"));
    assert_eq!(assignment.diagnostics.len(), 1);
}

#[test]
fn rear_camera_without_optics_gets_no_role_and_a_diagnostic() {
    let mut broken = main_wide_camera();
    broken.capabilities.optics.focal_lengths_mm.clear();
    let assignment = assign_roles(&[broken, front_camera()], &config());
    assert_eq!(camera_for(LensRole::Wide, &assignment), None);
    assert_eq!(camera_for(LensRole::Front, &assignment).as_deref(), Some("1"));
    assert_eq!(
        assignment.diagnostics,
        vec![RoleDiagnostic::FieldOfViewUnknown { camera: camera_id("2") }]
    );
}

#[test]
fn narrow_rear_camera_becomes_tele_and_thresholds_come_from_config() {
    let mut tele = main_wide_camera();
    tele.id = camera_id("5");
    tele.capabilities.optics.focal_lengths_mm = vec![15.0]; // ~36 градусов
    let assignment = assign_roles(&[tele.clone()], &config());
    assert_eq!(camera_for(LensRole::Tele, &assignment).as_deref(), Some("5"));

    // С порогом ширика ниже 36 градусов та же камера становится шириком: порог берётся из конфига.
    let strict = RoleClassifierConfig {
        ultra_wide_min_fov_degrees: 30.0,
        tele_max_fov_degrees: 20.0,
    };
    let assignment = assign_roles(&[tele], &strict);
    assert_eq!(camera_for(LensRole::UltraWide, &assignment).as_deref(), Some("5"));
}

#[test]
fn duplicate_camera_ids_from_backend_are_reported_as_error_not_panic() {
    let result = classify_catalog(vec![main_wide_camera(), main_wide_camera()], &config());
    assert!(result.is_err());
}

/// Подгоняет фокусное расстояние так, чтобы угол по длинной стороне сенсора был ровно `fov_degrees`.
fn with_field_of_view(
    mut camera: camera_contract::capabilities::CameraDescriptor,
    fov_degrees: f64,
) -> camera_contract::capabilities::CameraDescriptor {
    let optics = &mut camera.capabilities.optics;
    let long_side_mm = f64::from(
        optics
            .sensor_physical_size
            .width_mm
            .max(optics.sensor_physical_size.height_mm),
    );
    let focal_mm = long_side_mm / 2.0 / (fov_degrees.to_radians() / 2.0).tan();
    optics.focal_lengths_mm = vec![focal_mm as f32];
    camera
}

#[test]
fn angles_measured_on_the_phone_keep_their_roles_with_default_thresholds() {
    // Замер smoke `camera-catalog` (сессия 04): основная 72,46°, ширик 102,03°, фронт 71,00°.
    let cameras = vec![
        with_field_of_view(main_wide_camera(), 72.46),
        with_field_of_view(ultra_wide_camera(), 102.03),
        with_field_of_view(front_camera(), 71.00),
    ];
    let assignment = assign_roles(&cameras, &config());
    assert_eq!(camera_for(LensRole::Wide, &assignment).as_deref(), Some("2"));
    assert_eq!(camera_for(LensRole::UltraWide, &assignment).as_deref(), Some("3"));
    assert_eq!(camera_for(LensRole::Front, &assignment).as_deref(), Some("1"));
    // Запас порогов: ширик на 10° уже (92°) — всё ещё ширик, основная на 10° шире (82°) — всё ещё основная.
    let shifted = vec![
        with_field_of_view(main_wide_camera(), 82.46),
        with_field_of_view(ultra_wide_camera(), 92.03),
    ];
    let assignment = assign_roles(&shifted, &config());
    assert_eq!(camera_for(LensRole::Wide, &assignment).as_deref(), Some("2"));
    assert_eq!(camera_for(LensRole::UltraWide, &assignment).as_deref(), Some("3"));
}

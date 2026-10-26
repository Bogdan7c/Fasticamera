use super::*;
use crate::fixtures;
use crate::fixtures::camera_id;

fn binding(role: LensRole, camera: &str) -> RoleBinding {
    RoleBinding {
        role,
        camera: camera_id(camera),
    }
}

fn oneplus11_catalog() -> CameraCatalog {
    CameraCatalog::new(
        fixtures::oneplus11_visible_cameras(),
        vec![
            binding(LensRole::Wide, "2"),
            binding(LensRole::UltraWide, "3"),
            binding(LensRole::Front, "1"),
        ],
    )
    .expect("каталог OnePlus 11 корректен")
}

#[test]
fn resolve_by_role_and_by_id_finds_the_bound_camera() {
    let catalog = oneplus11_catalog();
    let wide = catalog
        .resolve(&CameraTarget::Role(LensRole::Wide))
        .expect("основная есть");
    assert_eq!(wide.id, camera_id("2"));
    let logical = catalog
        .resolve(&CameraTarget::Camera(camera_id("0")))
        .expect("логическая 0 есть");
    assert_eq!(logical.id, camera_id("0"));
    assert_eq!(catalog.role_of(&camera_id("3")), Some(LensRole::UltraWide));
    assert_eq!(catalog.role_of(&camera_id("0")), None);
}

#[test]
fn missing_tele_and_unknown_id_are_distinct_errors() {
    let catalog = oneplus11_catalog();
    assert_eq!(
        catalog.resolve(&CameraTarget::Role(LensRole::Tele)),
        Err(TargetUnavailable::RoleNotPresent(LensRole::Tele))
    );
    assert_eq!(
        catalog.resolve(&CameraTarget::Camera(camera_id("4"))),
        Err(TargetUnavailable::UnknownCamera(camera_id("4")))
    );
}

#[test]
fn constructor_rejects_each_broken_invariant() {
    let mut duplicated = fixtures::oneplus11_visible_cameras();
    duplicated.push(fixtures::main_wide_camera());
    assert_eq!(
        CameraCatalog::new(duplicated, Vec::new()),
        Err(CatalogError::DuplicateCamera(camera_id("2")))
    );

    assert_eq!(
        CameraCatalog::new(
            fixtures::oneplus11_visible_cameras(),
            vec![binding(LensRole::Wide, "2"), binding(LensRole::Wide, "0")]
        ),
        Err(CatalogError::DuplicateRole(LensRole::Wide))
    );

    assert_eq!(
        CameraCatalog::new(
            fixtures::oneplus11_visible_cameras(),
            vec![binding(LensRole::Tele, "4")]
        ),
        Err(CatalogError::RoleBoundToUnknownCamera {
            role: LensRole::Tele,
            camera: camera_id("4"),
        })
    );
}

#[test]
fn empty_catalog_resolves_nothing() {
    let catalog = CameraCatalog::empty();
    assert!(catalog.cameras().is_empty());
    assert_eq!(
        catalog.resolve(&CameraTarget::Role(LensRole::Front)),
        Err(TargetUnavailable::RoleNotPresent(LensRole::Front))
    );
}

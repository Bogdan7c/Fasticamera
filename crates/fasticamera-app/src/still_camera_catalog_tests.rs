//! Тесты описания камер для файлов: паспорт телефона, оптика из характеристик, роль в уникальном имени,
//! неизвестная камера, пустой на старте каталог.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "в тестах падение — способ сообщить о провале"
)]

use std::sync::Mutex;

use camera_contract::catalog::{CameraCatalog as EngineCatalog, RoleBinding};
use camera_contract::fixtures::{camera_id, main_wide_camera, oneplus11_visible_cameras, ultra_wide_camera};
use camera_contract::ids::LensRole;

use super::*;
use crate::device_config::DeviceConfig;

/// Каталог, который тест подменяет на ходу (как движок: сначала пустой, потом прочитанный).
struct SwappableCatalog(Mutex<Arc<EngineCatalog>>);

impl EngineCatalogSource for SwappableCatalog {
    fn current_catalog(&self) -> Arc<EngineCatalog> {
        Arc::clone(&self.0.lock().unwrap())
    }
}

fn device() -> DeviceIdentity {
    DeviceConfig {
        make: Some("OnePlus".to_owned()),
        model: Some("CPH2449".to_owned()),
    }
    .identity()
    .unwrap()
}

fn catalog_with_roles() -> Arc<EngineCatalog> {
    Arc::new(
        EngineCatalog::new(
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
            ],
        )
        .unwrap(),
    )
}

fn describer(source: Arc<SwappableCatalog>) -> EngineCameraCatalog {
    EngineCameraCatalog::new(source, device(), "Fasticamera 9.9.9".to_owned())
}

#[test]
fn main_camera_is_described_with_phone_identity_role_and_real_optics() {
    let source = Arc::new(SwappableCatalog(Mutex::new(catalog_with_roles())));
    let description = describer(source)
        .describe_camera(&camera_id("2"))
        .expect("камера в каталоге");

    assert_eq!(description.make, "OnePlus");
    assert_eq!(description.model, "CPH2449");
    assert_eq!(description.unique_camera_model, "OnePlus CPH2449 wide");
    assert_eq!(description.software, "Fasticamera 9.9.9");
    // Оптика основной камеры из образца характеристик: 5,59 мм, f/1,8.
    let lens = description.lens.expect("характеристики дали объектив");
    assert!((lens.focal_length_mm - main_wide_camera().capabilities.optics.focal_lengths_mm[0]).abs() < 1e-6);
    assert!((lens.f_number - 1.8).abs() < 1e-6);
}

#[test]
fn different_cameras_get_different_unique_names() {
    let source = Arc::new(SwappableCatalog(Mutex::new(catalog_with_roles())));
    let catalog = describer(source);
    let wide = catalog.describe_camera(&camera_id("2")).unwrap();
    let ultra = catalog.describe_camera(&camera_id("3")).unwrap();
    assert_ne!(wide.unique_camera_model, ultra.unique_camera_model);
    assert_eq!(ultra.unique_camera_model, "OnePlus CPH2449 ultra_wide");
    let ultra_optics = &ultra_wide_camera().capabilities.optics;
    assert!((ultra.lens.unwrap().focal_length_mm - ultra_optics.focal_lengths_mm[0]).abs() < 1e-6);
}

#[test]
fn camera_without_a_role_is_named_by_its_id() {
    let source = Arc::new(SwappableCatalog(Mutex::new(Arc::new(
        EngineCatalog::new(oneplus11_visible_cameras(), Vec::new()).unwrap(),
    ))));
    let description = describer(source).describe_camera(&camera_id("2")).unwrap();
    assert_eq!(description.unique_camera_model, "OnePlus CPH2449 camera 2");
}

#[test]
fn unknown_camera_and_empty_startup_catalog_give_none_not_a_guess() {
    let source = Arc::new(SwappableCatalog(Mutex::new(Arc::new(EngineCatalog::empty()))));
    let catalog = describer(Arc::clone(&source));
    assert!(
        catalog.describe_camera(&camera_id("2")).is_none(),
        "на старте каталог пуст"
    );

    // Каталог прочитан позже: тот же объект описаний сразу его видит (читается на каждый запрос).
    *source.0.lock().unwrap() = catalog_with_roles();
    assert!(catalog.describe_camera(&camera_id("2")).is_some());
    assert!(
        catalog.describe_camera(&camera_id("99")).is_none(),
        "камеры нет в каталоге"
    );
}

#[test]
fn software_line_carries_the_crate_version() {
    assert_eq!(
        software_description(),
        format!("Fasticamera {}", env!("CARGO_PKG_VERSION"))
    );
}

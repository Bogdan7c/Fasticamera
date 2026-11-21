//! Шкала зума на настоящих образцах OnePlus 11 и синтетических камерах.

use camera_contract::capabilities::CameraDescriptor;
use camera_contract::catalog::CameraCatalog;
use camera_contract::fixtures::{
    camera_id, front_camera, logical_back_camera, main_wide_camera, oneplus11_visible_cameras, ultra_wide_camera,
};
use camera_contract::ids::{LensFacing, LensRole};
use camera_contract::lens_calibration::{OpticalAxis, UncalibratedReason};
use camera_contract::viewfinder_crop::{CropAspect, CropCenter};
use camera_contract::zoom::{PreviewMirror, ZoomCatalog, ZoomFraming, ZoomRatio, ZoomStopKind};
use std::sync::Arc;

use super::{ZoomConfig, ZoomDiagnostic, build_zoom_catalog, build_zoom_catalog_reporting};
use crate::config::RoleClassifierConfig;
use crate::role_classifier::classify_catalog;

fn catalog_of(cameras: Vec<CameraDescriptor>) -> CameraCatalog {
    classify_catalog(cameras, &RoleClassifierConfig::default())
        .expect("образцы классифицируются")
        .catalog
}

/// Синтетический телевик ≈ 2×: 7,08 мм на сенсоре 5,248 × 3,942 мм (угол ≈ 40°, роль `Tele`).
fn synthetic_tele() -> CameraDescriptor {
    let mut tele = main_wide_camera();
    tele.id = camera_id("9");
    tele.capabilities.optics.focal_lengths_mm = vec![7.08];
    tele.capabilities.optics.sensor_physical_size.width_mm = 5.248;
    tele.capabilities.optics.sensor_physical_size.height_mm = 3.942;
    tele
}

fn real_catalog() -> ZoomCatalog {
    build_zoom_catalog(&catalog_of(oneplus11_visible_cameras()), &ZoomConfig::default())
}

#[test]
fn real_oneplus11_back_group_matches_the_measured_scale() {
    let zoom = real_catalog();
    let back = zoom.back.expect("задняя группа");
    assert_eq!(back.facing, LensFacing::Back);
    let roles: Vec<LensRole> = back.lenses.iter().map(|lens| lens.role).collect();
    assert_eq!(
        roles,
        vec![LensRole::UltraWide, LensRole::Wide],
        "логическая 0 в шкалу не входит"
    );

    let ultra_wide = back.lens_for_role(LensRole::UltraWide).expect("ширик");
    assert!(
        (ultra_wide.native_ratio.value() - 0.593).abs() < 0.002,
        "ширик ≈ 0,593×, получено {}",
        ultra_wide.native_ratio.value()
    );
    assert!((ultra_wide.equivalent_focal_mm - 14.0).abs() < 0.1);
    let wide = back.lens_for_role(LensRole::Wide).expect("основная");
    assert_eq!(wide.native_ratio.value(), 1.0);

    let stops: Vec<(f32, ZoomStopKind)> = back.stops.iter().map(|stop| (stop.ratio.value(), stop.kind)).collect();
    assert_eq!(stops.len(), 3);
    assert!((stops[0].0 - 0.593).abs() < 0.002);
    assert_eq!(stops[0].1, ZoomStopKind::Lens(LensRole::UltraWide));
    assert_eq!(stops[1], (1.0, ZoomStopKind::Lens(LensRole::Wide)));
    assert_eq!(stops[2], (2.0, ZoomStopKind::DigitalCrop));

    assert!((back.range.min.value() - 0.593).abs() < 0.002);
    assert_eq!(back.range.max.value(), 10.0);
}

#[test]
fn real_oneplus11_front_group_is_mirrored_and_capped_at_three() {
    let zoom = real_catalog();
    let front = zoom.front.expect("фронтальная группа");
    assert_eq!(front.facing, LensFacing::Front);
    assert_eq!(front.lenses.len(), 1);
    let lens = &front.lenses[0];
    assert_eq!(lens.role, LensRole::Front);
    assert_eq!(lens.native_ratio.value(), 1.0);
    assert_eq!(lens.preview_mirror, PreviewMirror::Horizontal);
    assert_eq!(
        lens.max_digital_zoom.value(),
        3.0,
        "предел фронта из конфига, а не 10 от HAL"
    );
    assert_eq!(front.range.min.value(), 1.0);
    assert_eq!(front.range.max.value(), 3.0);
    assert_eq!(front.stops.len(), 1, "у фронта ленты зума нет: кропов 2× нет");
}

#[test]
fn optical_axis_is_calibrated_for_ultra_wide_and_honestly_missing_for_front() {
    let zoom = real_catalog();
    let back = zoom.back.expect("задняя группа");
    let ultra_wide = back.lens_for_role(LensRole::UltraWide).expect("ширик");
    let OpticalAxis::Calibrated { point } = ultra_wide.optical_axis else {
        panic!("ось ширика должна быть откалибрована");
    };
    assert!(point.x() < 0.5, "ось ширика смещена влево (≈ 1759 из 4000)");
    assert_eq!(ultra_wide.preview_mirror, PreviewMirror::None);

    let front = zoom.front.expect("фронтальная группа");
    assert!(
        matches!(front.lenses[0].optical_axis, OpticalAxis::Uncalibrated { .. }),
        "заглушка HAL 0,0 — не калибровка"
    );
}

#[test]
fn synthetic_tele_becomes_a_lens_stop_and_replaces_the_digital_crop() {
    let mut cameras = oneplus11_visible_cameras();
    cameras.push(synthetic_tele());
    let build = build_zoom_catalog_reporting(&catalog_of(cameras), &ZoomConfig::default());
    let back = build.catalog.back.expect("задняя группа");
    let tele = back.lens_for_role(LensRole::Tele).expect("телевик в шкале");
    assert!(
        (tele.native_ratio.value() - 1.976).abs() < 0.01,
        "получено {}",
        tele.native_ratio.value()
    );

    let kinds: Vec<ZoomStopKind> = back.stops.iter().map(|stop| stop.kind).collect();
    assert_eq!(
        kinds,
        vec![
            ZoomStopKind::Lens(LensRole::UltraWide),
            ZoomStopKind::Lens(LensRole::Wide),
            ZoomStopKind::Lens(LensRole::Tele),
        ],
        "кроп 2× не создаётся рядом с настоящим телевиком"
    );
    assert!(build.diagnostics.is_empty());
}

#[test]
fn camera_without_focal_length_is_excluded_with_a_diagnostic() {
    let mut broken_front = front_camera();
    broken_front.capabilities.optics.focal_lengths_mm.clear();
    let build = build_zoom_catalog_reporting(
        &catalog_of(vec![main_wide_camera(), ultra_wide_camera(), broken_front]),
        &ZoomConfig::default(),
    );
    assert!(
        build.catalog.front.is_none(),
        "без опорной камеры фронтальной шкалы нет"
    );
    assert!(build.diagnostics.iter().any(|diagnostic| matches!(
        diagnostic,
        ZoomDiagnostic::FocalLengthUnknown { camera, role: LensRole::Front } if *camera == camera_id("1")
    )));
    assert!(build.diagnostics.iter().any(|diagnostic| matches!(
        diagnostic,
        ZoomDiagnostic::ReferenceLensMissing {
            facing: LensFacing::Front
        }
    )));
    assert!(build.catalog.back.is_some(), "задняя группа не пострадала");
}

#[test]
fn missing_hal_digital_zoom_means_no_digital_zoom_and_no_crop_stop() {
    let mut wide = main_wide_camera();
    wide.capabilities.optics.max_digital_zoom = None;
    let build = build_zoom_catalog_reporting(&catalog_of(vec![wide]), &ZoomConfig::default());
    let back = build.catalog.back.expect("задняя группа");
    assert_eq!(back.lenses[0].max_digital_zoom.value(), 1.0);
    assert_eq!(back.range.max.value(), 1.0);
    assert_eq!(back.stops.len(), 1, "кроп 2× вне дальности — упора нет");
    assert!(build.diagnostics.iter().any(|diagnostic| matches!(
        diagnostic,
        ZoomDiagnostic::MaxDigitalZoomNotReported { camera } if *camera == camera_id("2")
    )));
}

#[test]
fn config_caps_the_range_and_hal_limit_wins_when_lower() {
    let mut wide = main_wide_camera();
    wide.capabilities.optics.max_digital_zoom = Some(4.0);
    let zoom = build_zoom_catalog(&catalog_of(vec![wide.clone()]), &ZoomConfig::default());
    assert_eq!(
        zoom.back.expect("задняя").range.max.value(),
        4.0,
        "HAL ниже предела конфига"
    );

    let config = ZoomConfig {
        back_max_ratio: 5.0,
        ..ZoomConfig::default()
    };
    wide.capabilities.optics.max_digital_zoom = Some(10.0);
    let zoom = build_zoom_catalog(&catalog_of(vec![wide]), &config);
    assert_eq!(
        zoom.back.expect("задняя").range.max.value(),
        5.0,
        "предел конфига ниже HAL"
    );
}

#[test]
fn logical_camera_alone_gives_an_empty_catalog() {
    let zoom = build_zoom_catalog(&catalog_of(vec![logical_back_camera()]), &ZoomConfig::default());
    assert_eq!(zoom, ZoomCatalog::default());
}

fn config_distrusting(cameras: &[&str]) -> ZoomConfig {
    ZoomConfig {
        untrusted_optical_axis_cameras: cameras.iter().map(|id| (*id).to_owned()).collect(),
        ..ZoomConfig::default()
    }
}

/// Центр кадрирования камеры `camera_name` на 1× по готовой шкале (как считает экран и рендер).
fn crop_center_at_one_x(zoom: ZoomCatalog, camera_name: &str) -> CropCenter {
    ZoomFraming {
        aspect: CropAspect::FOUR_BY_THREE,
        facing: LensFacing::Back,
        ratio: ZoomRatio::new(1.0).expect("валидный коэффициент"),
        catalog: Arc::new(zoom),
    }
    .crop_for_camera(&camera_id(camera_name))
    .crop
    .center
}

#[test]
fn profile_distrusted_axis_frames_by_sensor_centre_and_leaves_other_cameras_alone() {
    let catalog = catalog_of(oneplus11_visible_cameras());
    let zoom = build_zoom_catalog(&catalog, &config_distrusting(&["3"]));
    let back = zoom.back.clone().expect("задняя группа");

    let ultra_wide = back.lens_for_role(LensRole::UltraWide).expect("ширик");
    assert_eq!(
        ultra_wide.optical_axis,
        OpticalAxis::Uncalibrated {
            reason: UncalibratedReason::DistrustedByProfile
        },
        "калибровка HAL у ширика правдоподобна, но профиль ей не верит"
    );
    assert_eq!(crop_center_at_one_x(zoom.clone(), "3"), CropCenter::ValidAreaCenter);

    // Остальные камеры считаются как без профиля: у основной ось по калибровке, у фронта — заглушка.
    let reference = build_zoom_catalog(&catalog, &ZoomConfig::default());
    let main_axis = |zoom: &ZoomCatalog| {
        zoom.back
            .as_ref()
            .and_then(|group| group.lens_for_role(LensRole::Wide))
            .map(|lens| lens.optical_axis)
    };
    assert_eq!(main_axis(&zoom), main_axis(&reference));
    assert_eq!(zoom.front, reference.front);
    // Без профиля ось ширика по-прежнему настоящая (регресс: список пуст по умолчанию).
    let default_ultra_wide = reference
        .back
        .expect("задняя группа")
        .lens_for_role(LensRole::UltraWide)
        .cloned();
    assert!(matches!(
        default_ultra_wide.expect("ширик").optical_axis,
        OpticalAxis::Calibrated { .. }
    ));
}

#[test]
fn profile_naming_a_camera_absent_from_the_catalog_is_reported_not_applied() {
    let build = build_zoom_catalog_reporting(&catalog_of(oneplus11_visible_cameras()), &config_distrusting(&["77"]));
    assert!(
        build
            .diagnostics
            .contains(&ZoomDiagnostic::UntrustedCameraNotInCatalog {
                camera: "77".to_owned()
            })
    );
    let ultra_wide = build
        .catalog
        .back
        .expect("группа")
        .lens_for_role(LensRole::UltraWide)
        .cloned();
    assert!(matches!(
        ultra_wide.expect("ширик").optical_axis,
        OpticalAxis::Calibrated { .. }
    ));
}

//! Ожидание каталога камер: причина «запасного сенсора» воспроизведена на настоящем движке с подставным бэкендом.

use std::sync::Arc;

use camera_contract::catalog::CameraCatalog;
use camera_contract::lens_shading::LensShadingCalibrations;
use render_loop::config::RenderLoopConfig;

use super::*;
use crate::camera_screen_config::CameraScreenConfig;
use crate::camera_test_support::{FakeCameraRig, WAIT};
use crate::saturation_bench::inputs::{InputSources, SensorSource, gather_inputs};

/// Снимок движка, который уже прочитал каталог OnePlus 11.
fn loaded_status(rig: &FakeCameraRig) -> CameraStatus {
    rig.handle()
        .wait_until_status(WAIT, catalog_published)
        .expect("каталог камер прочитан")
}

/// Снимок, каким он выглядит до первой публикации движка: каталог пуст (`EngineOutput::new`).
fn status_before_first_publication(rig: &FakeCameraRig) -> CameraStatus {
    CameraStatus {
        catalog: Arc::new(CameraCatalog::empty()),
        ..loaded_status(rig)
    }
}

fn sensor_of(status: &CameraStatus) -> SensorSource {
    let render = RenderLoopConfig::default();
    let screen = CameraScreenConfig::default();
    let none = LensShadingCalibrations::none();
    gather_inputs(&InputSources {
        engine_status: Some(status),
        lens_shading: &none,
        render_config: &render,
        look: None,
        screen: &screen,
    })
    .sensor_source
}

#[test]
fn an_empty_catalog_waits_then_the_published_one_gives_the_real_camera_sensor() {
    let rig = FakeCameraRig::new();
    let wait = CatalogWait::starting_at(Instant::now(), Duration::from_secs(5));
    let early = status_before_first_publication(&rig);
    // До публикации сценарий ждёт, а не берёт запасной набор.
    assert_eq!(wait.poll(early.clone(), Instant::now()), CatalogPoll::Waiting);
    // Без ожидания входы брались бы по этому снимку и сенсор был бы запасным: так выглядела причина на телефоне.
    let SensorSource::Fallback { reason } = sensor_of(&early) else {
        panic!("по пустому каталогу сенсор обязан быть запасным");
    };
    assert!(
        reason.contains("каталог камер пуст"),
        "причина объясняет, почему запасной: {reason}"
    );
    // Каталог опубликован: ожидание отдаёт снимок, а входы по нему — калибровка настоящей основной камеры.
    let CatalogPoll::Ready(published) = wait.poll(loaded_status(&rig), Instant::now()) else {
        panic!("каталог есть: ожидание обязано закончиться");
    };
    let SensorSource::Engine { camera, .. } = sensor_of(&published) else {
        panic!("после ожидания сенсор обязан быть от настоящей камеры");
    };
    assert_eq!(camera, "2", "основная камера OnePlus 11");
}

#[test]
fn the_wait_ends_at_the_deadline_with_the_catalog_it_has_and_the_fallback_explains_itself() {
    let rig = FakeCameraRig::new();
    let started = Instant::now();
    let wait = CatalogWait::starting_at(started, Duration::from_millis(500));
    let early = status_before_first_publication(&rig);
    assert_eq!(
        wait.poll(early.clone(), started + Duration::from_millis(499)),
        CatalogPoll::Waiting
    );
    assert_eq!(
        wait.poll(early.clone(), started + Duration::from_millis(500)),
        CatalogPoll::TimedOut(early),
        "срок вышел: ждать вечно нельзя"
    );
    // Опубликованный каталог всегда важнее срока: даже после дедлайна берём настоящий.
    assert!(matches!(
        wait.poll(loaded_status(&rig), started + Duration::from_secs(60)),
        CatalogPoll::Ready(_)
    ));
}

#[test]
fn describe_catalog_names_cameras_and_bound_roles_so_a_phone_run_shows_why_a_role_is_missing() {
    let rig = FakeCameraRig::new();
    let loaded = loaded_status(&rig);
    let text = describe_catalog(&loaded.catalog);
    assert!(text.contains("wide=2"), "основная роль привязана к камере 2: {text}");
    assert!(text.contains("в каталоге 4 камер"), "{text}");
    assert!(describe_catalog(&CameraCatalog::empty()).contains("пуст"));
}

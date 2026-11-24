//! Входы «как у продукта»: сенсор (каталог движка + карта теней устройства), вид и основа тона, запасной набор.

use camera_contract::catalog::CameraCatalog;
use camera_contract::fixtures::{front_camera, main_wide_camera, ultra_wide_camera};
use camera_core::role_classifier::classify_catalog;
use look_contract::{GlobalKnob, LookIntent, LookKnob};
use look_presets::PresetStoreConfig;

use super::*;
use crate::device_config::DeviceConfig;
use crate::lens_shading_profile::embedded_lens_shading;
use crate::look_startup::catalog_environment;

fn oneplus_catalog() -> CameraCatalog {
    classify_catalog(
        vec![main_wide_camera(), ultra_wide_camera(), front_camera()],
        &camera_core::config::CameraEngineConfig::default().roles,
    )
    .expect("образцы камер без повторов")
    .catalog
}

fn oneplus_device() -> DeviceConfig {
    DeviceConfig {
        make: Some("OnePlus".to_owned()),
        model: Some("CPH2447".to_owned()),
    }
}

fn sources<'a>(
    render: &'a RenderLoopConfig,
    screen: &'a CameraScreenConfig,
    lens_shading: &'a LensShadingCalibrations,
    look: Option<&'a LookHandle>,
) -> InputSources<'a> {
    InputSources {
        engine_status: None,
        lens_shading,
        render_config: render,
        look,
        screen,
    }
}

#[test]
fn fallback_set_resolves_isp_params_for_both_pipelines() {
    let render = RenderLoopConfig::default();
    let screen = CameraScreenConfig::default();
    let none = LensShadingCalibrations::none();
    let inputs = gather_inputs(&sources(&render, &screen, &none, None));
    assert!(matches!(&inputs.sensor_source, SensorSource::Fallback { reason } if reason.contains("движок")));
    let viewfinder = inputs.params(PipelineKind::Viewfinder).expect("видоискатель");
    let quality = inputs.params(PipelineKind::Quality).expect("качественный конвейер");
    assert_ne!(viewfinder, quality, "демозаик Preview и Quality дают разные параметры");
    assert_eq!(inputs.look.preset, "none", "библиотеки пресетов в этом входе нет");
}

#[test]
fn engine_catalog_gives_the_real_main_camera_sensor_with_the_device_lens_shading_map() {
    let catalog = oneplus_catalog();
    let calibrations = embedded_lens_shading(&oneplus_device()).expect("профиль устройства");
    let (sensor, source, geometry) = sensor_from_catalog(&catalog, &calibrations).expect("сенсор из каталога");
    assert_eq!(
        source,
        SensorSource::Engine {
            camera: "2".to_owned(),
            lens_shading_calibrated: true
        }
    );
    assert!(
        sensor.lens_shading_map().is_some(),
        "карта теней объектива — как у продукта"
    );
    let geometry = geometry.expect("геометрия потока из паспорта");
    assert_eq!(
        (geometry.valid_area.size.width, geometry.valid_area.size.height),
        (4096, 3072)
    );

    // Профиль чужого телефона карту не даёт: об этом честно говорит результат, а не молчаливая подмена.
    let foreign = embedded_lens_shading(&DeviceConfig {
        make: Some("Other".to_owned()),
        model: Some("X1".to_owned()),
    })
    .expect("чужое устройство — не ошибка");
    let (_, foreign_source, _) = sensor_from_catalog(&catalog, &foreign).expect("сенсор без карты");
    assert_eq!(
        foreign_source,
        SensorSource::Engine {
            camera: "2".to_owned(),
            lens_shading_calibrated: false
        }
    );
}

#[test]
fn empty_catalog_falls_back_with_the_reason_in_the_result() {
    let error = sensor_from_catalog(&CameraCatalog::empty(), &LensShadingCalibrations::none()).expect_err("пусто");
    assert!(error.contains("основной роли нет в каталоге"), "{error}");
}

#[test]
fn active_look_of_the_product_reaches_the_isp_params_and_the_summary() {
    let render = RenderLoopConfig::default();
    let screen = CameraScreenConfig::default();
    let none = LensShadingCalibrations::none();
    let look = LookHandle::in_memory(&PresetStoreConfig::default(), &catalog_environment(&render)).expect("библиотека");
    let neutral = gather_inputs(&sources(&render, &screen, &none, Some(&look)));
    assert_ne!(neutral.look.preset, "none");
    assert!(neutral.look_facts.is_some(), "экран калибровки получит факты о виде");
    assert_eq!(neutral.look.tone_base, look.active_recipe().base);

    // Владелец крутит ручку в библиотеке: следующие входы стенда несут уже другой вид, и ISP считает другие параметры.
    look.apply(LookIntent::SetKnob {
        knob: GlobalKnob::Contrast,
        value: LookKnob::new(0.8).expect("ручка в диапазоне"),
    })
    .expect("намерение принято");
    let edited = gather_inputs(&sources(&render, &screen, &none, Some(&look)));
    assert_ne!(
        neutral.params(PipelineKind::Viewfinder).expect("до"),
        edited.params(PipelineKind::Viewfinder).expect("после"),
        "вид из библиотеки пресетов действительно входит в параметры ISP"
    );
}

#[test]
fn camera_facts_are_live_with_a_stream_geometry_even_without_an_engine() {
    let render = RenderLoopConfig::default();
    let screen = CameraScreenConfig::default();
    let none = LensShadingCalibrations::none();
    let inputs = gather_inputs(&sources(&render, &screen, &none, None));
    assert_eq!(inputs.camera_facts.presence, CameraPresence::Live);
    let geometry = inputs.camera_facts.stream.expect("геометрия потока есть");
    assert_eq!(geometry.sensor_orientation, SensorOrientation::Degrees90);
}

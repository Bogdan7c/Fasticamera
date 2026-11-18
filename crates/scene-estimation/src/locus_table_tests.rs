//! Таблица локуса: раскладка для шейдера, положение известных белых, края, ошибки.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "в тестах падение — сигнал провала"
)]

use camera_contract::capabilities::RawSupport;
use camera_contract::fixtures::main_wide_camera;
use isp_model::color::chromaticity::Chromaticity;
use isp_model::color::dng::DngColorProfile;
use isp_model::color::temperature::Kelvin;
use isp_model::color::white_point::{CctDuv, Duv, chromaticity_from_cct_duv};

use super::*;

fn oneplus_profile() -> DngColorProfile {
    let RawSupport::Supported(info) = main_wide_camera().capabilities.raw else {
        panic!("в образце основной камеры есть RAW");
    };
    DngColorProfile::from_calibration_sets(&info.color_calibration).expect("профиль цвета")
}

fn table() -> CameraLocusTable {
    CameraLocusTable::build(&oneplus_profile(), &LocusTableConfig::default(), 0.02).expect("таблица")
}

fn white(kelvin: f64, duv: f64) -> Chromaticity {
    chromaticity_from_cct_duv(CctDuv {
        temperature: Kelvin(kelvin),
        duv: Duv(duv),
    })
    .expect("белая в таблице Робертсона")
}

#[test]
fn segment_layout_matches_the_shader_struct() {
    assert_eq!(std::mem::size_of::<LocusSegment>(), 48, "12 f32 подряд, без дополнения");
    assert_eq!(std::mem::align_of::<LocusSegment>(), 4);
    assert_eq!(std::mem::offset_of!(LocusSegment, origin), 0);
    assert_eq!(std::mem::offset_of!(LocusSegment, direction), 8);
    assert_eq!(std::mem::offset_of!(LocusSegment, normal), 16);
    assert_eq!(std::mem::offset_of!(LocusSegment, inverse), 24);
    assert_eq!(std::mem::offset_of!(LocusSegment, mired_start), 40);
    assert_eq!(std::mem::offset_of!(LocusSegment, mired_span), 44);
}

#[test]
fn table_covers_the_configured_range_with_uniform_mired_steps() {
    let table = table();
    let config = LocusTableConfig::default();
    assert_eq!(table.segments().len(), config.segments as usize);
    let (cold, warm) = table.mired_range();
    assert!((f64::from(cold) - 1.0e6 / config.max_temperature_kelvin).abs() < 1e-3);
    assert!((f64::from(warm) - 1.0e6 / config.min_temperature_kelvin).abs() < 1e-3);
    let span = table.segments()[0].mired_span;
    assert!(span > 0.0);
    for window in table.segments().windows(2) {
        assert!(
            (window[0].mired_start + window[0].mired_span - window[1].mired_start).abs() < 1e-3,
            "отрезки смыкаются"
        );
        assert!((window[1].mired_span - span).abs() < 1e-3, "шаг по майредам одинаков");
    }
}

#[test]
fn locate_recovers_temperature_and_duv_of_known_whites() {
    let profile = oneplus_profile();
    let table = table();
    for (kelvin, duv) in [
        (2200.0, 0.0),
        (2856.0, 0.0),
        (4000.0, 0.01),
        (5500.0, -0.015),
        (6500.0, 0.0),
        (6500.0, 0.03),
        (9000.0, -0.02),
    ] {
        let point = CameraLocusTable::log_chroma_of(&profile, white(kelvin, duv)).expect("нейтраль");
        let position = table.locate(point);
        let expected_mired = Kelvin(kelvin).mired();
        assert!(!position.off_table, "{kelvin} К внутри таблицы");
        assert!(
            // Внутри отрезка изотерма линейна по Duv; настоящая слегка искривлена, ошибка растёт с |Duv|
            // (измерено: 2,8 майред при Duv −0,015). Для оценки света это на порядок меньше порога видимости.
            (f64::from(position.mired) - expected_mired).abs() < 1.0 + 200.0 * duv.abs(),
            "{kelvin} К, Duv {duv}: майред {} вместо {expected_mired}",
            position.mired
        );
        assert!(
            (f64::from(position.duv) - duv).abs() < 0.001,
            "{kelvin} К, Duv {duv}: Duv {}",
            position.duv
        );
    }
}

#[test]
fn light_outside_the_table_is_reported_off_table() {
    let profile = oneplus_profile();
    let table = table();
    // 1700 К теплее самой тёплой точки таблицы (2000 К), 20000 К холоднее самой холодной (12000 К).
    for kelvin in [1700.0, 20_000.0] {
        let point = CameraLocusTable::log_chroma_of(&profile, white(kelvin, 0.0)).expect("нейтраль");
        assert!(table.locate(point).off_table, "{kelvin} К вне таблицы");
    }
}

#[test]
fn invalid_config_is_an_error_not_a_clamped_table() {
    let profile = oneplus_profile();
    let config = LocusTableConfig {
        segments: 1,
        ..LocusTableConfig::default()
    };
    assert!(matches!(
        CameraLocusTable::build(&profile, &config, 0.02),
        Err(LocusTableError::Config(_))
    ));
    // Температура холоднее границы таблицы Робертсона (≈ 1667 К) — полоса не строится.
    let too_warm = LocusTableConfig {
        min_temperature_kelvin: 1000.0,
        ..LocusTableConfig::default()
    };
    assert!(CameraLocusTable::build(&profile, &too_warm, 0.02).is_err());
}

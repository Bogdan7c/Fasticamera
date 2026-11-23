//! Виньетирование в кэше параметров: карта потока из калибровки паспорта, ключ пересчёта и общая карта потока.

use std::sync::Arc;

use camera_contract::lens_shading::{
    LensShadingCalibration, LensShadingCalibrationState, LensShadingGainGrid, NotCalibratedReason,
};
use isp_model::lens_shading::{CorrectionStrength, LensShadingControl, LensShadingSkipReason, LensShadingStep};

use super::tests::{DAYLIGHT, calibration, frame, plain, stream};
use super::*;
use camera_contract::fixtures::camera_id;
use camera_contract::geometry::PixelSize;

/// Калибровка камеры `camera` на активной области 64×48 (как у тестового потока): левый верх ярче правого низа.
fn calibration_of(camera: &str, corner_gain: f32) -> LensShadingCalibration {
    LensShadingCalibration {
        camera: camera_id(camera),
        active_array: PixelSize::new(64, 48),
        grid: LensShadingGainGrid::new(2, 2, vec![[corner_gain; 4], [1.5; 4], [2.0; 4], [1.0; 4]]).unwrap(),
    }
}

fn calibrated_stream(camera: &str, corner_gain: f32) -> Arc<StreamPassport> {
    let mut passport = (*stream(calibration())).clone();
    passport.camera = camera_id(camera);
    passport.lens_shading = LensShadingCalibrationState::Calibrated(Arc::new(calibration_of(camera, corner_gain)));
    Arc::new(passport)
}

fn applied_map(cache: &IspParamsCache) -> Arc<isp_model::lens_shading::LensShadingGainMap> {
    match &cache.current().expect("параметры есть").lens_shading {
        LensShadingStep::Apply(map) => Arc::clone(map),
        other => panic!("ожидали Apply, получили {other:?}"),
    }
}

#[test]
fn calibrated_stream_gets_its_map_and_uncalibrated_stream_is_skipped_with_a_reason() {
    let stream = calibrated_stream("2", 4.0);
    let mut cache = IspParamsCache::default();
    cache.prepare(&frame(&stream, plain()), DAYLIGHT, DisplayGamut::Srgb);
    let map = applied_map(&cache);
    // Активная область совпадает с буфером — карта потока равна калибровке.
    assert_eq!(map.points(), calibration_of("2", 4.0).grid.points());

    let uncalibrated = self::stream(calibration());
    let mut cache = IspParamsCache::default();
    cache.prepare(&frame(&uncalibrated, plain()), DAYLIGHT, DisplayGamut::Srgb);
    assert_eq!(
        cache.current().unwrap().lens_shading,
        LensShadingStep::Skip(LensShadingSkipReason::NotCalibrated)
    );
    let foreign = {
        let mut passport = (*uncalibrated).clone();
        passport.lens_shading = LensShadingCalibrationState::NotCalibrated(NotCalibratedReason::DeviceNotInProfile);
        Arc::new(passport)
    };
    let mut cache = IspParamsCache::default();
    cache.prepare(&frame(&foreign, plain()), DAYLIGHT, DisplayGamut::Srgb);
    assert_eq!(
        cache.current().unwrap().lens_shading,
        LensShadingStep::Skip(LensShadingSkipReason::NotCalibrated)
    );
}

#[test]
fn changing_strength_or_mode_invalidates_the_cached_params() {
    let stream = calibrated_stream("2", 4.0);
    let frame = frame(&stream, plain());
    let mut cache = IspParamsCache::default();
    assert_eq!(
        cache.prepare(&frame, DAYLIGHT, DisplayGamut::Srgb).outcome,
        ParamsOutcome::Resolved
    );
    let full = applied_map(&cache);
    assert_eq!(
        cache.prepare(&frame, DAYLIGHT, DisplayGamut::Srgb).outcome,
        ParamsOutcome::Unchanged,
        "то же управление — без пересчёта"
    );

    cache.set_lens_shading(LensShadingControl::Enabled(CorrectionStrength::new(0.5).unwrap()));
    assert_eq!(
        cache.prepare(&frame, DAYLIGHT, DisplayGamut::Srgb).outcome,
        ParamsOutcome::Resolved,
        "смена силы пересчитывает параметры"
    );
    let half = applied_map(&cache);
    assert_eq!(half.points()[0][0], 1.0 + (full.points()[0][0] - 1.0) * 0.5);
    assert!(!Arc::ptr_eq(&half, &full));

    cache.set_lens_shading(LensShadingControl::Disabled);
    assert_eq!(
        cache.prepare(&frame, DAYLIGHT, DisplayGamut::Srgb).outcome,
        ParamsOutcome::Resolved
    );
    assert_eq!(
        cache.current().unwrap().lens_shading,
        LensShadingStep::Skip(LensShadingSkipReason::DisabledByControl)
    );
}

#[test]
fn frames_of_one_stream_share_one_map_allocation() {
    let stream = calibrated_stream("2", 4.0);
    let mut cache = IspParamsCache::default();
    cache.prepare(&frame(&stream, plain()), DAYLIGHT, DisplayGamut::Srgb);
    let first = applied_map(&cache);
    // Другой охват экрана пересчитывает параметры, но карта потока строится не заново.
    cache.prepare(&frame(&stream, plain()), DAYLIGHT, DisplayGamut::DisplayP3);
    let second = applied_map(&cache);
    assert!(
        Arc::ptr_eq(&first, &second),
        "карта потока общая для всех кадров потока"
    );
}

#[test]
fn still_and_viewfinder_caches_get_the_same_map_for_the_same_stream() {
    let stream = calibrated_stream("2", 4.0);
    let mut viewfinder = IspParamsCache::default();
    let mut still = IspParamsCache::for_demosaic(DemosaicMethod::Quality);
    viewfinder.prepare(&frame(&stream, plain()), DAYLIGHT, DisplayGamut::Srgb);
    still.prepare(&frame(&stream, plain()), DAYLIGHT, DisplayGamut::Srgb);
    assert_eq!(
        applied_map(&viewfinder),
        applied_map(&still),
        "снимок проявляется с теми же усилениями, что видоискатель"
    );
}

#[test]
fn switching_camera_switches_the_map() {
    let main = calibrated_stream("2", 4.0);
    let wide = calibrated_stream("3", 6.0);
    let mut cache = IspParamsCache::default();
    cache.prepare(&frame(&main, plain()), DAYLIGHT, DisplayGamut::Srgb);
    let main_map = applied_map(&cache);
    cache.prepare(&frame(&wide, plain()), DAYLIGHT, DisplayGamut::Srgb);
    let wide_map = applied_map(&cache);
    assert_eq!(main_map.points()[0][0], 4.0);
    assert_eq!(wide_map.points()[0][0], 6.0);
}

#[test]
fn config_section_reaches_the_resolved_params() {
    let stream = calibrated_stream("2", 4.0);
    let weakened = crate::config::ViewfinderLensShadingConfig {
        strength: CorrectionStrength::new(0.25).unwrap(),
        ..Default::default()
    };
    let mut cache = IspParamsCache::default().with_lens_shading(weakened.control());
    cache.prepare(&frame(&stream, plain()), DAYLIGHT, DisplayGamut::Srgb);
    assert_eq!(applied_map(&cache).points()[0][0], 1.0 + (4.0 - 1.0) * 0.25);
}

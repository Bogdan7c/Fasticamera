//! Кэш параметров ISP: пересчёт только при смене входов, кадры без метаданных, честный отказ.

use std::sync::Arc;

use camera_contract::capabilities::{
    ColorCalibrationSet, LensShading, RawSupport, SensorLevels, VendorProcessingFlags,
};
use camera_contract::capture_result::{CaptureResultMeta, FrameNumber, SensorTimestamp};
use camera_contract::fixtures::{camera_id, main_wide_camera};
use camera_contract::frame_passport::{
    CaptureColorHints, ColorCorrectionGains, FrameCaptureDetails, FrameCaptureMeta, FrameSequence, MissingCaptureMeta,
    RawFramePassport, StreamPassport,
};
use camera_contract::geometry::{PixelRect, PixelSize, SensorOrientation};
use camera_contract::raw_format::{CfaLayout, MosaicKind, MosaicPattern, RawFormat};
use camera_contract::stream::{FrameRateMechanism, ReadoutAspect, TargetFrameRate};
use camera_contract::three_a::{RequestOrigin, ThreeAResultState};
use isp_model::color::chromaticity::Chromaticity;
use isp_model::demosaic::DemosaicMethod;
use isp_model::exposure::ExposureGain;
use isp_model::image::DisplayGamut;

use super::*;

fn calibration() -> Vec<ColorCalibrationSet> {
    match main_wide_camera().capabilities.raw {
        RawSupport::Supported(info) => info.color_calibration,
        RawSupport::NotSupported => panic!("в образце основной камеры есть RAW"),
    }
}

fn stream(color_calibration: Vec<ColorCalibrationSet>) -> Arc<StreamPassport> {
    Arc::new(StreamPassport {
        camera: camera_id("2"),
        raw_format: RawFormat::Raw16,
        buffer_size: PixelSize::new(64, 48),
        readout: ReadoutAspect::FourByThree,
        valid_area: PixelRect {
            left: 0,
            top: 0,
            size: PixelSize::new(64, 48),
        },
        mosaic: MosaicPattern {
            kind: MosaicKind::Bayer,
            layout: CfaLayout::Bggr,
        },
        static_levels: SensorLevels {
            black_level_pattern: [64; 4],
            white_level: 1023,
        },
        vendor_processing: VendorProcessingFlags {
            lens_shading: LensShading::AppliedByVendor,
        },
        sensor_orientation: SensorOrientation::Degrees90,
        frame_rate: TargetFrameRate::new(60).expect("60 к/с"),
        frame_rate_mechanism: FrameRateMechanism::HalDefault,
        color_calibration,
    })
}

/// Дневной свет и тёплая лампа (≈ 2856 К, точка A) — две заведомо разные белые.
const DAYLIGHT: Chromaticity = Chromaticity::D65;
const WARM_LAMP: Chromaticity = Chromaticity { x: 0.44757, y: 0.40745 };

fn frame(stream: &Arc<StreamPassport>, capture: FrameCaptureMeta) -> RawFramePassport {
    RawFramePassport {
        stream: Arc::clone(stream),
        sequence: FrameSequence(1),
        sensor_timestamp: SensorTimestamp::from_nanos(10),
        capture,
    }
}

fn paired(color: CaptureColorHints) -> FrameCaptureMeta {
    FrameCaptureMeta::Paired(FrameCaptureDetails {
        result: CaptureResultMeta {
            frame_number: FrameNumber(1),
            sensor_timestamp: Some(SensorTimestamp::from_nanos(10)),
            exposure_time: None,
            sensitivity_iso: None,
            reported_frame_duration: None,
            rolling_shutter_skew: None,
            three_a: ThreeAResultState::unreported(RequestOrigin::Repeating),
        },
        color,
    })
}

fn plain() -> FrameCaptureMeta {
    paired(CaptureColorHints::default())
}

fn missing() -> FrameCaptureMeta {
    FrameCaptureMeta::Missing {
        reason: MissingCaptureMeta::PairingTimedOut,
    }
}

fn this_frame(outcome: ParamsOutcome) -> ParamsDecision {
    ParamsDecision {
        hints: HintsSource::ThisFrame,
        outcome,
    }
}

#[test]
fn params_are_resolved_once_and_reused_while_the_white_stays_the_same() {
    let stream = stream(calibration());
    let mut cache = IspParamsCache::default();
    let frame = frame(&stream, plain());
    assert_eq!(
        cache.prepare(&frame, DAYLIGHT, DisplayGamut::Srgb),
        this_frame(ParamsOutcome::Resolved)
    );
    let first = cache.current().cloned().expect("параметры есть");
    assert_eq!(first.color.gamut(), DisplayGamut::Srgb);
    assert_eq!(
        cache.prepare(&frame, DAYLIGHT, DisplayGamut::Srgb),
        this_frame(ParamsOutcome::Unchanged),
        "та же белая — без пересчёта"
    );
    // Новый Arc с тем же содержимым — это новая сессия потока (лизинг создаёт `Arc` в
    // `begin_stream`): параметры прежней сессии ей не принадлежат, считаются заново.
    let equal_stream = Arc::new((*stream).clone());
    assert_eq!(
        cache.prepare(&self::frame(&equal_stream, plain()), DAYLIGHT, DisplayGamut::Srgb),
        this_frame(ParamsOutcome::Resolved)
    );
}

#[test]
fn exposure_gain_is_part_of_the_cache_key_and_unity_is_the_old_path_bit_for_bit() {
    let stream = stream(calibration());
    let frame = frame(&stream, plain());
    let mut cache = IspParamsCache::default();
    cache.prepare(&frame, DAYLIGHT, DisplayGamut::Srgb);
    let plain_params = cache.current().cloned().expect("параметры есть");
    // Единичная подтяжка — тот же ключ: без пересчёта и без единого бита разницы.
    let unity = cache.prepare_with_exposure_gain(&frame, DAYLIGHT, ExposureGain::UNITY, DisplayGamut::Srgb);
    assert_eq!(unity, this_frame(ParamsOutcome::Unchanged));
    assert_eq!(cache.current(), Some(&plain_params));
    // Подтяжка +0,5 EV: ключ другой — пересчёт, и тон действительно другой (свет сцены домножен на k).
    let half_stop = ExposureGain::from_ev(0.5).expect("подтяжка");
    let lifted = cache.prepare_with_exposure_gain(&frame, DAYLIGHT, half_stop, DisplayGamut::Srgb);
    assert_eq!(lifted, this_frame(ParamsOutcome::Resolved));
    let lifted_params = cache.current().cloned().expect("параметры есть");
    assert_ne!(lifted_params.tone, plain_params.tone);
    assert_eq!(lifted_params.color, plain_params.color, "цвет подтяжка не трогает");
    // Тот же k — кэш; возврат к 1 — снова пересчёт и точно прежние параметры.
    assert_eq!(
        cache.prepare_with_exposure_gain(&frame, DAYLIGHT, half_stop, DisplayGamut::Srgb),
        this_frame(ParamsOutcome::Unchanged)
    );
    assert_eq!(
        cache.prepare_with_exposure_gain(&frame, DAYLIGHT, ExposureGain::UNITY, DisplayGamut::Srgb),
        this_frame(ParamsOutcome::Resolved)
    );
    assert_eq!(cache.current(), Some(&plain_params));
    // Веса яркости для статистики AE — от параметров ИМЕННО этого кадра, подтяжка на них не влияет.
    assert_eq!(
        lifted_params.scene_luminance_weights(),
        plain_params.scene_luminance_weights()
    );
}

#[test]
fn base_scene_middle_gray_is_the_gray_the_resolved_tone_is_built_around() {
    let stream = stream(calibration());
    let mut cache = IspParamsCache::default();
    cache.prepare(&frame(&stream, plain()), DAYLIGHT, DisplayGamut::Srgb);
    let params = cache.current().expect("параметры есть");
    // Тон с подтяжкой 1 даёт на средний серый базовых настроек ровно экранный средний серый.
    let display = params.tone.map_channel(IspParamsCache::base_scene_middle_gray());
    let wanted = isp_model::tone::ToneCurveSettings::default().display_middle_gray;
    assert!((display - wanted).abs() < 1e-4, "{display} против {wanted}");
}

#[test]
fn white_or_gamut_change_triggers_a_fresh_resolve_with_different_params() {
    let stream = stream(calibration());
    let mut cache = IspParamsCache::default();
    cache.prepare(&frame(&stream, plain()), DAYLIGHT, DisplayGamut::Srgb);
    let daylight = cache.current().cloned().expect("параметры есть");

    assert_eq!(
        cache.prepare(&frame(&stream, plain()), WARM_LAMP, DisplayGamut::Srgb),
        this_frame(ParamsOutcome::Resolved)
    );
    let warm_params = cache.current().cloned().expect("параметры есть");
    assert_ne!(
        warm_params.white_balance, daylight.white_balance,
        "баланс следует белой точке"
    );
    // Тёплый свет: красный канал камеры сильнее, значит его усиление меньше, а синего — больше.
    let (warm, day) = (warm_params.white_balance.as_array(), daylight.white_balance.as_array());
    assert!(warm[0] < day[0] && warm[2] > day[2], "{warm:?} против {day:?}");

    assert_eq!(
        cache.prepare(&frame(&stream, plain()), WARM_LAMP, DisplayGamut::DisplayP3),
        this_frame(ParamsOutcome::Resolved)
    );
    assert_eq!(
        cache.current().map(|params| params.color.gamut()),
        Some(DisplayGamut::DisplayP3),
        "охват выхода следует экрану"
    );
}

/// Критерий выхода 08: HAL-подсказки цвета (коэффициенты, нейтральная точка) на параметры ISP не влияют.
#[test]
fn hal_colour_hints_never_change_the_params() {
    let stream = stream(calibration());
    let mut cache = IspParamsCache::default();
    cache.prepare(&frame(&stream, plain()), DAYLIGHT, DisplayGamut::Srgb);
    let baseline = cache.current().cloned().expect("параметры есть");
    let neutral = |numerator| camera_contract::capabilities::Rational {
        numerator,
        denominator: 1000,
    };
    let mutated = [
        CaptureColorHints {
            color_gains: Some(ColorCorrectionGains([2.0, 1.0, 1.0, 4.0 / 3.0])),
            ..CaptureColorHints::default()
        },
        CaptureColorHints {
            neutral_color_point: Some([neutral(300), neutral(1000), neutral(900)]),
            ..CaptureColorHints::default()
        },
        CaptureColorHints {
            color_gains: Some(ColorCorrectionGains([f32::NAN, 0.0, -1.0, 9.0])),
            neutral_color_point: Some([neutral(0), neutral(0), neutral(0)]),
            ..CaptureColorHints::default()
        },
    ];
    for hints in mutated {
        assert_eq!(
            cache.prepare(&frame(&stream, paired(hints)), DAYLIGHT, DisplayGamut::Srgb),
            this_frame(ParamsOutcome::Unchanged),
            "{hints:?}: те же входы — без пересчёта"
        );
        assert_eq!(cache.current(), Some(&baseline), "{hints:?}: параметры побитово те же");
    }
    // И при свежем пересчёте результат тот же, что без подсказок.
    let mut fresh = IspParamsCache::default();
    let wild = CaptureColorHints {
        color_gains: Some(ColorCorrectionGains([5.0, 1.0, 1.0, 0.2])),
        ..CaptureColorHints::default()
    };
    fresh.prepare(&frame(&stream, paired(wild)), DAYLIGHT, DisplayGamut::Srgb);
    assert_eq!(fresh.current(), Some(&baseline));
}

#[test]
fn frame_without_metadata_reuses_the_hints_of_an_earlier_frame() {
    let stream = stream(calibration());
    let mut cache = IspParamsCache::default();
    assert_eq!(
        cache.prepare(&frame(&stream, missing()), DAYLIGHT, DisplayGamut::Srgb),
        ParamsDecision {
            hints: HintsSource::EarlierFrame,
            outcome: ParamsOutcome::Unavailable(ParamsUnavailable::NoMetadataYet),
        },
        "первый кадр без метаданных: параметров нет, цвета не выдумываем"
    );
    assert!(cache.current().is_none());

    cache.prepare(&frame(&stream, plain()), DAYLIGHT, DisplayGamut::Srgb);
    assert_eq!(
        cache.prepare(&frame(&stream, missing()), DAYLIGHT, DisplayGamut::Srgb),
        ParamsDecision {
            hints: HintsSource::EarlierFrame,
            outcome: ParamsOutcome::Unchanged,
        }
    );
    // Экран сменил охват, а метаданных у кадра нет: пересчёт по подсказкам прежнего кадра.
    assert_eq!(
        cache.prepare(&frame(&stream, missing()), DAYLIGHT, DisplayGamut::DisplayP3),
        ParamsDecision {
            hints: HintsSource::EarlierFrame,
            outcome: ParamsOutcome::Resolved,
        }
    );
}

#[test]
fn failures_are_reported_and_never_replace_good_params() {
    let mut cache = IspParamsCache::default();
    let without_matrices = stream(Vec::new());
    assert!(
        matches!(
            cache
                .prepare(&frame(&without_matrices, plain()), DAYLIGHT, DisplayGamut::Srgb)
                .outcome,
            ParamsOutcome::Unavailable(ParamsUnavailable::Failed(ParamsFailure::Sensor(_)))
        ),
        "нет калибровки цвета — честная ошибка, а не единичная матрица"
    );

    let stream = stream(calibration());
    let not_a_white = Chromaticity { x: f64::NAN, y: 0.3 };
    assert!(
        matches!(
            cache
                .prepare(&frame(&stream, plain()), not_a_white, DisplayGamut::Srgb)
                .outcome,
            ParamsOutcome::Unavailable(ParamsUnavailable::Failed(ParamsFailure::InvalidNeutral(_)))
        ),
        "нечисловая белая без прежних параметров — параметров нет"
    );
    cache.prepare(&frame(&stream, plain()), DAYLIGHT, DisplayGamut::Srgb);
    let good = cache.current().cloned().expect("параметры есть");
    assert!(matches!(
        cache
            .prepare(&frame(&stream, plain()), not_a_white, DisplayGamut::Srgb)
            .outcome,
        ParamsOutcome::ReusedAfterFailure(ParamsFailure::InvalidNeutral(_))
    ));
    let zero_luminance = Chromaticity { x: 0.3, y: 0.0 };
    assert!(matches!(
        cache
            .prepare(&frame(&stream, plain()), zero_luminance, DisplayGamut::Srgb)
            .outcome,
        ParamsOutcome::ReusedAfterFailure(ParamsFailure::InvalidNeutral(_))
    ));
    assert_eq!(cache.current(), Some(&good), "прежние параметры остались");
}

#[test]
fn quality_cache_resolves_menon_params_from_the_same_inputs_as_the_viewfinder() {
    let stream = stream(calibration());
    let daylight = frame(&stream, plain());
    let mut viewfinder = IspParamsCache::default();
    let mut bench = IspParamsCache::for_demosaic(DemosaicMethod::Quality);
    assert_eq!(
        viewfinder.prepare(&daylight, DAYLIGHT, DisplayGamut::Srgb),
        this_frame(ParamsOutcome::Resolved)
    );
    assert_eq!(
        bench.prepare(&daylight, DAYLIGHT, DisplayGamut::Srgb),
        this_frame(ParamsOutcome::Resolved)
    );
    let preview = viewfinder.current().cloned().expect("параметры видоискателя");
    let quality = bench.current().cloned().expect("параметры стенда");
    assert_eq!(preview.demosaic, DemosaicMethod::Preview);
    assert_eq!(quality.demosaic, DemosaicMethod::Quality);
    assert_eq!(
        IspParams {
            demosaic: DemosaicMethod::Preview,
            ..quality
        },
        preview,
        "кроме демозаика параметры совпадают: те же входы"
    );
}

// ---- привязка к потоку: чужие подсказки и параметры не подставляются (решение 42) ----

/// Кадр без метаданных → «параметров нет» (кадр не показывается).
fn no_metadata_yet() -> ParamsDecision {
    ParamsDecision {
        hints: HintsSource::EarlierFrame,
        outcome: ParamsOutcome::Unavailable(ParamsUnavailable::NoMetadataYet),
    }
}

/// Поток A дал подсказки и параметры; первый кадр потока B без метаданных обязан остаться без
/// параметров для обоих демозаиков (видоискатель и стенд V4 используют один кэш).
#[test]
fn frame_without_metadata_of_a_new_stream_never_borrows_hints_of_the_previous_stream() {
    for demosaic in [DemosaicMethod::Preview, DemosaicMethod::Quality] {
        let stream_a = stream(calibration());
        // Та же камера и тот же режим, но другая сессия (другой `Arc`).
        let stream_b = Arc::new((*stream_a).clone());
        let mut cache = IspParamsCache::for_demosaic(demosaic);
        assert_eq!(
            cache.prepare(&frame(&stream_a, plain()), DAYLIGHT, DisplayGamut::Srgb),
            this_frame(ParamsOutcome::Resolved)
        );
        assert_eq!(
            cache.prepare(&frame(&stream_b, missing()), DAYLIGHT, DisplayGamut::Srgb),
            no_metadata_yet(),
            "{demosaic:?}: подсказки потока A кадру потока B чужие"
        );
        assert!(cache.current().is_none(), "{demosaic:?}: параметры потока A забыты");
        // Охват экрана сменился, метаданных всё ещё нет — тоже честное «нет», не «без изменений».
        assert_eq!(
            cache.prepare(&frame(&stream_b, missing()), DAYLIGHT, DisplayGamut::DisplayP3),
            no_metadata_yet()
        );
    }
}

/// Пересчёт для нового потока не удался: параметры прежнего потока (его CFA, уровни, матрицы)
/// на кадр нового не ставятся.
#[test]
fn resolve_failure_on_a_new_stream_never_reuses_params_of_the_previous_stream() {
    for demosaic in [DemosaicMethod::Preview, DemosaicMethod::Quality] {
        let stream_a = stream(calibration());
        let mut cache = IspParamsCache::for_demosaic(demosaic);
        cache.prepare(&frame(&stream_a, plain()), DAYLIGHT, DisplayGamut::Srgb);
        assert!(cache.current().is_some());

        let stream_b = Arc::new((*stream_a).clone());
        let not_a_white = Chromaticity { x: f64::NAN, y: 0.3 };
        assert!(
            matches!(
                cache
                    .prepare(&frame(&stream_b, plain()), not_a_white, DisplayGamut::Srgb)
                    .outcome,
                ParamsOutcome::Unavailable(ParamsUnavailable::Failed(ParamsFailure::InvalidNeutral(_)))
            ),
            "{demosaic:?}: отказ в новом потоке — без параметров, а не с параметрами потока A"
        );
        assert!(cache.current().is_none());
    }
}

/// `forget_stream` (поток неактивен) стирает подсказки даже для того же `Arc`.
#[test]
fn forgotten_stream_has_no_hints_and_no_params() {
    let stream = stream(calibration());
    let mut cache = IspParamsCache::default();
    cache.prepare(&frame(&stream, plain()), DAYLIGHT, DisplayGamut::Srgb);
    cache.forget_stream();
    assert!(cache.current().is_none());
    assert_eq!(
        cache.prepare(&frame(&stream, missing()), DAYLIGHT, DisplayGamut::Srgb),
        no_metadata_yet()
    );
    // Кадр с метаданными после этого считается заново.
    assert_eq!(
        cache.prepare(&frame(&stream, plain()), DAYLIGHT, DisplayGamut::Srgb),
        this_frame(ParamsOutcome::Resolved)
    );
}

/// Сменился охват экрана, а пересчёт не удался: параметры под прежний охват — неверные цвета,
/// их не используем.
#[test]
fn resolve_failure_after_a_gamut_change_does_not_reuse_params_of_the_other_gamut() {
    let stream = stream(calibration());
    let mut cache = IspParamsCache::default();
    cache.prepare(&frame(&stream, plain()), DAYLIGHT, DisplayGamut::Srgb);
    let not_a_white = Chromaticity { x: f64::NAN, y: 0.3 };
    assert!(matches!(
        cache
            .prepare(&frame(&stream, plain()), not_a_white, DisplayGamut::DisplayP3)
            .outcome,
        ParamsOutcome::Unavailable(ParamsUnavailable::Failed(ParamsFailure::InvalidNeutral(_)))
    ));
    assert!(matches!(
        cache
            .prepare(&frame(&stream, plain()), not_a_white, DisplayGamut::Srgb)
            .outcome,
        ParamsOutcome::ReusedAfterFailure(ParamsFailure::InvalidNeutral(_))
    ));
}

/// Ручки `viewfinder.tone` доходят до параметров кадра: контраст кривой — из конфига, а средний серый сцены
/// (цель AE) превращается на экране в середину ИЗ КОНФИГА. Цель AE при этом не сдвигается.
#[test]
fn tone_knobs_from_the_config_reach_the_resolved_params_without_moving_the_ae_target() {
    let tone = crate::config::ViewfinderToneConfig {
        contrast: 1.8,
        display_middle_gray: 0.24,
    };
    let stream = stream(calibration());
    let mut cache = IspParamsCache::default().with_tone(tone);
    cache.prepare(&frame(&stream, plain()), DAYLIGHT, DisplayGamut::Srgb);
    let params = cache.current().expect("параметры есть");
    assert_eq!(params.tone.parameters().contrast, 1.8);
    let display = params.tone.map_channel(IspParamsCache::base_scene_middle_gray());
    assert!((display - 0.24).abs() < 1e-4, "{display} против 0,24 из конфига");
    assert_eq!(
        IspParamsCache::base_scene_middle_gray(),
        isp_model::tone::ToneCurveSettings::default().scene_middle_gray,
        "цель AE не ручка тона"
    );
}

//! Кэш параметров ISP: пересчёт только при смене входов, кадры без метаданных, честный отказ.

use std::sync::Arc;

use camera_contract::capabilities::{
    ColorCalibrationSet, LensShading, Rational, RawSupport, SensorLevels, VendorProcessingFlags,
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
use isp_model::demosaic::DemosaicMethod;
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

fn rational(numerator: i32, denominator: i32) -> Rational {
    Rational { numerator, denominator }
}

fn hints(neutral: [Rational; 3]) -> CaptureColorHints {
    CaptureColorHints {
        neutral_color_point: Some(neutral),
        ..CaptureColorHints::default()
    }
}

const DAYLIGHT: [Rational; 3] = [
    Rational {
        numerator: 1,
        denominator: 2,
    },
    Rational {
        numerator: 1,
        denominator: 1,
    },
    Rational {
        numerator: 3,
        denominator: 4,
    },
];

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
        },
        color,
    })
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
fn params_are_resolved_once_and_reused_while_inputs_stay_the_same() {
    let stream = stream(calibration());
    let mut cache = IspParamsCache::default();
    let daylight = frame(&stream, paired(hints(DAYLIGHT)));
    assert_eq!(
        cache.prepare(&daylight, DisplayGamut::Srgb),
        this_frame(ParamsOutcome::Resolved)
    );
    let first = cache.current().cloned().expect("параметры есть");
    assert_eq!(first.color.gamut(), DisplayGamut::Srgb);
    assert_eq!(
        cache.prepare(&daylight, DisplayGamut::Srgb),
        this_frame(ParamsOutcome::Unchanged),
        "те же входы — без пересчёта"
    );
    // Новый Arc с тем же содержимым — это новая сессия потока (лизинг создаёт `Arc` в
    // `begin_stream`): параметры прежней сессии ей не принадлежат, считаются заново.
    let equal_stream = Arc::new((*stream).clone());
    assert_eq!(
        cache.prepare(&frame(&equal_stream, paired(hints(DAYLIGHT))), DisplayGamut::Srgb),
        this_frame(ParamsOutcome::Resolved)
    );
}

#[test]
fn neutral_or_gamut_change_triggers_a_fresh_resolve_with_different_params() {
    let stream = stream(calibration());
    let mut cache = IspParamsCache::default();
    cache.prepare(&frame(&stream, paired(hints(DAYLIGHT))), DisplayGamut::Srgb);
    let daylight = cache.current().cloned().expect("параметры есть");

    let warm = [rational(1, 1), rational(1, 1), rational(1, 3)];
    assert_eq!(
        cache.prepare(&frame(&stream, paired(hints(warm))), DisplayGamut::Srgb),
        this_frame(ParamsOutcome::Resolved)
    );
    let warm_params = cache.current().cloned().expect("параметры есть");
    assert_ne!(
        warm_params.white_balance, daylight.white_balance,
        "баланс следует нейтрали"
    );

    assert_eq!(
        cache.prepare(&frame(&stream, paired(hints(warm))), DisplayGamut::DisplayP3),
        this_frame(ParamsOutcome::Resolved)
    );
    assert_eq!(
        cache.current().map(|params| params.color.gamut()),
        Some(DisplayGamut::DisplayP3),
        "охват выхода следует экрану"
    );
}

#[test]
fn frame_without_metadata_reuses_the_hints_of_an_earlier_frame() {
    let stream = stream(calibration());
    let mut cache = IspParamsCache::default();
    assert_eq!(
        cache.prepare(&frame(&stream, missing()), DisplayGamut::Srgb),
        ParamsDecision {
            hints: HintsSource::EarlierFrame,
            outcome: ParamsOutcome::Unavailable(ParamsUnavailable::NoMetadataYet),
        },
        "первый кадр без метаданных: параметров нет, цвета не выдумываем"
    );
    assert!(cache.current().is_none());

    cache.prepare(&frame(&stream, paired(hints(DAYLIGHT))), DisplayGamut::Srgb);
    assert_eq!(
        cache.prepare(&frame(&stream, missing()), DisplayGamut::Srgb),
        ParamsDecision {
            hints: HintsSource::EarlierFrame,
            outcome: ParamsOutcome::Unchanged,
        }
    );
    // Экран сменил охват, а метаданных у кадра нет: пересчёт по подсказкам прежнего кадра.
    assert_eq!(
        cache.prepare(&frame(&stream, missing()), DisplayGamut::DisplayP3),
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
                .prepare(&frame(&without_matrices, paired(hints(DAYLIGHT))), DisplayGamut::Srgb)
                .outcome,
            ParamsOutcome::Unavailable(ParamsUnavailable::Failed(ParamsFailure::Sensor(_)))
        ),
        "нет калибровки цвета — честная ошибка, а не единичная матрица"
    );

    let stream = stream(calibration());
    assert_eq!(
        cache
            .prepare(
                &frame(&stream, paired(CaptureColorHints::default())),
                DisplayGamut::Srgb
            )
            .outcome,
        ParamsOutcome::Unavailable(ParamsUnavailable::Failed(ParamsFailure::NoNeutral))
    );
    cache.prepare(&frame(&stream, paired(hints(DAYLIGHT))), DisplayGamut::Srgb);
    let good = cache.current().cloned().expect("параметры есть");

    let broken = [rational(1, 0), rational(1, 1), rational(1, 1)];
    assert!(matches!(
        cache
            .prepare(&frame(&stream, paired(hints(broken))), DisplayGamut::Srgb)
            .outcome,
        ParamsOutcome::ReusedAfterFailure(ParamsFailure::InvalidNeutral(_))
    ));
    let negative = [rational(-1, 1), rational(1, 1), rational(1, 1)];
    assert!(matches!(
        cache
            .prepare(&frame(&stream, paired(hints(negative))), DisplayGamut::Srgb)
            .outcome,
        ParamsOutcome::ReusedAfterFailure(ParamsFailure::InvalidNeutral(_))
    ));
    assert_eq!(cache.current(), Some(&good), "прежние параметры остались");
}

#[test]
fn quality_cache_resolves_menon_params_from_the_same_inputs_as_the_viewfinder() {
    let stream = stream(calibration());
    let daylight = frame(&stream, paired(hints(DAYLIGHT)));
    let mut viewfinder = IspParamsCache::default();
    let mut bench = IspParamsCache::for_demosaic(DemosaicMethod::Quality);
    assert_eq!(
        viewfinder.prepare(&daylight, DisplayGamut::Srgb),
        this_frame(ParamsOutcome::Resolved)
    );
    assert_eq!(
        bench.prepare(&daylight, DisplayGamut::Srgb),
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
            cache.prepare(&frame(&stream_a, paired(hints(DAYLIGHT))), DisplayGamut::Srgb),
            this_frame(ParamsOutcome::Resolved)
        );
        assert_eq!(
            cache.prepare(&frame(&stream_b, missing()), DisplayGamut::Srgb),
            no_metadata_yet(),
            "{demosaic:?}: подсказки потока A кадру потока B чужие"
        );
        assert!(cache.current().is_none(), "{demosaic:?}: параметры потока A забыты");
        // Охват экрана сменился, метаданных всё ещё нет — тоже честное «нет», не «без изменений».
        assert_eq!(
            cache.prepare(&frame(&stream_b, missing()), DisplayGamut::DisplayP3),
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
        cache.prepare(&frame(&stream_a, paired(hints(DAYLIGHT))), DisplayGamut::Srgb);
        assert!(cache.current().is_some());

        let stream_b = Arc::new((*stream_a).clone());
        let broken = [rational(1, 0), rational(1, 1), rational(1, 1)];
        assert!(
            matches!(
                cache
                    .prepare(&frame(&stream_b, paired(hints(broken))), DisplayGamut::Srgb)
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
    cache.prepare(&frame(&stream, paired(hints(DAYLIGHT))), DisplayGamut::Srgb);
    cache.forget_stream();
    assert!(cache.current().is_none());
    assert_eq!(
        cache.prepare(&frame(&stream, missing()), DisplayGamut::Srgb),
        no_metadata_yet()
    );
    // Кадр с метаданными после этого считается заново.
    assert_eq!(
        cache.prepare(&frame(&stream, paired(hints(DAYLIGHT))), DisplayGamut::Srgb),
        this_frame(ParamsOutcome::Resolved)
    );
}

/// Сменился охват экрана, а пересчёт не удался: параметры под прежний охват — неверные цвета,
/// их не используем.
#[test]
fn resolve_failure_after_a_gamut_change_does_not_reuse_params_of_the_other_gamut() {
    let stream = stream(calibration());
    let mut cache = IspParamsCache::default();
    cache.prepare(&frame(&stream, paired(hints(DAYLIGHT))), DisplayGamut::Srgb);
    let broken = [rational(1, 0), rational(1, 1), rational(1, 1)];
    assert!(matches!(
        cache
            .prepare(&frame(&stream, paired(hints(broken))), DisplayGamut::DisplayP3)
            .outcome,
        ParamsOutcome::Unavailable(ParamsUnavailable::Failed(ParamsFailure::InvalidNeutral(_)))
    ));
    assert!(matches!(
        cache
            .prepare(&frame(&stream, paired(hints(broken))), DisplayGamut::Srgb)
            .outcome,
        ParamsOutcome::ReusedAfterFailure(ParamsFailure::InvalidNeutral(_))
    ));
}

/// Нейтральная точка `0/0` во всех каналах — ровно то, что HAL отдаёт в сессии из одного RAW-потока
/// (факт 07 на телефоне).
const ZERO_OVER_ZERO: [Rational; 3] = [
    Rational {
        numerator: 0,
        denominator: 0,
    },
    Rational {
        numerator: 0,
        denominator: 0,
    },
    Rational {
        numerator: 0,
        denominator: 0,
    },
];

#[test]
fn zero_neutral_point_falls_back_to_hal_gains_of_the_same_frame_with_the_same_colour() {
    let stream = stream(calibration());
    // Нейтраль DAYLIGHT = (1/2, 1, 3/4) ⇔ коэффициенты баланса (2, 1, 1, 4/3): серый после них одинаков.
    let by_point = {
        let mut cache = IspParamsCache::default();
        cache.prepare(&frame(&stream, paired(hints(DAYLIGHT))), DisplayGamut::Srgb);
        cache.current().cloned().expect("параметры по нейтральной точке")
    };
    let gains_only = CaptureColorHints {
        neutral_color_point: Some(ZERO_OVER_ZERO),
        color_gains: Some(ColorCorrectionGains([2.0, 1.0, 1.0, 4.0 / 3.0])),
        ..CaptureColorHints::default()
    };
    let mut cache = IspParamsCache::default();
    assert_eq!(
        cache.prepare(&frame(&stream, paired(gains_only)), DisplayGamut::Srgb),
        this_frame(ParamsOutcome::Resolved)
    );
    let by_gains = cache.current().cloned().expect("параметры по коэффициентам");
    // Тот же баланс белого и та же матрица цвета, что и по эквивалентной нейтральной точке.
    let white_balance_difference = by_point
        .white_balance
        .as_array()
        .iter()
        .zip(by_gains.white_balance.as_array())
        .map(|(point, gains)| (point - gains).abs())
        .fold(0.0_f32, f32::max);
    assert!(white_balance_difference < 1e-6, "{white_balance_difference}");
    assert_eq!(by_point.color.gamut(), by_gains.color.gamut());
}

#[test]
fn zero_neutral_point_without_usable_gains_is_an_honest_failure() {
    let stream = stream(calibration());
    for gains in [
        None,
        Some(ColorCorrectionGains([0.0, 1.0, 1.0, 1.5])),
        Some(ColorCorrectionGains([f32::NAN, 1.0, 1.0, 1.5])),
    ] {
        let zero_point = CaptureColorHints {
            neutral_color_point: Some(ZERO_OVER_ZERO),
            color_gains: gains,
            ..CaptureColorHints::default()
        };
        let mut cache = IspParamsCache::default();
        assert!(
            matches!(
                cache
                    .prepare(&frame(&stream, paired(zero_point)), DisplayGamut::Srgb)
                    .outcome,
                ParamsOutcome::Unavailable(ParamsUnavailable::Failed(ParamsFailure::InvalidNeutral(_)))
            ),
            "коэффициенты {gains:?} не годятся — кадр не показывается, цвет не выдумывается"
        );
        assert!(cache.current().is_none());
    }
    // Ни ключа, ни коэффициентов — «нейтрали нет», а не «некорректна».
    let mut cache = IspParamsCache::default();
    assert!(matches!(
        cache
            .prepare(
                &frame(&stream, paired(CaptureColorHints::default())),
                DisplayGamut::Srgb
            )
            .outcome,
        ParamsOutcome::Unavailable(ParamsUnavailable::Failed(ParamsFailure::NoNeutral))
    ));
}

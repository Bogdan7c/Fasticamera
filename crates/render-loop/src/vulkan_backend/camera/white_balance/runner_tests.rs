//! AWB потока рендера без GPU: холодный старт, ограниченное скрытие, смена потока, указания, прореживание.
//!
//! Статистика строится так же, как её выдал бы ISP (плитки сумм), поэтому проходит весь путь:
//! исполнитель оценки → контроллер → белая в плане кадра.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение — сигнал провала"
)]

use camera_contract::capabilities::{
    ColorCalibrationSet, LensShading, Rational, RawSupport, SensorLevels, VendorProcessingFlags,
};
use camera_contract::capture_result::{CaptureResultMeta, FrameNumber, SensorTimestamp};
use camera_contract::control::{ColorTemperature, Tint, WhiteBalanceSetting};
use camera_contract::fixtures::{camera_id, main_wide_camera};
use camera_contract::frame_passport::{
    CaptureColorHints, ColorCorrectionGains, FrameCaptureDetails, FrameCaptureMeta, FrameSequence, MissingCaptureMeta,
};
use camera_contract::geometry::{PixelRect, PixelSize, SensorOrientation};
use camera_contract::raw_format::{CfaLayout, MosaicKind, MosaicPattern, RawFormat};
use camera_contract::stream::{FrameRateMechanism, ReadoutAspect, TargetFrameRate};
use camera_contract::three_a::{
    RequestOrigin, ThreeAProfile, ThreeAResultState, WhiteBalanceDirective, WhiteBalanceMode,
};
use isp_model::color::temperature::Kelvin;
use isp_model::color::white_point::{CctDuv, Duv, cct_duv_from_chromaticity, chromaticity_from_cct_duv};
use isp_model::statistics::{AwbTile, TileGrid, quantize_sample};

use super::*;
use crate::viewfinder::ColdStartState;

const CELLS: u32 = 4096;
const FRAME_NANOS: i64 = 16_666_667;

fn calibration() -> Vec<ColorCalibrationSet> {
    match main_wide_camera().capabilities.raw {
        RawSupport::Supported(info) => info.color_calibration,
        RawSupport::NotSupported => panic!("в образце основной камеры есть RAW"),
    }
}

/// Новый поток (каждый вызов — новый `Arc`, то есть новая сессия камеры).
pub(super) fn new_stream() -> Arc<StreamPassport> {
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
        sensor_area: camera_contract::lens_shading::ValidAreaOnSensor::whole_active_array(PixelSize::new(64, 48)),
        lens_shading: camera_contract::lens_shading::LensShadingCalibrationState::NotCalibrated(
            camera_contract::lens_shading::NotCalibratedReason::NoCalibrationForCamera,
        ),
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
        color_calibration: calibration(),
    })
}

fn frame_at(stream: &Arc<StreamPassport>, index: i64, color: CaptureColorHints) -> RawFramePassport {
    RawFramePassport {
        stream: Arc::clone(stream),
        sequence: FrameSequence(u64::try_from(index).expect("номер кадра")),
        sensor_timestamp: SensorTimestamp::from_nanos(1_000_000_000 + index * FRAME_NANOS),
        capture: FrameCaptureMeta::Paired(FrameCaptureDetails {
            result: CaptureResultMeta {
                frame_number: FrameNumber(index),
                sensor_timestamp: Some(SensorTimestamp::from_nanos(1_000_000_000 + index * FRAME_NANOS)),
                exposure_time: None,
                sensitivity_iso: None,
                reported_frame_duration: None,
                rolling_shutter_skew: None,
                three_a: ThreeAResultState::unreported(RequestOrigin::Repeating),
            },
            color,
        }),
    }
}

pub(super) fn frame(stream: &Arc<StreamPassport>, index: i64) -> RawFramePassport {
    frame_at(stream, index, CaptureColorHints::default())
}

pub(super) fn identity_of(passport: &RawFramePassport) -> AwbFrameIdentity {
    AwbFrameIdentity {
        sequence: passport.sequence.0,
        sensor_timestamp_nanos: passport.sensor_timestamp.as_nanos(),
        frame_number: None,
    }
}

// ---- синтетическая статистика ----

fn grid() -> TileGrid {
    TileGrid { columns: 32, rows: 24 }
}

fn tile_of(rgb: [f32; 3]) -> AwbTile {
    let q = rgb.map(quantize_sample);
    AwbTile {
        sum_r: q[0] * CELLS,
        sum_g: 2 * q[1] * CELLS,
        sum_b: q[2] * CELLS,
        used_cells: CELLS,
        saturated_cells: 0,
    }
}

/// Серая поверхность под светом `kelvin` (нейтраль камеры из того же профиля, что у AWB).
fn gray_tile(kelvin: f64) -> AwbTile {
    let profile = DngColorProfile::from_calibration_sets(&calibration()).expect("профиль");
    let white = chromaticity_from_cct_duv(CctDuv {
        temperature: Kelvin(kelvin),
        duv: Duv(0.0),
    })
    .expect("белая");
    let neutral = profile.neutral_for_white(white).expect("нейтраль").values();
    tile_of([neutral[0] as f32 * 0.4, 0.4, neutral[2] as f32 * 0.4])
}

fn statistics_of(tile: AwbTile) -> AwbStatistics {
    AwbStatistics::from_tiles(grid(), vec![tile; 768]).expect("768 плиток")
}

pub(super) fn gray_scene(kelvin: f64) -> AwbStatistics {
    statistics_of(gray_tile(kelvin))
}

fn dark_scene() -> AwbStatistics {
    statistics_of(tile_of([0.002, 0.002, 0.002]))
}

fn clipped_scene() -> AwbStatistics {
    statistics_of(AwbTile {
        sum_r: 0,
        sum_g: 0,
        sum_b: 0,
        used_cells: 0,
        saturated_cells: CELLS,
    })
}

/// Однотонная зелень: далеко от кривой Планка, серых кандидатов нет.
pub(super) fn single_colour_scene() -> AwbStatistics {
    statistics_of(tile_of([0.05, 0.4, 0.05]))
}

/// Мало серых плиток среди зелени: оценка есть, но слабая.
fn weak_gray_scene(kelvin: f64) -> AwbStatistics {
    let mut tiles = vec![tile_of([0.05, 0.4, 0.05]); 768];
    for tile in tiles.iter_mut().take(8) {
        *tile = gray_tile(kelvin);
    }
    AwbStatistics::from_tiles(grid(), tiles).expect("768 плиток")
}

// ---- стенд ----

fn runner_with(
    config: WhiteBalanceConfig,
) -> (
    WhiteBalanceRunner,
    DirectiveSlot<WhiteBalanceDirective>,
    SceneWhiteMemory,
) {
    let directives = DirectiveSlot::new();
    let memory = SceneWhiteMemory::default();
    let runner = WhiteBalanceRunner::new(WhiteBalanceParts {
        config,
        directives: directives.clone(),
        memory: memory.clone(),
    })
    .expect("AWB собирается");
    (runner, directives, memory)
}

pub(super) fn runner() -> (
    WhiteBalanceRunner,
    DirectiveSlot<WhiteBalanceDirective>,
    SceneWhiteMemory,
) {
    runner_with(WhiteBalanceConfig::default())
}

pub(super) fn kelvin_of(white: Chromaticity) -> f64 {
    cct_duv_from_chromaticity(white)
        .expect("в таблице Робертсона")
        .temperature
        .0
}

/// Один «показ»: план кадра, затем (как делает сессия на следующем такте) статистика этого кадра.
pub(super) fn process(
    runner: &mut WhiteBalanceRunner,
    passport: &RawFramePassport,
    statistics: &AwbStatistics,
) -> FramePlan {
    let plan = runner.plan_frame(passport);
    if plan.statistics == StatisticsPlan::Compute {
        runner.ingest_statistics(plan.stream, identity_of(passport), statistics);
    }
    plan
}

#[test]
fn first_frames_are_hidden_until_the_first_estimate_then_shown_with_the_estimated_white() {
    let (mut runner, _, _) = runner();
    let stream = new_stream();
    let first = runner.plan_frame(&frame(&stream, 0));
    assert_eq!(
        first.presentation,
        IspPresentation::HideUntilColorKnown,
        "оценки ещё нет"
    );
    assert_eq!(
        first.statistics,
        StatisticsPlan::Compute,
        "на холодном старте статистика нужна"
    );
    assert_eq!(first.white, PROVISIONAL_WHITE, "пока оценки нет, белая — временная");
    // Статистика первого кадра дошла (fence слота): следующий кадр показывается уже с настоящей белой.
    runner.ingest_statistics(first.stream, identity_of(&frame(&stream, 0)), &gray_scene(2800.0));
    let second = runner.plan_frame(&frame(&stream, 1));
    assert_eq!(second.presentation, IspPresentation::Show);
    assert!(
        (kelvin_of(second.white) - 2800.0).abs() < 60.0,
        "{:?}",
        kelvin_of(second.white)
    );
    let diagnostics = runner.diagnostics();
    assert_eq!(diagnostics.hidden_frames_cold_start, 1);
    assert_eq!(diagnostics.cold_start_forced, 0);
    assert_eq!(diagnostics.cold_start, ColdStartState::Warm);
    assert_eq!(diagnostics.estimates, 1);
}

#[test]
fn dark_or_clipped_scenes_never_hide_the_picture_forever() {
    // Предел берётся из конфига, а не зашит числом: тест про то, что скрытие ограничено любым лимитом.
    let limit = WhiteBalanceConfig::default().max_hidden_frames;
    let total_frames = 2 * limit;
    for (name, scene) in [
        ("тёмная", dark_scene()),
        ("пересвеченная", clipped_scene()),
        ("однотонная", single_colour_scene()),
    ] {
        let (mut runner, _, _) = runner();
        let stream = new_stream();
        let mut presentations = Vec::new();
        for index in 0..i64::from(total_frames) {
            presentations.push(process(&mut runner, &frame(&stream, index), &scene).presentation);
        }
        let hidden = presentations
            .iter()
            .filter(|p| **p == IspPresentation::HideUntilColorKnown)
            .count();
        assert_eq!(hidden, limit as usize, "{name}: скрыто ровно max_hidden_frames");
        assert!(
            presentations[limit as usize..]
                .iter()
                .all(|p| *p == IspPresentation::Show),
            "{name}: после лимита показ идёт всегда"
        );
        let diagnostics = runner.diagnostics();
        assert_eq!(
            (diagnostics.hidden_frames_cold_start, diagnostics.cold_start_forced),
            (u64::from(limit), 1),
            "{name}"
        );
        assert_eq!(
            diagnostics.scene_white, None,
            "{name}: оценки не было — белая временная"
        );
        assert_eq!(
            diagnostics.frames_with_provisional_white,
            u64::from(total_frames),
            "{name}"
        );
    }
}

#[test]
fn forced_presentation_uses_the_best_available_weak_estimate_instead_of_the_provisional_white() {
    let (mut runner, _, _) = runner();
    let stream = new_stream();
    let scene = weak_gray_scene(3200.0);
    let mut last = None;
    // Лимит скрытия — из конфига: кадров ровно столько, чтобы последний был первым принудительно показанным.
    for index in 0..=i64::from(WhiteBalanceConfig::default().max_hidden_frames) {
        last = Some(process(&mut runner, &frame(&stream, index), &scene));
    }
    let forced = last.expect("кадры были");
    assert_eq!(forced.presentation, IspPresentation::Show);
    assert!(
        (kelvin_of(forced.white) - 3200.0).abs() < 80.0,
        "слабая оценка лучше выдумки: {}",
        kelvin_of(forced.white)
    );
    let diagnostics = runner.diagnostics();
    assert_eq!(diagnostics.cold_start_forced, 1);
    assert_eq!(
        diagnostics.decisions.accepted, 0,
        "порог уверенности не нарушен: оценку взяли как запасную"
    );
    assert!(
        diagnostics
            .last_estimate
            .is_some_and(|estimate| estimate.confidence < 0.17)
    );
}

#[test]
fn single_colour_scene_after_a_good_estimate_holds_the_white() {
    let (mut runner, _, _) = runner();
    let stream = new_stream();
    for index in 0..4 {
        process(&mut runner, &frame(&stream, index), &gray_scene(4000.0));
    }
    let settled = runner.plan_frame(&frame(&stream, 4)).white;
    // 40 кадров однотонной зелени: уверенность нулевая, белая не уходит.
    let mut plan = None;
    for index in 5..45 {
        plan = Some(process(&mut runner, &frame(&stream, index), &single_colour_scene()));
    }
    assert_eq!(plan.expect("кадры были").white, settled, "белая держится");
    let diagnostics = runner.diagnostics();
    let held = diagnostics.decisions.held_low_confidence + diagnostics.decisions.held_no_candidates;
    assert!(held >= 5, "однотонные оценки отвергнуты: {diagnostics:?}");
    assert_eq!(
        diagnostics.decisions.accepted, 1,
        "принята только оценка по серой сцене"
    );
    assert_eq!(
        diagnostics.hidden_frames_cold_start, 1,
        "скрыт только первый кадр до первой оценки"
    );
}

#[test]
fn a_new_stream_starts_from_the_last_scene_white_and_shows_immediately() {
    let (mut runner, _, memory) = runner();
    let first_camera = new_stream();
    for index in 0..4 {
        process(&mut runner, &frame(&first_camera, index), &gray_scene(2800.0));
    }
    assert!(memory.load().is_some(), "белая сцены сохранена для следующей камеры");
    // Смена камеры: поток становится неактивным, потом приходит новый.
    runner.forget_stream();
    let second_camera = new_stream();
    let plan = runner.plan_frame(&frame(&second_camera, 100));
    assert_eq!(
        plan.presentation,
        IspPresentation::Show,
        "белая сцены известна — прятать нечего"
    );
    assert!(
        (kelvin_of(plan.white) - 2800.0).abs() < 60.0,
        "старт с прежней белой, а не с D65"
    );
    assert_eq!(
        plan.statistics,
        StatisticsPlan::Compute,
        "первый кадр нового потока даёт статистику"
    );
    assert_eq!(runner.diagnostics().cold_start, ColdStartState::Warm);
}

#[test]
fn scene_white_survives_a_new_session_through_the_backend_memory() {
    let (mut runner, directives, memory) = runner();
    let stream = new_stream();
    for index in 0..4 {
        process(&mut runner, &frame(&stream, index), &gray_scene(5000.0));
    }
    drop(runner);
    // Новая сессия показа (surface пересоздана): тот же ящик указаний и та же память.
    let reborn = WhiteBalanceRunner::new(WhiteBalanceParts {
        config: WhiteBalanceConfig::default(),
        directives,
        memory,
    });
    let mut reborn = reborn.expect("AWB собирается");
    let plan = reborn.plan_frame(&frame(&new_stream(), 0));
    assert_eq!(plan.presentation, IspPresentation::Show);
    assert!((kelvin_of(plan.white) - 5000.0).abs() < 80.0);
}

#[test]
fn statistics_of_a_previous_stream_are_dropped_not_fed_to_the_new_cameras_table() {
    let (mut runner, _, _) = runner();
    let old = new_stream();
    let old_plan = runner.plan_frame(&frame(&old, 0));
    runner.forget_stream();
    let new = new_stream();
    let new_plan = runner.plan_frame(&frame(&new, 0));
    assert_ne!(old_plan.stream, new_plan.stream);
    // Запоздавшая статистика слота старой камеры.
    runner.ingest_statistics(old_plan.stream, identity_of(&frame(&old, 0)), &gray_scene(2800.0));
    let diagnostics = runner.diagnostics();
    assert_eq!(diagnostics.statistics_stale, 1);
    assert_eq!(diagnostics.estimates, 0, "оценщик чужую статистику не видел");
    assert_eq!(diagnostics.scene_white, None);
}

#[test]
fn statistics_are_decimated_after_the_first_estimate_but_not_before_it() {
    let (mut runner, _, _) = runner();
    let stream = new_stream();
    let mut plans = Vec::new();
    for index in 0..13 {
        plans.push(process(&mut runner, &frame(&stream, index), &gray_scene(4000.0)).statistics);
    }
    assert_eq!(plans[0], StatisticsPlan::Compute, "первый кадр потока");
    // Оценка из первого кадра приходит сразу (CPU-исполнитель); дальше — каждый 4-й кадр.
    let computed: Vec<usize> = plans
        .iter()
        .enumerate()
        .filter(|(_, p)| **p == StatisticsPlan::Compute)
        .map(|(i, _)| i)
        .collect();
    assert_eq!(computed, vec![0, 4, 8, 12]);
    let diagnostics = runner.diagnostics();
    assert_eq!(diagnostics.statistics_requested, 4);
    assert_eq!(diagnostics.statistics_skipped_by_decimation, 9);
    assert_eq!(diagnostics.statistics_every_n_frames, 4);
}

#[test]
fn manual_directive_sets_the_white_at_once_on_a_fresh_stream_without_hiding() {
    let (mut runner, directives, _) = runner();
    let setting = WhiteBalanceSetting {
        temperature: ColorTemperature::from_kelvin(3300.0).expect("температура"),
        tint: Tint::NEUTRAL,
    };
    let generation = directives.publish(WhiteBalanceDirective {
        mode: WhiteBalanceMode::Manual { setting },
        profile: ThreeAProfile::Photo,
    });
    let plan = runner.plan_frame(&frame(&new_stream(), 0));
    assert_eq!(
        plan.presentation,
        IspPresentation::Show,
        "ручная точка — это уже известная белая"
    );
    assert!(
        (kelvin_of(plan.white) - 3300.0).abs() < 5.0,
        "{}",
        kelvin_of(plan.white)
    );
    let diagnostics = runner.diagnostics();
    assert_eq!(diagnostics.directive_generation, Some(generation.0));
    assert_eq!(diagnostics.hidden_frames_cold_start, 0);
}

#[test]
fn manual_point_outside_the_cameras_table_is_limited_and_reported() {
    let (mut runner, directives, _) = runner();
    directives.publish(WhiteBalanceDirective {
        mode: WhiteBalanceMode::Manual {
            setting: WhiteBalanceSetting {
                temperature: ColorTemperature::from_kelvin(1500.0).expect("температура"),
                tint: Tint::NEUTRAL,
            },
        },
        profile: ThreeAProfile::Video,
    });
    let plan = runner.plan_frame(&frame(&new_stream(), 0));
    assert!(
        (kelvin_of(plan.white) - 2000.0).abs() < 5.0,
        "применена граница таблицы"
    );
    let limit = runner.diagnostics().manual_limit.expect("отчёт «ограничено до»");
    assert_eq!((limit.requested_kelvin, limit.applied_kelvin), (1500.0, 2000.0));
}

#[test]
fn locked_directive_freezes_the_white_and_a_newer_generation_replaces_the_older() {
    let (mut runner, directives, _) = runner();
    let stream = new_stream();
    for index in 0..4 {
        process(&mut runner, &frame(&stream, index), &gray_scene(6500.0));
    }
    let before = runner.plan_frame(&frame(&stream, 4)).white;
    directives.publish(WhiteBalanceDirective {
        mode: WhiteBalanceMode::Locked,
        profile: ThreeAProfile::Photo,
    });
    let mut after = before;
    for index in 5..60 {
        after = process(&mut runner, &frame(&stream, index), &gray_scene(2800.0)).white;
    }
    assert_eq!(after, before, "блокировка держит белую, пока свет меняется");
    assert!(runner.diagnostics().decisions.ignored_locked >= 1);
    // Новое указание «авто» (поколение 2) снимает блокировку.
    let second = directives.publish(WhiteBalanceDirective::DEFAULT);
    for index in 60..200 {
        after = process(&mut runner, &frame(&stream, index), &gray_scene(2800.0)).white;
    }
    assert!((kelvin_of(after) - 2800.0).abs() < 100.0, "{}", kelvin_of(after));
    assert_eq!(runner.diagnostics().directive_generation, Some(second.0));
}

#[test]
fn hal_colour_hints_never_change_the_white_but_are_compared_in_diagnostics() {
    let hint_sets = [
        CaptureColorHints::default(),
        CaptureColorHints {
            color_gains: Some(ColorCorrectionGains([2.0, 1.0, 1.0, 1.3])),
            ..CaptureColorHints::default()
        },
        CaptureColorHints {
            neutral_color_point: Some([
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
            ]),
            color_gains: Some(ColorCorrectionGains([0.3, 1.0, 1.0, 5.0])),
            ..CaptureColorHints::default()
        },
    ];
    let mut whites = Vec::new();
    let mut diagnostics = Vec::new();
    for hints in hint_sets {
        let (mut runner, _, _) = runner();
        let stream = new_stream();
        let mut plan = None;
        for index in 0..8 {
            plan = Some(process(
                &mut runner,
                &frame_at(&stream, index, hints),
                &gray_scene(4000.0),
            ));
        }
        whites.push(plan.expect("кадры были").white);
        diagnostics.push(runner.diagnostics());
    }
    assert_eq!(whites[0], whites[1], "подсказки HAL не меняют белую побитово");
    assert_eq!(whites[0], whites[2]);
    assert_eq!(diagnostics[0].hal_white, None, "нет подсказок — нет белой HAL");
    let hal = diagnostics[1].hal_white.expect("коэффициенты дали белую HAL");
    let delta = diagnostics[1].hal_delta.expect("разница с нашей");
    let ours = diagnostics[1].scene_white.expect("наша белая");
    assert!((ours.temperature_kelvin - 4000.0).abs() < 100.0);
    assert!(
        (hal.temperature_kelvin - ours.temperature_kelvin).abs() > 300.0,
        "белая HAL другая: {hal:?}"
    );
    assert!(delta.mired.abs() > 10.0, "разница считается: {delta:?}");
}

#[test]
fn frames_without_metadata_still_get_the_scene_white() {
    let (mut runner, _, _) = runner();
    let stream = new_stream();
    for index in 0..4 {
        process(&mut runner, &frame(&stream, index), &gray_scene(4000.0));
    }
    let mut passport = frame(&stream, 5);
    passport.capture = FrameCaptureMeta::Missing {
        reason: MissingCaptureMeta::PairingTimedOut,
    };
    let plan = runner.plan_frame(&passport);
    assert_eq!(plan.presentation, IspPresentation::Show);
    assert!((kelvin_of(plan.white) - 4000.0).abs() < 60.0);
}

#[test]
fn a_stream_without_colour_calibration_shows_with_the_provisional_white_and_reports_why() {
    let (mut runner, _, _) = runner();
    let mut passport = (*new_stream()).clone();
    passport.color_calibration = Vec::new();
    let stream = Arc::new(passport);
    let plan = runner.plan_frame(&frame(&stream, 0));
    assert_eq!(
        plan.presentation,
        IspPresentation::Show,
        "ждать нечего: оценок не будет"
    );
    assert_eq!(
        plan.statistics,
        StatisticsPlan::Skip,
        "статистика без таблицы бесполезна"
    );
    assert_eq!(plan.white, PROVISIONAL_WHITE);
    assert!(runner.diagnostics().table_failure.is_some());
}

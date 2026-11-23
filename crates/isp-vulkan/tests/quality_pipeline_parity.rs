//! Сквозной паритет стенда V4 с эталоном `isp-model` (`run_reference`, `DemosaicMethod::Quality`):
//! RAW-кадр (RAW16-картинка и упакованный RAW10, реальные кропы OnePlus 11 и синтетика) → уровни →
//! баланс → Menon 2007 → цвет → охват → тон → OETF → приёмник кадра. Сверяются две точки: линейный RGB
//! камеры после демозаика (f32, плоскости) и итоговый сигнал экрана (цель приёмника, fp16).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть способ сообщить о провале"
)]

#[path = "../../isp-model/tests/support/mod.rs"]
mod isp_support;
mod lens_shading_support;
mod quality_support;

use camera_contract::geometry::PixelSize;
use gpu_vulkan::kit_test_support::run_one_shot_blocking;
use gpu_vulkan::raw_mosaic_preview::reference::pack_raw10;
use gpu_vulkan::raw_mosaic_preview::synthetic::{SyntheticRaw10, SyntheticRaw16};
use isp_model::cfa::BayerPattern;
use isp_model::color::dng::CameraNeutral;
use isp_model::demosaic::DemosaicMethod;
use isp_model::image::{DisplayGamut, RawMosaic};
use isp_model::params::{FrameInputs, IspParams, ProcessingControls};
use isp_model::pipeline::{ReferenceRun, run_reference};
use isp_model::sensor::SensorDescription;
use isp_model::test_patterns::zone_plate;
use isp_support::real_capture::load_real_capture;
use isp_vulkan::quality::{
    QualityError, StreamGeneration, VideoFrameInput, VideoHeadroomConfig, VideoHeadroomWorkload,
};
use lens_shading_support::{asymmetric_map, skipped, with_map};
use quality_support::{
    area_of, assert_no_validation_errors, extent, gpu_context_or_skip, max_rgb_difference, padded_frame, run_gpu, sink,
    workload_config,
};

/// Допуск линейного RGB камеры (уровни + баланс + демозаик, f32): НОЛЬ — бит в бит. Деление в уровнях
/// идёт через `divide_correctly_rounded` (без него OpFDiv ошибался на 1 ULP у части отсчётов, и это
/// переворачивало решение H/V демозаика: 1e-3 на зонной пластинке). Остальное — как в
/// `quality_demosaic_parity.rs`.
const LINEAR_TOLERANCE: f32 = 0.0;

/// Допуск сигнала экрана (0…1). Цель приёмника в тесте — fp16: полшага fp16 у 1,0 = 4,9e-4 — это и
/// есть измеренный максимум на RADV (4,88e-4). Сверх квантования — `pow` в охвате, тоне и OETF: в Vulkan
/// его точность наследуется от `exp2(y·log2 x)` (несколько ULP f32, ниже 1e-5 на выходе). Допуск 1e-3 —
/// четверть шага 8-битного выхода: ловит ошибку любой формулы (неверная матрица или кривая дают ≥ 1e-2).
const DISPLAY_TOLERANCE: f32 = 1e-3;

fn assert_parity(name: &str, run: &ReferenceRun, linear: &[[f32; 3]], display: &[[f32; 3]]) {
    let width = run.camera_rgb.size().width;
    let (linear_difference, linear_at) = max_rgb_difference(linear, run.camera_rgb.pixels(), width);
    let (display_difference, display_at) = max_rgb_difference(display, run.display_encoded.pixels(), width);
    println!(
        "{name}: линейный RGB max |Δ| {linear_difference:e} в {linear_at:?}; сигнал экрана max |Δ| \
         {display_difference:e} в {display_at:?}"
    );
    assert!(
        linear_difference <= LINEAR_TOLERANCE,
        "{name}: линейный {linear_difference:e} в {linear_at:?}"
    );
    assert!(
        display_difference <= DISPLAY_TOLERANCE,
        "{name}: экран {display_difference:e} в {display_at:?}"
    );
    // Картинка не вырождена: эталон не чёрный и не плоский, иначе совпадение ничего бы не доказало.
    let encoded = run.display_encoded.pixels();
    let brightest = encoded.iter().flatten().copied().fold(0.0_f32, f32::max);
    let darkest = encoded.iter().flatten().copied().fold(1.0_f32, f32::min);
    assert!(
        brightest - darkest > 0.2,
        "{name}: эталон слишком плоский ({darkest}..{brightest})"
    );
}

fn params_for(sensor: &SensorDescription, neutral: CameraNeutral) -> IspParams {
    let frame = FrameInputs {
        levels: sensor.static_levels(),
    };
    let controls =
        ProcessingControls::with_defaults(neutral, DemosaicMethod::Quality, DisplayGamut::Srgb).with_legacy_tone();
    IspParams::resolve(sensor, &frame, &controls).unwrap()
}

#[test]
fn real_crops_match_reference_through_raw16_and_raw10_sources() {
    let Some(context) = gpu_context_or_skip() else { return };
    for name in [
        "real-main-lamp-jar",
        "real-main-lamp-towel",
        "real-main-lamp-tap-highlight",
    ] {
        let capture = load_real_capture(&isp_support::assets_dir(), name);
        let params = params_for(&capture.sensor, capture.neutral);
        let reference = run_reference(&capture.raw, &params).unwrap();
        // Мусор в отступе (белый уровень): если шейдер прочтёт за полезной областью, паритет сломается.
        let (frame, frame_width, frame_height) = padded_frame(&capture.raw, 1023);

        let raw16 = SyntheticRaw16::new(&context, frame_width, frame_height, &frame).unwrap();
        let (linear, display) = run_gpu(&context, &raw16.source(), &params, area_of(&capture.raw));
        assert_parity(&format!("{name}/raw16"), &reference, &linear, &display);

        // RAW10: ширина кадра кратна 4 (52), шаг строки с паддингом 0xA5.
        let row_stride_bytes = frame_width / 4 * 5 + 6;
        let packed = pack_raw10(&frame, frame_width, row_stride_bytes, 0xA5);
        let raw10 = SyntheticRaw10::new(&context, frame_width, frame_height, row_stride_bytes, &packed).unwrap();
        let (linear, display) = run_gpu(&context, &raw10.source(), &params, area_of(&capture.raw));
        assert_parity(&format!("{name}/raw10"), &reference, &linear, &display);
    }
    assert_no_validation_errors(&context);
}

#[test]
fn synthetic_zone_plate_matches_reference_for_every_cfa_layout() {
    let Some(context) = gpu_context_or_skip() else { return };
    let calibration = load_real_capture(&isp_support::assets_dir(), "real-main-lamp-gray-tile");
    let scene = zone_plate(PixelSize::new(64, 48), 0.5).unwrap();
    for pattern in [
        BayerPattern::RGGB,
        BayerPattern::GRBG,
        BayerPattern::GBRG,
        BayerPattern::BGGR,
    ] {
        let sensor = SensorDescription::new(
            pattern,
            calibration.sensor.static_levels(),
            *calibration.sensor.color_profile(),
            calibration.sensor.vendor_lens_shading(),
        );
        let params = params_for(&sensor, calibration.neutral);
        let levels = sensor.static_levels();
        // RAW = black + свет · нейтраль · (white − black): серая пластинка после баланса остаётся серой.
        let neutral = calibration.neutral.values();
        let samples: Vec<u16> = (0..scene.size().height)
            .flat_map(|y| (0..scene.size().width).map(move |x| (x, y)))
            .map(|(x, y)| {
                let channel = pattern.channel_at(x, y).index();
                let light = f64::from(scene.pixel(x, y)[channel]) * 0.6 * neutral[channel];
                let black = f64::from(levels.black_pattern()[(y as usize % 2) * 2 + x as usize % 2]);
                (black + light * (f64::from(levels.white()) - black)).round() as u16
            })
            .collect();
        let raw = RawMosaic::new(scene.size(), pattern, samples).unwrap();
        let reference = run_reference(&raw, &params).unwrap();
        let (frame, frame_width, frame_height) = padded_frame(&raw, 0);
        let raw16 = SyntheticRaw16::new(&context, frame_width, frame_height, &frame).unwrap();
        let (linear, display) = run_gpu(&context, &raw16.source(), &params, area_of(&raw));
        assert_parity(&format!("zone_plate/{pattern:?}"), &reference, &linear, &display);
    }
    assert_no_validation_errors(&context);
}

#[test]
fn shaded_real_crops_match_reference_bit_for_bit_through_raw16_and_raw10_in_an_offset_valid_area() {
    let Some(context) = gpu_context_or_skip() else { return };
    for name in ["real-main-lamp-jar", "real-main-lamp-towel"] {
        let capture = load_real_capture(&isp_support::assets_dir(), name);
        let plain = params_for(&capture.sensor, capture.neutral);
        let params = with_map(&plain, &asymmetric_map(5, 4));
        let reference = run_reference(&capture.raw, &params).unwrap();
        // Карта обязана изменить эталон, иначе совпадение с ним ничего не доказывает.
        let unshaded = run_reference(&capture.raw, &plain).unwrap();
        let (effect, _) = max_rgb_difference(reference.camera_rgb.pixels(), unshaded.camera_rgb.pixels(), 48);
        assert!(effect > 0.02, "{name}: карта почти не меняет эталон ({effect})");
        // Полезная область со смещением (2, 2) внутри кадра с белым мусором: координаты карты — от неё.
        let (frame, frame_width, frame_height) = padded_frame(&capture.raw, 1023);

        let raw16 = SyntheticRaw16::new(&context, frame_width, frame_height, &frame).unwrap();
        let (linear, display) = run_gpu(&context, &raw16.source(), &params, area_of(&capture.raw));
        assert_parity(&format!("{name}/shaded/raw16"), &reference, &linear, &display);

        let row_stride_bytes = frame_width / 4 * 5 + 6;
        let packed = pack_raw10(&frame, frame_width, row_stride_bytes, 0xA5);
        let raw10 = SyntheticRaw10::new(&context, frame_width, frame_height, row_stride_bytes, &packed).unwrap();
        let (linear, display) = run_gpu(&context, &raw10.source(), &params, area_of(&capture.raw));
        assert_parity(&format!("{name}/shaded/raw10"), &reference, &linear, &display);
    }
    assert_no_validation_errors(&context);
}

#[test]
fn shaded_zone_plate_matches_reference_bit_for_bit_for_every_cfa_layout() {
    let Some(context) = gpu_context_or_skip() else { return };
    let calibration = load_real_capture(&isp_support::assets_dir(), "real-main-lamp-gray-tile");
    let scene = zone_plate(PixelSize::new(64, 48), 0.5).unwrap();
    // Карта 7×6 — не квадратная и не равна размеру кадра: узлы не совпадают с пикселями.
    let map = asymmetric_map(7, 6);
    for pattern in [
        BayerPattern::RGGB,
        BayerPattern::GRBG,
        BayerPattern::GBRG,
        BayerPattern::BGGR,
    ] {
        let sensor = SensorDescription::new(
            pattern,
            calibration.sensor.static_levels(),
            *calibration.sensor.color_profile(),
            calibration.sensor.vendor_lens_shading(),
        );
        let params = with_map(&params_for(&sensor, calibration.neutral), &map);
        let levels = sensor.static_levels();
        let neutral = calibration.neutral.values();
        let samples: Vec<u16> = (0..scene.size().height)
            .flat_map(|y| (0..scene.size().width).map(move |x| (x, y)))
            .map(|(x, y)| {
                let channel = pattern.channel_at(x, y).index();
                let light = f64::from(scene.pixel(x, y)[channel]) * 0.4 * neutral[channel];
                let black = f64::from(levels.black_pattern()[(y as usize % 2) * 2 + x as usize % 2]);
                (black + light * (f64::from(levels.white()) - black)).round() as u16
            })
            .collect();
        let raw = RawMosaic::new(scene.size(), pattern, samples).unwrap();
        let reference = run_reference(&raw, &params).unwrap();
        let (frame, frame_width, frame_height) = padded_frame(&raw, 0);
        let raw16 = SyntheticRaw16::new(&context, frame_width, frame_height, &frame).unwrap();
        let (linear, display) = run_gpu(&context, &raw16.source(), &params, area_of(&raw));
        assert_parity(&format!("zone_plate/shaded/{pattern:?}"), &reference, &linear, &display);
    }
    assert_no_validation_errors(&context);
}

#[test]
fn skipped_lens_shading_equals_identity_map_bit_for_bit() {
    let Some(context) = gpu_context_or_skip() else { return };
    let capture = load_real_capture(&isp_support::assets_dir(), "real-main-lamp-jar");
    let plain = skipped(&params_for(&capture.sensor, capture.neutral));
    let identity = with_map(&plain, &lens_shading_support::identity_map(3, 3));
    let (frame, frame_width, frame_height) = padded_frame(&capture.raw, 1023);
    let raw16 = SyntheticRaw16::new(&context, frame_width, frame_height, &frame).unwrap();
    let area = area_of(&capture.raw);
    let (linear_skipped, display_skipped) = run_gpu(&context, &raw16.source(), &plain, area);
    let (linear_identity, display_identity) = run_gpu(&context, &raw16.source(), &identity, area);
    assert_eq!(linear_skipped, linear_identity, "линейный RGB");
    assert_eq!(display_skipped, display_identity, "сигнал экрана");
    assert_no_validation_errors(&context);
}

#[test]
fn map_larger_than_the_buffer_is_an_explicit_error_before_any_command() {
    let Some(context) = gpu_context_or_skip() else { return };
    let capture = load_real_capture(&isp_support::assets_dir(), "real-main-lamp-jar");
    let params = with_map(&params_for(&capture.sensor, capture.neutral), &asymmetric_map(5, 5));
    let (frame, frame_width, frame_height) = padded_frame(&capture.raw, 0);
    let raw16 = SyntheticRaw16::new(&context, frame_width, frame_height, &frame).unwrap();
    let config = VideoHeadroomConfig {
        lens_shading_map: isp_vulkan::lens_shading_map::LensShadingMapLimits {
            max_columns: 4,
            max_rows: 4,
        },
        ..workload_config()
    };
    let size = extent(48, 48);
    let mut workload = VideoHeadroomWorkload::new(&context, size, &config).unwrap();
    let mut frame_sink = sink(&context, size.width, size.height);
    let source = raw16.source();
    let input = VideoFrameInput {
        source: &source,
        params: &params,
        valid_area: area_of(&capture.raw),
        stream: StreamGeneration(1),
        sequence: 1,
        presentation_time_ns: 0,
    };
    let mut outcome = None;
    run_one_shot_blocking(&context, |command_buffer| {
        outcome = Some(workload.record(command_buffer, 0, &input, &mut frame_sink));
        Ok(())
    })
    .unwrap();
    assert!(
        matches!(
            outcome,
            Some(Err(QualityError::LensShadingMap(
                isp_vulkan::lens_shading_map::LensShadingMapError::TooLarge {
                    columns: 5,
                    rows: 5,
                    max_columns: 4,
                    max_rows: 4
                }
            )))
        ),
        "{outcome:?}"
    );
    assert_no_validation_errors(&context);
}

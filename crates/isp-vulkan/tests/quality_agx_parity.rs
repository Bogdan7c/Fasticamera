//! Сквозной паритет качественного конвейера (V4) с эталоном `isp-model` для основы AgX и своего белого
//! точки (решения 176, 177): RAW → уровни → баланс с обрезкой по `L(pos)` → Menon 2007 → XYZ → W → AgX →
//! цвета выхода → OETF. Допуски — как у прежней основы (`quality_pipeline_parity.rs`): линейный RGB камеры
//! (в нём сидит обрезка по `L`) — бит в бит, сигнал экрана — 1e-3 (шаг fp16 у 1,0 = 4,9e-4).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть способ сообщить о провале"
)]

#[path = "../../isp-model/tests/support/mod.rs"]
mod isp_support;
mod lens_shading_support;
mod look_support;
mod quality_support;

use camera_contract::geometry::PixelSize;
use gpu_vulkan::raw_mosaic_preview::MosaicSource;
use gpu_vulkan::raw_mosaic_preview::reference::pack_raw10;
use gpu_vulkan::raw_mosaic_preview::synthetic::{SyntheticRaw10, SyntheticRaw16};
use isp_model::cfa::BayerPattern;
use isp_model::color::dng::CameraNeutral;
use isp_model::demosaic::DemosaicMethod;
use isp_model::image::{DisplayGamut, RawMosaic};
use isp_model::params::{FrameInputs, IspParams, ProcessingControls};
use isp_model::pipeline::{WorkingReferenceRun, run_reference_working};
use isp_model::sensor::SensorDescription;
use isp_model::test_patterns::zone_plate;
use isp_model::tone::agx_settings::AgxSettings;
use isp_model::tone::base::ToneBase;
use isp_support::real_capture::load_real_capture;
use lens_shading_support::{asymmetric_map, with_map};
use quality_support::{
    area_of, assert_no_validation_errors, gpu_context_or_skip, max_rgb_difference, padded_frame, run_gpu,
};

/// Линейный RGB камеры: бит в бит (в нём — обрезка баланса по белому точки).
const LINEAR_TOLERANCE: f32 = 0.0;
/// Сигнал экрана (fp16-цель): четверть шага 8 бит, как у прежней основы.
const DISPLAY_TOLERANCE: f32 = 1e-3;

fn agx_params(sensor: &SensorDescription, neutral: CameraNeutral, gamut: DisplayGamut) -> IspParams {
    let frame = FrameInputs {
        levels: sensor.static_levels(),
    };
    let controls = ProcessingControls {
        tone_base: ToneBase::Agx(AgxSettings::default()),
        ..ProcessingControls::with_defaults(neutral, DemosaicMethod::Quality, gamut)
    };
    IspParams::resolve(sensor, &frame, &controls).unwrap()
}

fn assert_agx_parity(name: &str, run: &WorkingReferenceRun, linear: &[[f32; 3]], display: &[[f32; 3]]) {
    let width = run.front_end.camera_rgb.size().width;
    let (linear_difference, linear_at) = max_rgb_difference(linear, run.front_end.camera_rgb.pixels(), width);
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
    let encoded = run.display_encoded.pixels();
    let brightest = encoded.iter().flatten().copied().fold(0.0_f32, f32::max);
    let darkest = encoded.iter().flatten().copied().fold(1.0_f32, f32::min);
    assert!(
        brightest - darkest > 0.2,
        "{name}: эталон слишком плоский ({darkest}..{brightest})"
    );
}

/// Эталон доходит до света выше общего белого: иначе тест не проверял бы свой белый точки.
fn assert_reaches_per_point_white(name: &str, run: &WorkingReferenceRun) {
    let brightest = run
        .front_end
        .camera_rgb
        .pixels()
        .iter()
        .flatten()
        .copied()
        .fold(0.0_f32, f32::max);
    assert!(
        brightest > 1.2,
        "{name}: кадр не доходит до белого точки выше 1 (max {brightest})"
    );
}

fn run_both_sources(
    context: &gpu_vulkan::context::GpuContext,
    name: &str,
    raw: &RawMosaic,
    params: &IspParams,
    reference: &WorkingReferenceRun,
) {
    let (frame, frame_width, frame_height) = padded_frame(raw, 1023);
    let raw16 = SyntheticRaw16::new(context, frame_width, frame_height, &frame).unwrap();
    let (linear, display) = run_gpu(context, &raw16.source(), params, area_of(raw));
    assert_agx_parity(&format!("{name}/raw16"), reference, &linear, &display);
    let row_stride_bytes = frame_width / 4 * 5 + 6;
    let packed = pack_raw10(&frame, frame_width, row_stride_bytes, 0xA5);
    let raw10 = SyntheticRaw10::new(context, frame_width, frame_height, row_stride_bytes, &packed).unwrap();
    let source: MosaicSource = raw10.source();
    let (linear, display) = run_gpu(context, &source, params, area_of(raw));
    assert_agx_parity(&format!("{name}/raw10"), reference, &linear, &display);
}

#[test]
fn real_crops_without_correction_match_reference_for_both_outputs() {
    let Some(context) = gpu_context_or_skip() else { return };
    for gamut in [DisplayGamut::Srgb, DisplayGamut::DisplayP3] {
        for name in [
            "real-main-lamp-jar",
            "real-main-lamp-towel",
            "real-main-lamp-tap-highlight",
        ] {
            let capture = load_real_capture(&isp_support::assets_dir(), name);
            let params = agx_params(&capture.sensor, capture.neutral, gamut);
            let reference = run_reference_working(&capture.raw, &params).unwrap();
            run_both_sources(
                &context,
                &format!("{name}/{gamut:?}"),
                &capture.raw,
                &params,
                &reference,
            );
        }
    }
    assert_no_validation_errors(&context);
}

#[test]
fn shaded_real_crops_use_own_white_and_match_reference_bit_for_bit() {
    let Some(context) = gpu_context_or_skip() else { return };
    for name in [
        "real-main-lamp-jar",
        "real-main-lamp-towel",
        "real-main-lamp-tap-highlight",
    ] {
        let capture = load_real_capture(&isp_support::assets_dir(), name);
        let plain = agx_params(&capture.sensor, capture.neutral, DisplayGamut::Srgb);
        let params = with_map(&plain, &asymmetric_map(5, 4));
        let reference = run_reference_working(&capture.raw, &params).unwrap();
        // Карта меняет эталон: иначе совпадение ничего бы не доказывало.
        let unshaded = run_reference_working(&capture.raw, &plain).unwrap();
        let (effect, _) = max_rgb_difference(
            reference.front_end.camera_rgb.pixels(),
            unshaded.front_end.camera_rgb.pixels(),
            48,
        );
        assert!(effect > 0.02, "{name}: карта почти не меняет эталон ({effect})");
        run_both_sources(&context, &format!("{name}/shaded"), &capture.raw, &params, &reference);
    }
    assert_no_validation_errors(&context);
}

#[test]
fn saturated_zone_plate_with_map_matches_reference_for_every_cfa_layout() {
    let Some(context) = gpu_context_or_skip() else { return };
    let calibration = load_real_capture(&isp_support::assets_dir(), "real-main-lamp-gray-tile");
    let scene = zone_plate(PixelSize::new(64, 48), 0.5).unwrap();
    // Карта 7×6: узлы не совпадают с пикселями, усиления до ~2,3 — яркие места насыщают сенсор после коррекции.
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
        let params = with_map(&agx_params(&sensor, calibration.neutral, DisplayGamut::Srgb), &map);
        let levels = sensor.static_levels();
        let neutral = calibration.neutral.values();
        // Свет доходит до насыщения сенсора (множитель 1,3 · нейтраль): часть зонной пластинки — в пересвете.
        let samples: Vec<u16> = (0..scene.size().height)
            .flat_map(|y| (0..scene.size().width).map(move |x| (x, y)))
            .map(|(x, y)| {
                let channel = pattern.channel_at(x, y).index();
                let light = (f64::from(scene.pixel(x, y)[channel]) * 1.3 * neutral[channel]).min(1.0);
                let black = f64::from(levels.black_pattern()[(y as usize % 2) * 2 + x as usize % 2]);
                (black + light * (f64::from(levels.white()) - black)).round() as u16
            })
            .collect();
        let raw = RawMosaic::new(scene.size(), pattern, samples).unwrap();
        let reference = run_reference_working(&raw, &params).unwrap();
        assert_reaches_per_point_white(&format!("zone_plate/{pattern:?}"), &reference);
        let (frame, frame_width, frame_height) = padded_frame(&raw, 0);
        let raw16 = SyntheticRaw16::new(&context, frame_width, frame_height, &frame).unwrap();
        let (linear, display) = run_gpu(&context, &raw16.source(), &params, area_of(&raw));
        assert_agx_parity(&format!("zone_plate/shaded/{pattern:?}"), &reference, &linear, &display);
    }
    assert_no_validation_errors(&context);
}

/// Возврат оттенка (решение 180) в качественном пути: степени 0, 0,5, 1 на реальных кропах и развёртке по оттенку
/// совпадают с эталоном, и каждая степень на развёртке даёт свою картинку.
#[test]
fn hue_restoration_degrees_match_reference_and_each_changes_the_picture() {
    let Some(context) = gpu_context_or_skip() else { return };
    let calibration = load_real_capture(&isp_support::assets_dir(), "real-main-lamp-gray-tile");
    let sweep = look_support::hue_sweep_mosaic(&calibration.sensor, calibration.neutral, 64, 48);
    let mut sweep_pictures: Vec<Vec<[f32; 3]>> = Vec::new();
    for degree in [0.0, 0.5, 1.0] {
        let settings = AgxSettings {
            hue_restoration: degree,
            ..AgxSettings::default()
        };
        let frame = FrameInputs {
            levels: calibration.sensor.static_levels(),
        };
        for gamut in [DisplayGamut::Srgb, DisplayGamut::DisplayP3] {
            let controls = ProcessingControls {
                tone_base: ToneBase::Agx(settings),
                ..ProcessingControls::with_defaults(calibration.neutral, DemosaicMethod::Quality, gamut)
            };
            let params = IspParams::resolve(&calibration.sensor, &frame, &controls).unwrap();
            let reference = run_reference_working(&sweep, &params).unwrap();
            let (padded, frame_width, frame_height) = padded_frame(&sweep, 1023);
            let raw16 = SyntheticRaw16::new(&context, frame_width, frame_height, &padded).unwrap();
            let (linear, display) = run_gpu(&context, &raw16.source(), &params, area_of(&sweep));
            assert_agx_parity(
                &format!("возврат оттенка {degree}/{gamut:?}/развёртка"),
                &reference,
                &linear,
                &display,
            );
            if gamut == DisplayGamut::Srgb {
                sweep_pictures.push(display);
            }
        }
        for name in ["real-main-lamp-jar", "real-main-lamp-tap-highlight"] {
            let capture = load_real_capture(&isp_support::assets_dir(), name);
            let controls = ProcessingControls {
                tone_base: ToneBase::Agx(settings),
                ..ProcessingControls::with_defaults(capture.neutral, DemosaicMethod::Quality, DisplayGamut::Srgb)
            };
            let params = IspParams::resolve(
                &capture.sensor,
                &FrameInputs {
                    levels: capture.sensor.static_levels(),
                },
                &controls,
            )
            .unwrap();
            let reference = run_reference_working(&capture.raw, &params).unwrap();
            run_both_sources(
                &context,
                &format!("возврат оттенка {degree}/{name}"),
                &capture.raw,
                &params,
                &reference,
            );
        }
    }
    assert_ne!(sweep_pictures[0], sweep_pictures[1], "степень 0,5 не отличается от 0");
    assert_ne!(sweep_pictures[1], sweep_pictures[2], "степень 1 не отличается от 0,5");
    assert_no_validation_errors(&context);
}

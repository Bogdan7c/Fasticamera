//! Тон вида (контраст, средние тона, тени, света — `look_apply_tone` в выходном проходе) в КАЧЕСТВЕННОМ конвейере (V4) на
//! GPU против эталона `isp-model` на всех четырёх основах (AgX, ACES, filmic, прежняя), обеих целях вывода и подтяжках AE
//! ±0,5 EV: все 16 углов четырёх ручек на развёртке по оттенку, серой шкале и реальном кропе с пересветом. Допуск —
//! действующий качественного пути (`DISPLAY_TOLERANCE`, как у `quality_look_parity.rs`); разные углы дают разные
//! картинки (ручки доходят до GPU), нейтральные ручки — картинка основы бит в бит.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть способ сообщить о провале"
)]

#[path = "../../isp-model/tests/support/mod.rs"]
mod isp_support;
mod look_support;
mod quality_support;

use gpu_vulkan::raw_mosaic_preview::synthetic::SyntheticRaw16;
use isp_model::color::dng::CameraNeutral;
use isp_model::demosaic::DemosaicMethod;
use isp_model::image::{DisplayGamut, RawMosaic};
use isp_model::look::LookControls;
use isp_model::params::{ExposureGain, FrameInputs, IspParams, ProcessingControls};
use isp_model::pipeline::run_reference_encoded;
use isp_model::sensor::SensorDescription;
use isp_model::tone::aces::AcesSettings;
use isp_model::tone::agx_settings::AgxSettings;
use isp_model::tone::base::ToneBase;
use isp_model::tone::filmic::FilmicSettings;
use isp_support::real_capture::load_real_capture;
use look_support::{TONE_CORNER_COUNT, tone_corner};
use quality_support::{
    area_of, assert_no_validation_errors, gpu_context_or_skip, max_rgb_difference, padded_frame, run_gpu,
};

/// Сигнал экрана (fp16-цель): допуск качественного пути, как у `quality_look_parity.rs`.
const DISPLAY_TOLERANCE: f32 = 1e-3;
/// Подтяжки AE на краях.
const AE_EDGES_EV: [f32; 2] = [-0.5, 0.5];

fn four_bases() -> [(&'static str, ToneBase); 4] {
    [
        ("AgX", ToneBase::Agx(AgxSettings::default())),
        ("ACES", ToneBase::Aces(AcesSettings::default())),
        ("filmic", ToneBase::Filmic(FilmicSettings::default())),
        ("прежняя", ToneBase::Legacy),
    ]
}

fn params(
    sensor: &SensorDescription,
    neutral: CameraNeutral,
    gamut: DisplayGamut,
    base: ToneBase,
    look: LookControls,
    exposure_ev: f32,
) -> IspParams {
    let frame = FrameInputs {
        levels: sensor.static_levels(),
    };
    let controls = ProcessingControls {
        tone_base: base,
        exposure_gain: ExposureGain::from_ev(exposure_ev).unwrap(),
        look,
        ..ProcessingControls::with_defaults(neutral, DemosaicMethod::Quality, gamut)
    };
    IspParams::resolve(sensor, &frame, &controls).unwrap()
}

/// Сигнал экрана GPU и наибольшее |Δ| против эталона.
fn gpu_display(
    context: &gpu_vulkan::context::GpuContext,
    raw: &RawMosaic,
    params: &IspParams,
) -> (f32, (u32, u32), Vec<[f32; 3]>) {
    let (frame, frame_width, frame_height) = padded_frame(raw, 1023);
    let source = SyntheticRaw16::new(context, frame_width, frame_height, &frame).unwrap();
    let (_, display) = run_gpu(context, &source.source(), params, area_of(raw));
    let reference = run_reference_encoded(raw, params).unwrap();
    let (difference, at) = max_rgb_difference(&display, reference.pixels(), raw.size().width);
    (difference, at, display)
}

/// Кадры паритета: развёртка по оттенку, серая шкала и реальный кроп с пересветом (своя калибровка у каждого).
fn frames() -> Vec<(&'static str, SensorDescription, CameraNeutral, RawMosaic)> {
    let calibration = load_real_capture(&isp_support::assets_dir(), "real-main-lamp-gray-tile");
    let sweep = look_support::hue_sweep_mosaic(&calibration.sensor, calibration.neutral, 64, 48);
    let ramp = look_support::gray_ramp_mosaic(&calibration.sensor, calibration.neutral, 64, 48);
    let highlight = load_real_capture(&isp_support::assets_dir(), "real-main-lamp-tap-highlight");
    vec![
        (
            "развёртка по оттенку",
            calibration.sensor.clone(),
            calibration.neutral,
            sweep,
        ),
        ("серая шкала", calibration.sensor, calibration.neutral, ramp),
        (
            "реальный кроп с пересветом",
            highlight.sensor,
            highlight.neutral,
            highlight.raw,
        ),
    ]
}

#[test]
fn tone_knob_corners_with_ae_edges_match_reference_on_every_base_and_output() {
    let Some(context) = gpu_context_or_skip() else { return };
    let frames = frames();
    let mut failures = Vec::new();
    for (base_name, base) in four_bases() {
        let mut worst = 0.0_f32;
        for gamut in [DisplayGamut::DisplayP3, DisplayGamut::Srgb] {
            let mut ramp_pictures = Vec::new();
            for exposure_ev in AE_EDGES_EV {
                for corner in 0..TONE_CORNER_COUNT {
                    for (scene, sensor, neutral, raw) in &frames {
                        let params = params(sensor, *neutral, gamut, base, tone_corner(corner), exposure_ev);
                        assert!(params.look.tone.is_some(), "угол {corner:04b}: стадия тона включена");
                        let (difference, at, display) = gpu_display(&context, raw, &params);
                        worst = worst.max(difference);
                        if difference > DISPLAY_TOLERANCE {
                            failures.push(format!(
                                "{base_name} {gamut:?} угол {corner:04b} AE {exposure_ev:+} {scene}: \
                                 max |Δ| {difference:e} в {at:?} > {DISPLAY_TOLERANCE:e}"
                            ));
                        }
                        if exposure_ev < 0.0 && *scene == "серая шкала" {
                            ramp_pictures.push(display);
                        }
                    }
                }
            }
            for (index, picture) in ramp_pictures.iter().enumerate() {
                for (other_index, other) in ramp_pictures.iter().enumerate().skip(index + 1) {
                    assert_ne!(
                        picture, other,
                        "{base_name} {gamut:?}: углы {index} и {other_index} дали одну картинку"
                    );
                }
            }
        }
        eprintln!("{base_name}: худший max |Δ| сигнала экрана = {worst:.3e} (допуск {DISPLAY_TOLERANCE:e})");
    }
    assert_no_validation_errors(&context);
    assert!(
        failures.is_empty(),
        "углы ручек тона в качественном пути: {} нарушений:\n  {}",
        failures.len(),
        failures.join("\n  ")
    );
}

#[test]
fn neutral_tone_knobs_equal_the_base_bit_for_bit_and_a_touched_knob_changes_the_quality_picture() {
    let Some(context) = gpu_context_or_skip() else { return };
    let (_, sensor, neutral, sweep) = frames().remove(0);
    for (base_name, base) in four_bases() {
        let plain = params(
            &sensor,
            neutral,
            DisplayGamut::DisplayP3,
            base,
            LookControls::default(),
            0.0,
        );
        let zeroed = params(
            &sensor,
            neutral,
            DisplayGamut::DisplayP3,
            base,
            look_support::tone_look(0.0, 0.0, 0.0, 0.0),
            0.0,
        );
        assert!(
            zeroed.look.tone.is_none(),
            "{base_name}: нулевые ручки — стадия пропущена"
        );
        let (_, _, plain_picture) = gpu_display(&context, &sweep, &plain);
        let (_, _, zeroed_picture) = gpu_display(&context, &sweep, &zeroed);
        assert_eq!(
            plain_picture, zeroed_picture,
            "{base_name}: нулевые ручки тона ≠ основа"
        );
        for (knob_name, values) in [
            ("контраст", [0.5, 0.0, 0.0, 0.0]),
            ("средние", [0.0, 0.5, 0.0, 0.0]),
            ("тени", [0.0, 0.0, 0.5, 0.0]),
            ("света", [0.0, 0.0, 0.0, 0.5]),
        ] {
            let [contrast, midtones, shadows, highlights] = values;
            let touched = params(
                &sensor,
                neutral,
                DisplayGamut::DisplayP3,
                base,
                look_support::tone_look(contrast, midtones, shadows, highlights),
                0.0,
            );
            let (_, _, picture) = gpu_display(&context, &sweep, &touched);
            assert_ne!(
                picture, plain_picture,
                "{base_name}: ручка «{knob_name}» не изменила картинку"
            );
        }
    }
    assert_no_validation_errors(&context);
}

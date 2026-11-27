//! Тон вида (контраст, средние тона, тени, света — стадия `look_apply_tone` после основы, решения 236, 242–246) в
//! ВИДОИСКАТЕЛЕ на GPU (RADV, validation) против эталона `isp-model` на ВСЕХ четырёх основах (AgX — умолчание, ACES,
//! filmic, прежняя). Допуски — действующие (`ParityMeasure::violations`): ошибка математики сверх шага хранения
//! ≤ 1e-5, max |Δ| ≤ ½ LSB 10 бит + 1e-5, ноль пикселей хуже 1 LSB; в обоих форматах выхода (fp16 и 10 бит).
//!
//! Что проверяется:
//! * все 16 углов четырёх ручек × подтяжка AE ±0,5 EV × три сцены (серая шкала, яркие насыщенные, развёртка по
//!   оттенку) на каждой основе; разные углы — разные картинки (ручки доходят до шейдера);
//! * тон вместе с цветом (колёса, насыщенность и сочность, полосы HUE ±60, таблица-«единица», теплота) на AgX и
//!   прежней основе, обе цели вывода; тон в этой связке действительно меняет картинку;
//! * бит в бит на ОДНОМ ISP: нейтраль → цвет → тон → тон+цвет → цвет → тон → нейтраль; строки тона и цвета не
//!   «залипают», вариант pipeline один;
//! * срез стадии `LookColor` вырезает и тон, и цвет (набор стадий — единый выключатель слоя вида над светом экрана).
//!
//! Нарушения собираются по всем случаям и печатаются разом: тест падает ОДИН раз со всем списком, а не на первом.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть способ сообщить о провале"
)]

mod lens_shading_support;
mod look_support;
mod viewfinder_support;

use camera_contract::geometry::SensorOrientation;
use isp_model::image::DisplayGamut;
use isp_model::look::LookControls;
use isp_model::params::ExposureGain;
use isp_model::tone::aces::AcesSettings;
use isp_model::tone::agx_settings::AgxSettings;
use isp_model::tone::base::ToneBase;
use isp_model::tone::filmic::FilmicSettings;
use isp_vulkan::viewfinder::{IspStage, IspStageSet, ViewfinderIsp, ViewfinderStorageFormat};
use look_contract::{BandAdjustments, LookSettings};
use look_support::{
    TONE_CORNER_COUNT, all_wheels, band, controls, identity_lut, knob, tone_corner, tone_corner_is_ill_conditioned,
    tone_corner_settings, tone_look, with_lut,
};
use viewfinder_support::{
    ErrorAllowance, ParamsSpec, RawFrame, RunRequest, assert_no_validation_errors, gpu_context_or_skip, hue_sweep,
    measure_viewfinder_parity, reference_upright_encoded, run_viewfinder, synthetic_scenes, test_config, tone_scenes,
};

/// Подтяжки AE на краях (±0,5 EV — край остаточной подтяжки продукта).
const AE_EDGES_EV: [f32; 2] = [-0.5, 0.5];

/// Четыре основы тона с умолчаниями продукта.
fn four_bases() -> [(&'static str, ToneBase); 4] {
    [
        ("AgX", ToneBase::Agx(AgxSettings::default())),
        ("ACES", ToneBase::Aces(AcesSettings::default())),
        ("filmic", ToneBase::Filmic(FilmicSettings::default())),
        ("прежняя", ToneBase::Legacy),
    ]
}

fn spec(base: ToneBase, gamut: DisplayGamut) -> ParamsSpec {
    ParamsSpec {
        gamut,
        tone_base: base,
        ..ParamsSpec::default()
    }
}

/// Картинка GPU (fp16) кадра — для сравнения картинок между собой.
fn gpu_picture(
    context: &gpu_vulkan::context::GpuContext,
    raw: &RawFrame,
    params: &isp_model::params::IspParams,
    spec: &ParamsSpec,
) -> Vec<[f32; 4]> {
    run_viewfinder(
        context,
        &test_config(ViewfinderStorageFormat::HalfFloatOnly),
        &RunRequest::whole_frame(raw, params, spec.display()),
    )
    .pixels
}

/// Обрыв теста со всеми нарушениями сразу (пусто — паритет держится).
fn assert_no_violations(what: &str, failures: &[String]) {
    assert!(
        failures.is_empty(),
        "{what}: {} нарушений допусков паритета:\n  {}",
        failures.len(),
        failures.join("\n  ")
    );
}

/// Прогон всех 16 углов ручек тона на всех основах: паритет с эталоном по трём сценам и подтяжкам AE ±0,5; возвращает
/// нарушения допусков (пусто — держится). Углы дают разные картинки серой шкалы — ручки доходят до GPU.
fn corners_parity_violations(context: &gpu_vulkan::context::GpuContext) -> Vec<String> {
    let mut failures = Vec::new();
    for (base_name, base) in four_bases() {
        let spec = spec(base, DisplayGamut::DisplayP3);
        let scenes = tone_scenes(&spec);
        let (mut worst_excess, mut worst_steep_excess) = (0.0_f32, 0.0_f32);
        let mut ramp_pictures = Vec::new();
        for exposure_ev in AE_EDGES_EV {
            for corner in 0..TONE_CORNER_COUNT {
                let params =
                    spec.build_with_look_and_exposure(tone_corner(corner), ExposureGain::from_ev(exposure_ev).unwrap());
                assert!(params.look.tone.is_some(), "угол {corner:04b}: стадия тона включена");
                let allowance = if tone_corner_is_ill_conditioned(corner) {
                    ErrorAllowance::SteepToneAtWhite
                } else {
                    ErrorAllowance::Standard
                };
                for (scene, raw) in &scenes {
                    let measure = measure_viewfinder_parity(context, raw, &params, &spec);
                    let worst = if allowance == ErrorAllowance::Standard {
                        &mut worst_excess
                    } else {
                        &mut worst_steep_excess
                    };
                    *worst = worst.max(measure.worst_math_excess());
                    failures.extend(measure.violations(allowance).into_iter().map(|violation| {
                        format!("{base_name} угол {corner:04b} AE {exposure_ev:+} {scene}: {violation}")
                    }));
                }
                if exposure_ev < 0.0 {
                    ramp_pictures.push(gpu_picture(context, &scenes[0].1, &params, &spec));
                }
            }
        }
        eprintln!(
            "{base_name}: худшая ошибка математики сверх шага хранения = {worst_excess:.3e}, у обрывистых углов = {worst_steep_excess:.3e}"
        );
        for (index, picture) in ramp_pictures.iter().enumerate() {
            for (other_index, other) in ramp_pictures.iter().enumerate().skip(index + 1) {
                assert_ne!(
                    picture, other,
                    "{base_name}: углы {index} и {other_index} дали одну картинку"
                );
            }
        }
    }
    failures
}

#[test]
fn tone_knob_corners_with_ae_edges_match_reference_on_every_base() {
    let Some(context) = gpu_context_or_skip() else { return };
    let failures = corners_parity_violations(&context);
    assert_no_validation_errors(&context);
    assert_no_violations("углы ручек тона", &failures);
}

/// Вид «тон + цвет»: ручки тона из `tone`, цвет по случаю. Возвращает (имя случая, вид).
fn tone_with_colour_cases(tone: LookSettings) -> Vec<(&'static str, LookControls)> {
    let with = |colour: LookSettings| LookSettings {
        contrast: tone.contrast,
        midtones: tone.midtones,
        shadows: tone.shadows,
        highlights: tone.highlights,
        ..colour
    };
    // Полосы на краях HUE ±60: красная сдвигается в плюс, зелёная в минус; плюс насыщенность и яркость.
    let bands = BandAdjustments::new(vec![band(0, 1.0, 0.5, -0.4), band(3, -1.0, -0.6, 0.5)]).unwrap();
    let everything = LookSettings {
        wheels: all_wheels(),
        saturation: knob(0.6),
        vibrance: knob(-0.4),
        bands: bands.clone(),
        warmth: knob(0.4),
        tint: knob(-0.3),
        ..LookSettings::NEUTRAL
    };
    vec![
        (
            "тон + колёса",
            controls(with(LookSettings {
                wheels: all_wheels(),
                ..LookSettings::NEUTRAL
            })),
        ),
        (
            "тон + насыщенность и сочность",
            controls(with(LookSettings {
                saturation: knob(0.6),
                vibrance: knob(-0.4),
                ..LookSettings::NEUTRAL
            })),
        ),
        (
            "тон + полосы HUE ±60",
            controls(with(LookSettings {
                bands,
                ..LookSettings::NEUTRAL
            })),
        ),
        (
            "тон + таблица-единица",
            with_lut(controls(with(LookSettings::NEUTRAL)), identity_lut()),
        ),
        (
            "тон + теплота",
            controls(with(LookSettings {
                warmth: knob(0.4),
                ..LookSettings::NEUTRAL
            })),
        ),
        (
            "тон + всё сразу + таблица",
            with_lut(controls(with(everything)), identity_lut()),
        ),
    ]
}

/// Ручки тона под цветом: «умеренный» (ход ±0,6: рабочая зона владельца) и края «всё в плюс» (самый крутой наклон).
/// Края «всё в минус» здесь нет: он плохо обусловлен сам по себе (`tone_corner_is_ill_conditioned`).
fn tone_settings_under_colour() -> Vec<(&'static str, LookSettings)> {
    vec![
        (
            "тон ±0,6 (контраст+, средние−, тени+, света−)",
            look_support::tone_settings(0.6, -0.6, 0.6, -0.6),
        ),
        (
            "тон ∓0,6 (контраст−, средние+, тени−, света+)",
            look_support::tone_settings(-0.6, 0.6, -0.6, 0.6),
        ),
        ("тон край +1 всех ручек", tone_corner_settings(0b1111)),
    ]
}

/// Тот же вид с нейтральными ручками тона (цвет, теплота и таблица те же).
fn without_tone(look: &LookControls) -> LookControls {
    let mut settings = look.settings.clone();
    (
        settings.contrast,
        settings.midtones,
        settings.shadows,
        settings.highlights,
    ) = (knob(0.0), knob(0.0), knob(0.0), knob(0.0));
    let mut untoned = LookControls::new(settings, look.model.clone());
    untoned.lut = look.lut.clone();
    untoned
}

/// Наибольшее отличие двух эталонных картинок.
fn biggest_reference_difference(
    raw: &RawFrame,
    left: &isp_model::params::IspParams,
    right: &isp_model::params::IspParams,
) -> f32 {
    let encode = |params| reference_upright_encoded(raw, raw.full_rect(), SensorOrientation::Degrees0, params);
    encode(left)
        .pixels
        .iter()
        .zip(&encode(right).pixels)
        .flat_map(|(first, second)| first.iter().zip(second).map(|(a, b)| (a - b).abs()))
        .fold(0.0_f32, f32::max)
}

/// Паритет «тон + цвет» для пар (основа, цель вывода): нарушения допусков по всем случаям.
fn tone_with_colour_violations(
    context: &gpu_vulkan::context::GpuContext,
    targets: &[(&str, ToneBase, DisplayGamut)],
) -> Vec<String> {
    let mut failures = Vec::new();
    for (base_name, base, gamut) in targets {
        let spec = spec(*base, *gamut);
        let scenes = tone_scenes(&spec);
        for (tone_name, tone) in tone_settings_under_colour() {
            for (case, look_of_case) in tone_with_colour_cases(tone.clone()) {
                let params = spec.build_with_look(look_of_case.clone());
                assert!(params.look.tone.is_some(), "{case}: стадия тона включена");
                assert!(
                    params.look.color.is_some() || params.look.lut.is_some() || params.look.white_shift.mired != 0.0,
                    "{case}: цвет, таблица или теплота тоже включены"
                );
                let untoned_params = spec.build_with_look(without_tone(&look_of_case));
                let allowance = if *base == ToneBase::Legacy && *gamut == DisplayGamut::Srgb {
                    ErrorAllowance::LegacySrgbExtremeColour
                } else {
                    ErrorAllowance::Standard
                };
                for (scene, raw) in &scenes {
                    let violations = measure_viewfinder_parity(context, raw, &params, &spec).violations(allowance);
                    if violations.is_empty() {
                        continue;
                    }
                    // Откуда ошибка: тот же вид без тона. Нарушает и он — ошибка в цветовой стадии, не в тоне.
                    let without_tone_violations = measure_viewfinder_parity(context, raw, &untoned_params, &spec)
                        .violations(allowance)
                        .len();
                    failures.extend(violations.into_iter().map(|violation| {
                        format!(
                            "{base_name} {gamut:?} {tone_name} {case} {scene}: {violation} \
                             (тот же вид без тона: {without_tone_violations} нарушений)"
                        )
                    }));
                }
            }
        }
    }
    failures
}

#[test]
fn tone_together_with_colour_wheels_bands_lut_and_warmth_matches_reference() {
    let Some(context) = gpu_context_or_skip() else { return };
    let agx = ToneBase::Agx(AgxSettings::default());
    let failures = tone_with_colour_violations(
        &context,
        &[
            ("AgX", agx, DisplayGamut::DisplayP3),
            ("AgX", agx, DisplayGamut::Srgb),
            ("прежняя", ToneBase::Legacy, DisplayGamut::DisplayP3),
            ("прежняя", ToneBase::Legacy, DisplayGamut::Srgb),
        ],
    );
    assert_no_validation_errors(&context);
    assert_no_violations("тон вместе с цветом", &failures);
}

#[test]
fn tone_changes_the_picture_of_every_colour_case() {
    // Эталон тона не пуст: тот же цвет без тона и с тоном (угол «всё в плюс» и «всё в минус») — разные картинки.
    for (base_name, base) in four_bases() {
        let spec = spec(base, DisplayGamut::DisplayP3);
        let raw = hue_sweep(&spec);
        for (tone_name, tone) in tone_settings_under_colour() {
            for (case, toned) in &tone_with_colour_cases(tone.clone()) {
                let difference = biggest_reference_difference(
                    &raw,
                    &spec.build_with_look(without_tone(toned)),
                    &spec.build_with_look(toned.clone()),
                );
                assert!(
                    difference > 0.01,
                    "{base_name} {tone_name} {case}: тон почти не меняет картинку ({difference})"
                );
            }
        }
    }
}

#[test]
fn neutral_tone_colour_sequence_on_one_isp_is_bit_exact_and_keeps_one_pipeline_variant() {
    let Some(context) = gpu_context_or_skip() else { return };
    for (base_name, base) in four_bases() {
        let spec = spec(base, DisplayGamut::DisplayP3);
        let raw = hue_sweep(&spec);
        let config = test_config(ViewfinderStorageFormat::HalfFloatOnly);
        let colour_settings = LookSettings {
            saturation: knob(0.7),
            vibrance: knob(0.5),
            wheels: all_wheels(),
            ..LookSettings::NEUTRAL
        };
        let tone_settings = tone_corner_settings(0b0110);
        let both_settings = LookSettings {
            contrast: tone_settings.contrast,
            midtones: tone_settings.midtones,
            shadows: tone_settings.shadows,
            highlights: tone_settings.highlights,
            ..colour_settings.clone()
        };
        let neutral = spec.build();
        let colour = spec.build_with_look(controls(colour_settings));
        let tone = spec.build_with_look(controls(tone_settings));
        let both = spec.build_with_look(controls(both_settings));
        assert!(neutral.look.tone.is_none() && neutral.look.color.is_none());
        assert!(colour.look.tone.is_none() && colour.look.color.is_some());
        assert!(tone.look.tone.is_some() && tone.look.color.is_none());
        assert!(both.look.tone.is_some() && both.look.color.is_some());

        // Нейтраль = картинка основы: набор без стадий вида даёт ту же картинку бит в бит.
        let request = |params| RunRequest::whole_frame(&raw, params, spec.display());
        let product = run_viewfinder(&context, &config, &request(&neutral));
        let without_look = run_viewfinder(
            &context,
            &config,
            &RunRequest {
                stages: IspStageSet::ALL.without(IspStage::LookColor).without(IspStage::LookLut),
                ..request(&neutral)
            },
        );
        assert_eq!(product.pixels, without_look.pixels, "{base_name}: нейтраль ≠ основа");
        // Тот же набор вырезает и тон: вид «только тон» без стадии вида равен нейтрали (единый выключатель).
        let tone_cut = run_viewfinder(
            &context,
            &config,
            &RunRequest {
                stages: IspStageSet::ALL.without(IspStage::LookColor),
                ..request(&tone)
            },
        );
        assert_eq!(
            tone_cut.pixels, product.pixels,
            "{base_name}: срез LookColor не вырезал тон"
        );

        let mut isp = ViewfinderIsp::new(&context, &config).unwrap();
        let gpu_source = viewfinder_support::GpuSource::new(&context, &raw, viewfinder_support::SourceLayout::Raw16);
        let source = gpu_source.source();
        // нейтраль → цвет → тон → тон+цвет → цвет → тон → нейтраль: ни одна строка блока не «залипает».
        let sequence = [&neutral, &colour, &tone, &both, &colour, &tone, &neutral];
        let mut runs = Vec::new();
        for (frame, params) in sequence.into_iter().enumerate() {
            viewfinder_support::record_and_submit(
                &context,
                &mut isp,
                &request(params),
                &source,
                frame % 2,
                gpu_vulkan::backdrop::BackdropFrameNumber(frame as u64 + 1),
            );
            runs.push(viewfinder_support::readback(&mut isp, frame % 2));
        }
        let pixels = |index: usize| &runs[index].pixels;
        assert_ne!(pixels(0), pixels(1), "{base_name}: цвет не изменил картинку");
        assert_ne!(pixels(0), pixels(2), "{base_name}: тон не изменил картинку");
        assert_ne!(pixels(1), pixels(3), "{base_name}: тон не изменил картинку с цветом");
        assert_ne!(pixels(2), pixels(3), "{base_name}: цвет не изменил картинку с тоном");
        assert_eq!(
            pixels(1),
            pixels(4),
            "{base_name}: «цвет» после тона не вернулся бит в бит (строки тона залипли)"
        );
        assert_eq!(
            pixels(2),
            pixels(5),
            "{base_name}: «тон» после цвета не вернулся бит в бит (строки цвета залипли)"
        );
        assert_eq!(pixels(0), pixels(6), "{base_name}: возврат к нейтрали не бит в бит");
        assert_eq!(
            pixels(0),
            &product.pixels,
            "{base_name}: разные ISP дали разную нейтраль"
        );
        assert_eq!(
            isp.cached_pipeline_variants(),
            1,
            "{base_name}: смена «нейтраль ↔ цвет ↔ тон» создала новый вариант pipeline"
        );
    }
    assert_no_validation_errors(&context);
}

#[test]
fn single_tone_knob_each_changes_the_picture_and_neutral_knobs_are_exactly_the_base() {
    // Каждая ручка отдельно на ±1 меняет серую шкалу; все четыре в нуле — картинка основы бит в бит.
    let Some(context) = gpu_context_or_skip() else { return };
    for (base_name, base) in four_bases() {
        let spec = spec(base, DisplayGamut::DisplayP3);
        let ramp = synthetic_scenes(&spec).remove(0).1;
        let neutral_picture = gpu_picture(&context, &ramp, &spec.build(), &spec);
        let zero_knobs = gpu_picture(
            &context,
            &ramp,
            &spec.build_with_look(tone_look(0.0, 0.0, 0.0, 0.0)),
            &spec,
        );
        assert_eq!(zero_knobs, neutral_picture, "{base_name}: нулевые ручки тона ≠ основа");
        for (knob_name, settings) in [
            ("контраст", [1.0, 0.0, 0.0, 0.0]),
            ("средние", [0.0, 1.0, 0.0, 0.0]),
            ("тени", [0.0, 0.0, 1.0, 0.0]),
            ("света", [0.0, 0.0, 0.0, 1.0]),
        ] {
            for sign in [1.0_f32, -1.0] {
                let [contrast, midtones, shadows, highlights] = settings.map(|value| value * sign);
                let params = spec.build_with_look(tone_look(contrast, midtones, shadows, highlights));
                assert_ne!(
                    gpu_picture(&context, &ramp, &params, &spec),
                    neutral_picture,
                    "{base_name}: ручка «{knob_name}» {sign:+} не изменила картинку"
                );
            }
        }
    }
    assert_no_validation_errors(&context);
}

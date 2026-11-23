//! Выключатели стадий видоискателя у основы AgX: каждый вариант шейдера собирается, работает и меняет картинку
//! ровно так, как записано в `viewfinder::stages` (сверка с эмуляцией на CPU из функций `isp-model`), все стадии
//! включены = эталон. Аналог `viewfinder_stages.rs` для прежней основы; здесь же — стадии вида (цвет и таблица).
//!
//! Эмуляция выключенной стадии «сжатие охвата / вход» (`GamutCompression`) выразима, только пока свет сцены в W
//! неотрицателен (вход «обрезка нулём» тогда тождествен): кадр теста подобран так, и это проверяется.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть способ сообщить о провале"
)]

mod lens_shading_support;
mod look_support;
mod viewfinder_support;

use camera_contract::geometry::{PixelSize, SensorOrientation};
use isp_model::cfa::{CfaCell, RgbChannel};
use isp_model::image::{DisplayGamut, Mosaic, ShadingCorrectedMosaic, mosaic_stage};
use isp_model::lens_shading::{
    CropInValidArea, LensShadingSampling, LensShadingSkipReason, LensShadingStep, apply_lens_shading_with,
};
use isp_model::levels::normalize_sample;
use isp_model::params::IspParams;
use isp_model::test_patterns::color_bars;
use isp_model::tone::agx_settings::AgxSettings;
use isp_model::tone::base::{ResolvedToneBase, ToneBase};
use isp_model::transfer::srgb_encode;
use isp_model::white_level::{WhiteLevelField, WhiteLevelPlane};
use isp_vulkan::viewfinder::{IspStage, IspStageSet, ViewfinderStorageFormat};
use viewfinder_support::{
    ParamsSpec, RawFrame, RunRequest, ViewfinderRun, assert_no_validation_errors, gpu_context_or_skip,
    reference_upright_encoded, run_viewfinder, storage_step, test_config,
};

const WIDTH: u32 = 48;
const HEIGHT: u32 = 32;
/// Допуск ошибки вычислений сверх шага хранения fp16 (обоснование — `viewfinder_parity.rs`).
const MATH_EXCESS_TOLERANCE: f32 = 1.0e-5;

fn shading_gains(raw: &RawFrame, params: &IspParams) -> ShadingCorrectedMosaic {
    let ones = Mosaic::from_samples(
        PixelSize::new(raw.width, raw.height),
        raw.pattern,
        vec![1.0; (raw.width * raw.height) as usize],
        mosaic_stage::Normalized,
    )
    .unwrap();
    apply_lens_shading_with(&ones, &params.lens_shading, LensShadingSampling::PerCellCentre).0
}

/// Эмуляция прохода AgX на CPU с семантикой выключенных стадий (таблица в `viewfinder::stages`). Возвращает
/// пиксели выхода и наименьший канал света сцены в W (для проверки допущения о выключенном входе).
fn emulate(raw: &RawFrame, params: &IspParams, stages: IspStageSet) -> (Vec<[f32; 3]>, f32) {
    let ResolvedToneBase::Agx(base) = &params.tone_base else {
        panic!("тест AgX");
    };
    let agx = base.parameters();
    let on = |stage| stages.contains(stage);
    let shading = shading_gains(raw, params);
    let gains = params.white_balance.as_array();
    let size = PixelSize::new(raw.width, raw.height);
    // Стадия «виньетирование» выключена — карта не читается и в белом точки: он считается как без коррекции.
    let skipped = LensShadingStep::Skip(LensShadingSkipReason::NotCalibrated);
    let field = WhiteLevelField::new(
        if on(IspStage::LensShading) {
            &params.lens_shading
        } else {
            &skipped
        },
        &params.white_balance,
        CropInValidArea::whole(size),
        LensShadingSampling::PerCellCentre,
    );
    let whites = WhiteLevelPlane::for_demosaic(&field, params.demosaic, size);
    let mut lowest_scene = f32::MAX;
    let mut pixels = Vec::new();
    for cell_y in 0..raw.height / 2 {
        for cell_x in 0..raw.width / 2 {
            let white = whites.level(cell_x, cell_y);
            let mut balanced = [0.0_f32; 4];
            let mut channels = [RgbChannel::Red; 4];
            for (position, (dx, dy)) in [(0, 0), (1, 0), (0, 1), (1, 1)].into_iter().enumerate() {
                let (x, y) = (cell_x * 2 + dx, cell_y * 2 + dy);
                let value = raw.samples[(y * raw.width + x) as usize];
                let channel = params.pattern.channel_at(x, y);
                let normalized = if on(IspStage::Levels) {
                    normalize_sample(value, CfaCell::of_pixel(x, y), &params.levels)
                } else {
                    f32::from(value) / params.levels.white()
                };
                let shaded = if on(IspStage::LensShading) {
                    normalized * shading.sample(x, y)
                } else {
                    normalized
                };
                // Обрезка после баланса — по своему белому точки (у AgX, решение 177).
                balanced[position] = if on(IspStage::WhiteBalance) {
                    (shaded * gains[channel.index()]).min(white)
                } else {
                    shaded
                };
                channels[position] = channel;
            }
            let camera_rgb = if on(IspStage::Demosaic) {
                let mut color = [0.0_f32; 3];
                let mut green_sum = 0.0_f32;
                for (sample, channel) in balanced.iter().zip(channels) {
                    match channel {
                        RgbChannel::Green => green_sum += sample,
                        other => color[other.index()] = *sample,
                    }
                }
                color[1] = green_sum / 2.0;
                color
            } else {
                [balanced[0]; 3]
            };
            let scene = if on(IspStage::Color) {
                base.xyz_d50_to_working(params.color.camera_to_xyz_d50().apply(camera_rgb))
            } else {
                camera_rgb
            };
            lowest_scene = lowest_scene.min(scene.into_iter().fold(f32::MAX, f32::min));
            // Вход: у ClipToZero при неотрицательном свете тождество, поэтому «вход выключен + основа включена» =
            // `map_scene_pixel` без изменений (допущение проверяет `lowest_scene`); «основа выключена» = только вход.
            let shaped = if on(IspStage::Tone) {
                base.map_scene_pixel(scene, white)
            } else if on(IspStage::GamutCompression) {
                base.apply_input(scene)
            } else {
                scene
            };
            let looked = match &params.look.color {
                Some(color) if on(IspStage::LookColor) => color.apply(shaped),
                _ => shaped,
            };
            let looked = match &params.look.lut {
                Some(lut) if on(IspStage::LookLut) => lut.apply(looked),
                _ => looked,
            };
            let display = if on(IspStage::OutputGamutCompression) {
                base.to_output(looked)
            } else {
                // Только матрица и обрезка (мягкое сжатие охвата выхода выключено).
                let rows = agx.working_to_output;
                let row = |r: [f32; 3]| r[0] * looked[0] + r[1] * looked[1] + r[2] * looked[2];
                [row(rows[0]), row(rows[1]), row(rows[2])].map(|channel| channel.clamp(0.0, 1.0))
            };
            pixels.push(if on(IspStage::Oetf) {
                display.map(srgb_encode)
            } else {
                display
            });
        }
    }
    (pixels, lowest_scene)
}

/// Наибольшая ошибка сверх шага хранения fp16 между GPU и эмуляцией.
fn excess_over_storage(run: &ViewfinderRun, expected: &[[f32; 3]]) -> f32 {
    let mut excess = 0.0_f32;
    for (gpu, cpu) in run.pixels.iter().zip(expected) {
        for channel in 0..3 {
            let difference = (gpu[channel] - cpu[channel]).abs();
            excess = excess.max(difference - storage_step(run.format, cpu[channel]));
        }
    }
    excess
}

#[test]
fn each_stage_off_variant_matches_the_cpu_emulation_for_agx() {
    let Some(context) = gpu_context_or_skip() else { return };
    let spec = ParamsSpec {
        gamut: DisplayGamut::Srgb,
        tone_base: ToneBase::Agx(AgxSettings::default()),
        ..ParamsSpec::default()
    };
    // Карта виньетирования: стадия «виньетирование» не пропущена решением кадра, а свой белый точки разный по полю.
    // Вид с цветом и таблицей: стадиям вида есть что делать.
    let look = look_support::with_lut(look_support::saturation_vibrance(0.6, 0.3), look_support::known_lut());
    let params =
        lens_shading_support::with_map(&spec.build_with_look(look), &lens_shading_support::asymmetric_map(5, 4));
    // Цветные полосы на серой подложке (свет остаётся внутри охвата W: неотрицателен), яркость растёт вниз до пересвета.
    let bars = color_bars(PixelSize::new(WIDTH, HEIGHT), 1.0).unwrap();
    let raw = RawFrame::from_linear(WIDTH, HEIGHT, spec.pattern, spec.levels(), |x, y, channel| {
        let level = 0.05 + 1.3 * f64::from(y) / f64::from(HEIGHT - 1);
        let gray = spec.neutral[channel] / spec.neutral.iter().copied().fold(f64::MIN, f64::max);
        (0.55 * gray + 0.45 * f64::from(bars.pixel(x, y)[channel])) * level
    });
    let config = test_config(ViewfinderStorageFormat::HalfFloatOnly);

    // Эмуляция «всё включено» совпадает с эталоном `isp-model` (иначе сверка вариантов ничего бы не доказывала).
    let reference = reference_upright_encoded(&raw, raw.full_rect(), SensorOrientation::Degrees0, &params);
    let (all_on, lowest) = emulate(&raw, &params, IspStageSet::ALL);
    assert_eq!(
        all_on, reference.pixels,
        "эмуляция расходится с эталоном при всех включённых стадиях"
    );
    assert!(lowest >= 0.0, "кадр теста вышел за охват W (наименьший канал {lowest})");

    let product = run_viewfinder(
        &context,
        &config,
        &RunRequest::whole_frame(&raw, &params, spec.display()),
    );
    assert!(excess_over_storage(&product, &reference.pixels) <= MATH_EXCESS_TOLERANCE);

    for stage in IspStage::ALL {
        let stages = IspStageSet::ALL.without(stage);
        let request = RunRequest {
            stages,
            ..RunRequest::whole_frame(&raw, &params, spec.display())
        };
        let run = run_viewfinder(&context, &config, &request);
        let (expected, _) = emulate(&raw, &params, stages);
        let excess = excess_over_storage(&run, &expected);
        eprintln!("без {stage:?}: сверх шага fp16 = {excess:.2e}");
        assert!(excess <= MATH_EXCESS_TOLERANCE, "без {stage:?}: {excess}");
        match stage {
            IspStage::AwbStatistics => {
                assert_eq!(run.pixels, product.pixels, "статистика не влияет на картинку");
                assert_eq!(run.report.awb_statistics, None, "выключенная статистика не отдаётся");
            }
            // У ClipToZero и неотрицательного света вход — тождество: стадия «вход» картинку не меняет.
            IspStage::GamutCompression => {
                assert_eq!(
                    run.pixels, product.pixels,
                    "при неотрицательном свете вход AgX — тождество"
                );
            }
            _ => assert_ne!(
                run.pixels, product.pixels,
                "без {stage:?} картинка AgX обязана измениться"
            ),
        }
    }
    assert_no_validation_errors(&context);
}

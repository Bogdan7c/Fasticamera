//! Выключатели стадий (specialization-константы) на GPU: каждый вариант собирается и работает,
//! выключенная стадия меняет картинку ровно так, как записано в `viewfinder::stages` (сверка с
//! эмуляцией на CPU из функций `isp-model`), все стадии включены = паритетный выход.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть способ сообщить о провале"
)]

mod lens_shading_support;
mod viewfinder_support;

use camera_contract::geometry::{PixelSize, SensorOrientation};
use isp_model::cfa::{CfaCell, RgbChannel};
use isp_model::image::{Mosaic, ShadingCorrectedMosaic, mosaic_stage};
use isp_model::lens_shading::{LensShadingSampling, apply_lens_shading_with};
use isp_model::levels::normalize_sample;
use isp_model::params::IspParams;
use isp_model::test_patterns::color_bars;
use isp_model::transfer::srgb_encode;
use isp_model::white_balance::SENSOR_CLIP_LEVEL;
use isp_vulkan::viewfinder::{IspStage, IspStageSet, ViewfinderStorageFormat};
use viewfinder_support::{
    ParamsSpec, RawFrame, RunRequest, ViewfinderRun, assert_no_validation_errors, gpu_context_or_skip,
    reference_upright, run_viewfinder, storage_step, test_config,
};

const WIDTH: u32 = 48;
const HEIGHT: u32 = 32;
/// Допуск ошибки вычислений сверх шага хранения fp16 (обоснование — `viewfinder_parity.rs`).
const MATH_EXCESS_TOLERANCE: f32 = 1.0e-5;

/// Усиления виньетирования на каждый отсчёт кадра (видоискатель — по центру ячейки): эталон применяется к
/// мозаике из единиц, и результат `1,0 · gain` — это точно `gain`.
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

/// Эмуляция прохода на CPU с семантикой выключенных стадий (таблица в `viewfinder::stages`).
/// Все стадии включены — ровно `isp_model::pipeline::run_reference` (проверяется отдельно).
fn emulate(raw: &RawFrame, params: &IspParams, stages: IspStageSet) -> Vec<[f32; 3]> {
    let on = |stage| stages.contains(stage);
    let shading = shading_gains(raw, params);
    let gains = params.white_balance.as_array();
    let mut pixels = Vec::new();
    for cell_y in 0..raw.height / 2 {
        for cell_x in 0..raw.width / 2 {
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
                balanced[position] = if on(IspStage::WhiteBalance) {
                    (shaded * gains[channel.index()]).min(SENSOR_CLIP_LEVEL)
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
                params
                    .color
                    .xyz_d50_to_scene()
                    .apply(params.color.camera_to_xyz_d50().apply(camera_rgb))
            } else {
                camera_rgb
            };
            let in_gamut = if on(IspStage::GamutCompression) {
                params.gamut_compression.compress_pixel(scene)
            } else {
                scene
            };
            let display = if on(IspStage::Tone) {
                params.tone.map_pixel(in_gamut)
            } else {
                in_gamut
            };
            pixels.push(if on(IspStage::Oetf) {
                display.map(srgb_encode)
            } else {
                display
            });
        }
    }
    pixels
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
fn each_stage_off_variant_runs_and_changes_output_as_documented() {
    let Some(context) = gpu_context_or_skip() else { return };
    let spec = ParamsSpec::default();
    // Карта виньетирования в параметрах: иначе стадия «виньетирование» была бы пропущена решением кадра, а не набором.
    let params = lens_shading_support::with_map(&spec.build(), &lens_shading_support::asymmetric_map(5, 4));
    // Цветные полосы с пересветом в нижних строках: каждая стадия здесь что-то делает.
    let bars = color_bars(PixelSize::new(WIDTH, HEIGHT), 1.0).unwrap();
    let raw = RawFrame::from_linear(WIDTH, HEIGHT, spec.pattern, spec.levels(), |x, y, channel| {
        f64::from(bars.pixel(x, y)[channel]) * (0.05 + 1.3 * f64::from(y) / f64::from(HEIGHT - 1))
    });
    let config = test_config(ViewfinderStorageFormat::HalfFloatOnly);

    // Эмуляция «всё включено» = эталон (иначе сверка вариантов ничего бы не доказывала).
    let reference = reference_upright(&raw, raw.full_rect(), SensorOrientation::Degrees0, &params);
    assert_eq!(emulate(&raw, &params, IspStageSet::ALL), reference.pixels);

    let product = run_viewfinder(
        &context,
        &config,
        &RunRequest::whole_frame(&raw, &params, spec.display()),
    );
    assert!(excess_over_storage(&product, &reference.pixels) <= MATH_EXCESS_TOLERANCE);
    assert!(product.report.awb_statistics.is_some());

    for stage in IspStage::ALL {
        let stages = IspStageSet::ALL.without(stage);
        let request = RunRequest {
            stages,
            ..RunRequest::whole_frame(&raw, &params, spec.display())
        };
        let run = run_viewfinder(&context, &config, &request);
        let excess = excess_over_storage(&run, &emulate(&raw, &params, stages));
        eprintln!("без {stage:?}: сверх шага fp16 = {excess:.2e}");
        assert!(excess <= MATH_EXCESS_TOLERANCE, "без {stage:?}: {excess}");
        if stage == IspStage::AwbStatistics {
            assert_eq!(run.pixels, product.pixels, "статистика не влияет на картинку");
            assert_eq!(run.report.awb_statistics, None, "выключенная статистика не отдаётся");
        } else if matches!(stage, IspStage::LookColor | IspStage::LookLut) {
            // Вид нейтрален (ручки в нуле, таблицы нет): стадиям вида резать нечего — картинка не меняется.
            assert_eq!(run.pixels, product.pixels, "при нейтральном виде {stage:?} пуста");
            assert!(run.report.awb_statistics.is_some());
        } else if stage == IspStage::OutputGamutCompression {
            // Стадия принадлежит основе AgX: у прежней основы её нет в проходе, картинка не меняется.
            assert_eq!(run.pixels, product.pixels, "у прежней основы {stage:?} пуста");
            assert!(run.report.awb_statistics.is_some());
        } else {
            assert_ne!(run.pixels, product.pixels, "без {stage:?} картинка обязана измениться");
            assert!(run.report.awb_statistics.is_some());
        }
    }
    assert_no_validation_errors(&context);
}

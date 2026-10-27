//! Каждая ручка `IspParams` отдельно против эталона: баланс белого, экспозиция и тон, гамма выхода,
//! уровни по позициям CFA, все 4 раскладки, вход RAW10 (строка с мусорным хвостом) против RAW16.
//!
//! Для каждой ручки два свойства: (1) GPU совпадает с эталоном в допуске паритета (как в
//! `viewfinder_parity.rs`); (2) смена ручки МЕНЯЕТ картинку GPU — значит, значение действительно
//! доходит до шейдера, а не совпадает с эталоном случайно.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть способ сообщить о провале"
)]

mod viewfinder_support;

use camera_contract::geometry::PixelSize;
use camera_contract::geometry::SensorOrientation;
use gpu_vulkan::context::GpuContext;
use isp_model::cfa::BayerPattern;
use isp_model::image::DisplayGamut;
use isp_model::test_patterns::color_bars;
use isp_model::tone::ToneCurveSettings;
use isp_vulkan::viewfinder::ViewfinderStorageFormat;
use viewfinder_support::{
    DAYLIGHT, LAMP, ParamsSpec, RED_BRIGHTEST, RawFrame, RunRequest, SourceLayout, ViewfinderRun,
    assert_no_validation_errors, gpu_context_or_skip, parity, reference_upright, run_viewfinder, test_config,
};

/// Допуск: полшага 10 бит + ошибка вычислений (обоснование — `viewfinder_parity.rs`).
const TOLERANCE: f32 = 0.5 / 1023.0 + 1.0e-5;
const WIDTH: u32 = 56;
const HEIGHT: u32 = 40;

/// Сцена: цветные полосы, яркость растёт сверху вниз; уровни — из спецификации параметров.
fn scene(spec: &ParamsSpec) -> RawFrame {
    let bars = color_bars(PixelSize::new(WIDTH, HEIGHT), 1.0).unwrap();
    RawFrame::from_linear(WIDTH, HEIGHT, spec.pattern, spec.levels(), |x, y, channel| {
        let row_level = 0.05 + 0.9 * f64::from(y) / f64::from(HEIGHT - 1);
        f64::from(bars.pixel(x, y)[channel]) * row_level
    })
}

/// Прогон на GPU (fp16-выход) + проверка паритета; возвращает картинку GPU.
fn checked_run(context: &GpuContext, what: &str, spec: &ParamsSpec, raw: &RawFrame) -> ViewfinderRun {
    let params = spec.build();
    let request = RunRequest::whole_frame(raw, &params, spec.display());
    let run = run_viewfinder(context, &test_config(ViewfinderStorageFormat::HalfFloatOnly), &request);
    let reference = reference_upright(raw, raw.full_rect(), SensorOrientation::Degrees0, &params);
    let result = parity(&run, &reference);
    eprintln!(
        "{what}: max |Δ| = {:.3e}, сверх шага = {:.2e}",
        result.max_abs, result.math_excess
    );
    assert!(result.max_abs <= TOLERANCE, "{what}: {result:?}");
    assert_eq!(result.over_one_lsb, 0.0, "{what}");
    run
}

/// Средняя разница двух картинок GPU по каналам (доказывает, что ручка дошла до шейдера).
fn mean_difference(first: &ViewfinderRun, second: &ViewfinderRun) -> f32 {
    let total: f32 = first
        .pixels
        .iter()
        .zip(&second.pixels)
        .map(|(left, right)| {
            (0..3)
                .map(|channel| (left[channel] - right[channel]).abs())
                .sum::<f32>()
        })
        .sum();
    total / (first.pixels.len() * 3) as f32
}

#[test]
fn white_balance_neutrals_match_reference_and_change_the_picture() {
    let Some(context) = gpu_context_or_skip() else { return };
    let base = ParamsSpec::default();
    let raw = scene(&base);
    let runs: Vec<ViewfinderRun> = [LAMP, DAYLIGHT, RED_BRIGHTEST]
        .into_iter()
        .map(|neutral| {
            let spec = ParamsSpec { neutral, ..base };
            checked_run(&context, &format!("нейтраль {neutral:?}"), &spec, &raw)
        })
        .collect();
    assert!(mean_difference(&runs[0], &runs[1]) > 0.01, "лампа ≠ день");
    assert!(mean_difference(&runs[0], &runs[2]) > 0.01, "лампа ≠ тёплый свет");
    assert_no_validation_errors(&context);
}

#[test]
fn exposure_contrast_and_hue_preservation_match_reference() {
    let Some(context) = gpu_context_or_skip() else { return };
    let base = ParamsSpec::default();
    let raw = scene(&base);
    let default_run = checked_run(&context, "тон по умолчанию", &base, &raw);
    let variants = [
        (
            "экспозиция: средний серый сцены 0,15",
            ToneCurveSettings {
                scene_middle_gray: 0.15,
                ..ToneCurveSettings::default()
            },
        ),
        (
            "контраст 1,8",
            ToneCurveSettings {
                contrast: 1.8,
                ..ToneCurveSettings::default()
            },
        ),
        (
            "сохранение оттенка 0",
            ToneCurveSettings {
                hue_preservation: 0.0,
                ..ToneCurveSettings::default()
            },
        ),
        (
            "сохранение оттенка 0,5",
            ToneCurveSettings {
                hue_preservation: 0.5,
                ..ToneCurveSettings::default()
            },
        ),
    ];
    for (what, tone) in variants {
        let spec = ParamsSpec { tone, ..base };
        let run = checked_run(&context, what, &spec, &raw);
        assert!(
            mean_difference(&default_run, &run) > 1.0e-3,
            "{what}: картинка не изменилась"
        );
    }
    assert_no_validation_errors(&context);
}

#[test]
fn srgb_and_display_p3_outputs_match_reference_and_differ() {
    let Some(context) = gpu_context_or_skip() else { return };
    let base = ParamsSpec::default();
    let raw = scene(&base);
    let p3 = checked_run(&context, "Display P3", &base, &raw);
    let srgb = checked_run(
        &context,
        "sRGB",
        &ParamsSpec {
            gamut: DisplayGamut::Srgb,
            ..base
        },
        &raw,
    );
    // Насыщенные полосы в P3 записываются меньшими значениями, чем в sRGB (охват шире).
    assert!(mean_difference(&p3, &srgb) > 5.0e-3);
    assert_no_validation_errors(&context);
}

#[test]
fn per_position_black_levels_and_white_level_match_reference() {
    let Some(context) = gpu_context_or_skip() else { return };
    let spec = ParamsSpec {
        black: [60.0, 64.0, 68.0, 72.0],
        white: 1000.0,
        ..ParamsSpec::default()
    };
    let raw = scene(&spec);
    let distinct = checked_run(&context, "чёрный по позициям 60/64/68/72, белый 1000", &spec, &raw);
    // Те же RAW, но параметры с общим чёрным 64: картинка обязана отличаться (позиции не перепутаны).
    let uniform = checked_run(
        &context,
        "общий чёрный 64",
        &ParamsSpec {
            black: [64.0; 4],
            ..spec
        },
        &raw,
    );
    assert!(mean_difference(&distinct, &uniform) > 1.0e-3);
    assert_no_validation_errors(&context);
}

#[test]
fn all_four_cfa_layouts_match_reference() {
    let Some(context) = gpu_context_or_skip() else { return };
    for pattern in [
        BayerPattern::RGGB,
        BayerPattern::GRBG,
        BayerPattern::GBRG,
        BayerPattern::BGGR,
    ] {
        let spec = ParamsSpec {
            pattern,
            ..ParamsSpec::default()
        };
        checked_run(&context, &format!("{pattern:?}"), &spec, &scene(&spec));
    }
    assert_no_validation_errors(&context);
}

#[test]
fn raw10_with_padded_stride_and_raw16_give_identical_output() {
    let Some(context) = gpu_context_or_skip() else { return };
    let spec = ParamsSpec::default();
    let params = spec.build();
    let raw = scene(&spec);
    let config = test_config(ViewfinderStorageFormat::HalfFloatOnly);
    let raw16 = run_viewfinder(
        &context,
        &config,
        &RunRequest::whole_frame(&raw, &params, spec.display()),
    );
    for padding_bytes in [0, 12] {
        let raw10 = run_viewfinder(
            &context,
            &config,
            &RunRequest {
                layout: SourceLayout::Raw10 { padding_bytes },
                ..RunRequest::whole_frame(&raw, &params, spec.display())
            },
        );
        assert_eq!(
            raw10.pixels, raw16.pixels,
            "RAW10 (хвост строки {padding_bytes} байт) ≠ RAW16"
        );
        assert_eq!(
            raw10.report.awb_statistics, raw16.report.awb_statistics,
            "статистика тоже одинакова"
        );
    }
    assert_no_validation_errors(&context);
}

//! Паритет ISP видоискателя на GPU (RADV, полная validation) с эталоном `isp-model` (Preview).
//!
//! Сцены: синтетика (серая шкала, цветные полосы, пересветы, около чёрного) и 4 реальных кропа
//! 48×48 кадра C7 «лампа». Сравнение — закодированный сигнал экрана, пиксель в пиксель.
//!
//! # Допуски и откуда расхождения
//! * fp16-выход (`HalfFloatOnly`) показывает МАТЕМАТИКУ. У fp16 11 бит мантиссы: шаг у значений
//!   0,5…1 — 4,9e-4, и RADV при записи в картинку вправе округлять к нулю, поэтому |Δ| до одного
//!   шага fp16 объясняется форматом. Остаток сверх шага (`math_excess`) — ошибка вычислений GPU:
//!   `pow` (exp2·log2, точность Vulkan наследуется), деление (2,5 ULP), FMA в матрицах. Замер на RADV:
//!   ≤ 1,2e-7 (1 ULP f32) на всех сценах. Допуск `MATH_EXCESS_TOLERANCE` = 1e-5 (в 80 раз меньше 1 LSB),
//!   плюс max |Δ| ≤ 0,5 LSB и НОЛЬ пикселей хуже 1 LSB.
//! * 10-бит выход (`PreferTenBitUnorm`) — то же плюс квантование UNORM (округление к ближайшему,
//!   полшага 1/2046). Замер: max |Δ| = 0,500 LSB, остаток сверх полшага 0. Допуск: max |Δ| ≤ 0,5 LSB
//!   плюс `MATH_EXCESS_TOLERANCE`.
//!
//! # Решение по fp16-арифметике (замер, не догадка)
//! Опробованы два варианта шейдера (временно, в продукт не вошли): хвост «охват, сохранение оттенка
//! и OETF» в `float16_t` (pow тона в f32) — цветные полосы 2,46 LSB, 20 % пикселей хуже 1 LSB: допуск
//! сломан; только OETF в fp16 — max 0,82 LSB, остаток сверх шага fp16 до 3,3e-4 против 1,2e-7 у f32:
//! съедает 2/3 запаса до 1 LSB ради одной `pow` на канал (трансцендентная функция, на Adreno её
//! считает отдельный блок, выигрыш fp16-ALU там мал). Решение: весь проход в f32.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть способ сообщить о провале"
)]

mod viewfinder_support;

use camera_contract::geometry::SensorOrientation;
use isp_model::demosaic::DemosaicMethod;
use isp_model::image::DisplayGamut;
use isp_model::params::{FrameInputs, IspParams, ProcessingControls};
use isp_vulkan::viewfinder::{OutputAllocation, ViewfinderGeometry, ViewfinderStorageFormat};
use viewfinder_support::{
    HEIGHT_OF_SYNTHETIC_SCENES as HEIGHT, MATH_EXCESS_TOLERANCE, ParamsSpec, RawFrame, RunRequest,
    WIDTH_OF_SYNTHETIC_SCENES as WIDTH, assert_no_validation_errors, assert_viewfinder_parity, gpu_context_or_skip,
    isp_support, parity, reference_upright, run_viewfinder, synthetic_scenes, test_config,
};

#[test]
fn synthetic_scenes_match_reference_within_tolerance() {
    let Some(context) = gpu_context_or_skip() else { return };
    let spec = ParamsSpec::default();
    let params = spec.build();
    for (name, raw) in synthetic_scenes(&spec) {
        assert_viewfinder_parity(&context, name, &raw, &params, &spec);
    }
    assert_no_validation_errors(&context);
}

#[test]
fn real_crops_match_reference_within_tolerance() {
    let Some(context) = gpu_context_or_skip() else { return };
    for crop in ["gray-tile", "towel", "jar", "tap-highlight"] {
        let capture =
            isp_support::real_capture::load_real_capture(&isp_support::assets_dir(), &format!("real-main-lamp-{crop}"));
        let frame = FrameInputs {
            levels: capture.sensor.static_levels(),
        };
        let controls =
            ProcessingControls::with_defaults(capture.neutral, DemosaicMethod::Preview, DisplayGamut::DisplayP3)
                .with_legacy_tone();
        let params = IspParams::resolve(&capture.sensor, &frame, &controls).unwrap();
        let raw = RawFrame {
            width: capture.raw.size().width,
            height: capture.raw.size().height,
            pattern: capture.raw.pattern(),
            samples: capture.raw.samples().to_vec(),
        };
        let spec = ParamsSpec {
            gamut: DisplayGamut::DisplayP3,
            ..ParamsSpec::default()
        };
        assert_viewfinder_parity(&context, crop, &raw, &params, &spec);
    }
    assert_no_validation_errors(&context);
}

#[test]
fn rotated_output_matches_rotated_reference() {
    let Some(context) = gpu_context_or_skip() else { return };
    let spec = ParamsSpec::default();
    let params = spec.build();
    let (_, raw) = synthetic_scenes(&spec).swap_remove(1);
    for orientation in [SensorOrientation::Degrees90, SensorOrientation::Degrees270] {
        let geometry = ViewfinderGeometry {
            valid_area: raw.full_rect(),
            crop: raw.full_rect(),
            sensor_orientation: orientation,
            allocation: OutputAllocation::Exact,
        };
        let request = RunRequest {
            geometry,
            ..RunRequest::whole_frame(&raw, &params, spec.display())
        };
        let run = run_viewfinder(&context, &test_config(ViewfinderStorageFormat::HalfFloatOnly), &request);
        assert_eq!(
            (run.width, run.height),
            (HEIGHT / 2, WIDTH / 2),
            "{orientation:?}: стороны поменялись"
        );
        let reference = reference_upright(&raw, raw.full_rect(), orientation, &params);
        let result = parity(&run, &reference);
        assert!(
            result.math_excess <= MATH_EXCESS_TOLERANCE,
            "{orientation:?}: {result:?}"
        );
    }
    assert_no_validation_errors(&context);
}

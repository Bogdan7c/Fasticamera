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

use camera_contract::geometry::PixelSize;
use camera_contract::geometry::SensorOrientation;
use gpu_vulkan::context::GpuContext;
use gpu_vulkan::storage_image::StorageImageFormat;
use isp_model::demosaic::DemosaicMethod;
use isp_model::image::DisplayGamut;
use isp_model::params::{FrameInputs, IspParams, LensShadingMapInput, ProcessingControls};
use isp_model::test_patterns::color_bars;
use isp_vulkan::viewfinder::{ViewfinderGeometry, ViewfinderStorageFormat};
use viewfinder_support::{
    ONE_LSB, ParamsSpec, RawFrame, RunRequest, assert_no_validation_errors, gpu_context_or_skip, isp_support, parity,
    reference_upright, run_viewfinder, test_config,
};

/// Допуск ошибки вычислений GPU сверх шага хранения (см. модуль).
const MATH_EXCESS_TOLERANCE: f32 = 1.0e-5;
/// Полшага 10-битного выхода.
const HALF_LSB: f32 = 0.5 / 1023.0;

const WIDTH: u32 = 64;
const HEIGHT: u32 = 48;

/// Сцены синтетики: имя и кадр.
fn synthetic_scenes(spec: &ParamsSpec) -> Vec<(&'static str, RawFrame)> {
    let levels = spec.levels();
    let neutral = spec.neutral;
    let brightest = neutral.iter().copied().fold(f64::MIN, f64::max);
    // Серая шкала: серый объект под нейтралью, яркость 0…1,1 слева направо (правый край — пересвет).
    let gray_ramp = RawFrame::from_linear(WIDTH, HEIGHT, spec.pattern, levels, |x, _, channel| {
        f64::from(x) / f64::from(WIDTH - 1) * 1.1 * neutral[channel] / brightest
    });
    // Цветные полосы `isp-model` (7 цветов) на уровне 0,6, строки — разная яркость 0,2…1.
    let bars = color_bars(PixelSize::new(WIDTH, HEIGHT), 0.6).unwrap();
    let color_bar_frame = RawFrame::from_linear(WIDTH, HEIGHT, spec.pattern, levels, |x, y, channel| {
        let row_level = 0.2 + 0.8 * f64::from(y) / f64::from(HEIGHT - 1);
        f64::from(bars.pixel(x, y)[channel]) * row_level / 0.6
    });
    // Пересветы: полностью насыщенные ячейки, насыщен один канал, сильно за белым у красного.
    let highlights = RawFrame::from_linear(WIDTH, HEIGHT, spec.pattern, levels, |x, y, channel| {
        let band = x * 4 / WIDTH;
        match (band, channel) {
            (0, _) => 1.0,
            (1, 0) => 1.0,
            (1, _) => 0.4 + 0.01 * f64::from(y),
            (2, 2) => 0.98,
            (2, _) => 0.3,
            _ => 0.9 * neutral[channel] / brightest,
        }
    });
    // Около чёрного: значения от black−3 до black+20 (ниже чёрного обрезаются уровнями).
    let near_black = RawFrame::from_fn(WIDTH, HEIGHT, spec.pattern, |x, y, channel| {
        let base = 61 + ((x + 3 * y + channel as u32) % 24);
        base as u16
    });
    vec![
        ("серая шкала", gray_ramp),
        ("цветные полосы", color_bar_frame),
        ("пересветы", highlights),
        ("около чёрного", near_black),
    ]
}

/// Прогоняет кадр в обоих форматах и проверяет допуски; возвращает (fp16, 10 бит) max |Δ|.
fn assert_parity(
    context: &GpuContext,
    name: &str,
    raw: &RawFrame,
    params: &IspParams,
    spec: &ParamsSpec,
) -> (f32, f32) {
    let request = RunRequest::whole_frame(raw, params, spec.display());
    let reference = reference_upright(raw, raw.full_rect(), SensorOrientation::Degrees0, params);

    let half = run_viewfinder(context, &test_config(ViewfinderStorageFormat::HalfFloatOnly), &request);
    assert_eq!(half.format, StorageImageFormat::Rgba16Float);
    let half_parity = parity(&half, &reference);
    eprintln!(
        "{name} fp16: max |Δ| = {:.3e} ({:.3} LSB), доля > 1 LSB = {:.4}, сверх шага fp16 = {:.2e}",
        half_parity.max_abs,
        half_parity.max_abs / ONE_LSB,
        half_parity.over_one_lsb,
        half_parity.math_excess
    );
    assert!(
        half_parity.math_excess <= MATH_EXCESS_TOLERANCE,
        "{name} fp16: {half_parity:?}"
    );
    assert!(
        half_parity.max_abs <= HALF_LSB + MATH_EXCESS_TOLERANCE,
        "{name} fp16: {half_parity:?}"
    );
    assert_eq!(half_parity.over_one_lsb, 0.0, "{name} fp16: {half_parity:?}");

    let ten_bit = run_viewfinder(
        context,
        &test_config(ViewfinderStorageFormat::PreferTenBitUnorm),
        &request,
    );
    let ten_bit_parity = parity(&ten_bit, &reference);
    eprintln!(
        "{name} {:?}: max |Δ| = {:.3e} ({:.3} LSB), доля > 1 LSB = {:.4}, сверх полшага = {:.2e}",
        ten_bit.format,
        ten_bit_parity.max_abs,
        ten_bit_parity.max_abs / ONE_LSB,
        ten_bit_parity.over_one_lsb,
        ten_bit_parity.math_excess
    );
    // Квантование 10 бит — полшага (0,5 LSB) + математика; больше — ошибка формулы.
    assert_eq!(
        ten_bit.format,
        StorageImageFormat::A2B10G10R10Unorm,
        "RADV пишет rgb10_a2"
    );
    assert!(
        ten_bit_parity.math_excess <= MATH_EXCESS_TOLERANCE,
        "{name} 10 бит: {ten_bit_parity:?}"
    );
    assert!(
        ten_bit_parity.max_abs <= HALF_LSB + MATH_EXCESS_TOLERANCE,
        "{name} 10 бит: {ten_bit_parity:?}"
    );
    (half_parity.max_abs, ten_bit_parity.max_abs)
}

#[test]
fn synthetic_scenes_match_reference_within_tolerance() {
    let Some(context) = gpu_context_or_skip() else { return };
    let spec = ParamsSpec::default();
    let params = spec.build();
    for (name, raw) in synthetic_scenes(&spec) {
        assert_parity(&context, name, &raw, &params, &spec);
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
            lens_shading_map: LensShadingMapInput::NotProvided,
        };
        let controls =
            ProcessingControls::with_defaults(capture.neutral, DemosaicMethod::Preview, DisplayGamut::DisplayP3);
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
        assert_parity(&context, crop, &raw, &params, &spec);
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

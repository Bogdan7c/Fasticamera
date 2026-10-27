//! Ориентация и кадрирование ISP видоискателя на GPU.
//!
//! * Метка в левом верхнем углу кадра СЕНСОРА попадает в ожидаемый угол прямой картинки для всех
//!   четырёх ориентаций (ожидание записано явно, а не той же формулой, что в шейдере).
//! * Кадрирование (соотношение + зум) выбирает ожидаемую область: совпадение с эталоном именно по
//!   этой области на пространственно меняющейся сцене.
//! * Полезная область 16:9 с мусором ниже: мусор (пересвет) не попадает ни в картинку, ни в статистику.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть способ сообщить о провале"
)]

mod viewfinder_support;

use camera_contract::geometry::{PixelRect, PixelSize, SensorOrientation};
use camera_contract::viewfinder_crop::{CropAspect, ViewfinderCrop, ZoomFactor};
use isp_model::test_patterns::color_bars;
use isp_vulkan::viewfinder::{ViewfinderGeometry, ViewfinderStorageFormat};
use viewfinder_support::{
    ParamsSpec, RawFrame, RunRequest, SourceLayout, assert_no_validation_errors, gpu_context_or_skip, parity,
    reference_upright, run_viewfinder, test_config,
};

const TOLERANCE: f32 = 0.5 / 1023.0 + 1.0e-5;

/// Красный ли пиксель (метка): красный канал заметно выше зелёного и синего. Тон и сжатие охвата
/// приглушают насыщенный красный (на выходе ≈ (0,99; 0,54; 0,56)), серый фон — каналы почти равны.
fn is_red(pixel: [f32; 4]) -> bool {
    pixel[0] - pixel[1].max(pixel[2]) > 0.25
}

#[test]
fn sensor_corner_marker_lands_in_the_expected_upright_corner_for_every_orientation() {
    let Some(context) = gpu_context_or_skip() else { return };
    let spec = ParamsSpec::default();
    let params = spec.build();
    let (width, height) = (32_u32, 24_u32);
    // Серый фон, метка — красная ячейка 2×2 в левом верхнем углу сенсора.
    let raw = RawFrame::from_linear(width, height, spec.pattern, spec.levels(), |x, y, channel| {
        if x < 2 && y < 2 {
            [0.9, 0.05, 0.05][channel]
        } else {
            0.2 * spec.neutral[channel]
        }
    });
    // Поворот по часовой: левый верхний угол сенсора уходит
    // 0° → левый верх, 90° → правый верх, 180° → правый низ, 270° → левый низ.
    let expectations = [
        (SensorOrientation::Degrees0, "левый верхний"),
        (SensorOrientation::Degrees90, "правый верхний"),
        (SensorOrientation::Degrees180, "правый нижний"),
        (SensorOrientation::Degrees270, "левый нижний"),
    ];
    for (orientation, expected_corner) in expectations {
        let request = RunRequest {
            geometry: ViewfinderGeometry {
                valid_area: raw.full_rect(),
                crop: raw.full_rect(),
                sensor_orientation: orientation,
            },
            ..RunRequest::whole_frame(&raw, &params, spec.display())
        };
        let run = run_viewfinder(&context, &test_config(ViewfinderStorageFormat::HalfFloatOnly), &request);
        let (right, bottom) = (run.width - 1, run.height - 1);
        let corners = [
            ("левый верхний", run.pixel(0, 0)),
            ("правый верхний", run.pixel(right, 0)),
            ("правый нижний", run.pixel(right, bottom)),
            ("левый нижний", run.pixel(0, bottom)),
        ];
        for (corner, pixel) in corners {
            assert_eq!(
                is_red(pixel),
                corner == expected_corner,
                "{orientation:?}: угол {corner} = {pixel:?}, метка ожидалась в углу {expected_corner}"
            );
        }
        let red_pixels = run.pixels.iter().filter(|pixel| is_red(**pixel)).count();
        assert_eq!(red_pixels, 1, "{orientation:?}: ровно одна ячейка-метка");
    }
    assert_no_validation_errors(&context);
}

#[test]
fn crop_with_aspect_and_zoom_selects_the_expected_region() {
    let Some(context) = gpu_context_or_skip() else { return };
    let spec = ParamsSpec::default();
    let params = spec.build();
    let (width, height) = (64_u32, 48_u32);
    // Сцена меняется по обеим осям: полосы по x, яркость по y — любая другая область даст другие пиксели.
    let bars = color_bars(PixelSize::new(width, height), 1.0).unwrap();
    let raw = RawFrame::from_linear(width, height, spec.pattern, spec.levels(), |x, y, channel| {
        f64::from(bars.pixel(x, y)[channel]) * (0.1 + 0.8 * f64::from(y) / f64::from(height - 1))
    });
    let cases = [
        (CropAspect::SQUARE, 1.0, (8, 0, 48, 48)),
        (CropAspect::SIXTEEN_BY_NINE, 1.0, (0, 6, 64, 36)),
        (CropAspect::SensorNative, 2.0, (16, 12, 32, 24)),
    ];
    for (aspect, zoom, (left, top, crop_width, crop_height)) in cases {
        let crop = ViewfinderCrop {
            aspect,
            zoom: ZoomFactor::new(zoom).unwrap(),
        };
        let geometry = ViewfinderGeometry::from_crop(raw.full_rect(), &crop, SensorOrientation::Degrees90).unwrap();
        let expected = PixelRect {
            left,
            top,
            size: PixelSize::new(crop_width, crop_height),
        };
        assert_eq!(geometry.crop, expected, "{aspect:?} ×{zoom}");
        let request = RunRequest {
            geometry,
            ..RunRequest::whole_frame(&raw, &params, spec.display())
        };
        let run = run_viewfinder(&context, &test_config(ViewfinderStorageFormat::HalfFloatOnly), &request);
        let reference = reference_upright(&raw, expected, SensorOrientation::Degrees90, &params);
        let result = parity(&run, &reference);
        assert!(result.max_abs <= TOLERANCE, "{aspect:?} ×{zoom}: {result:?}");
    }
    assert_no_validation_errors(&context);
}

#[test]
fn rows_below_a_16_by_9_valid_area_are_never_read() {
    let Some(context) = gpu_context_or_skip() else { return };
    let spec = ParamsSpec::default();
    let params = spec.build();
    let (width, height) = (64_u32, 48_u32);
    let valid_area = PixelRect {
        left: 0,
        top: 0,
        size: PixelSize::new(64, 36),
    };
    // Полезная область — тёмно-серая сцена; ниже — «мусор»: полный пересвет, как незаписанные строки.
    let raw = RawFrame::from_linear(width, height, spec.pattern, spec.levels(), |x, y, channel| {
        if y >= 36 {
            1.0
        } else {
            (0.1 + 0.3 * f64::from(x) / 63.0) * spec.neutral[channel]
        }
    });
    for layout in [SourceLayout::Raw16, SourceLayout::Raw10 { padding_bytes: 8 }] {
        let request = RunRequest {
            layout,
            geometry: ViewfinderGeometry {
                valid_area,
                crop: valid_area,
                sensor_orientation: SensorOrientation::Degrees90,
            },
            ..RunRequest::whole_frame(&raw, &params, spec.display())
        };
        let run = run_viewfinder(&context, &test_config(ViewfinderStorageFormat::HalfFloatOnly), &request);
        let reference = reference_upright(&raw, valid_area, SensorOrientation::Degrees90, &params);
        let result = parity(&run, &reference);
        assert!(result.max_abs <= TOLERANCE, "{layout:?}: {result:?}");
        let totals = run.report.awb_statistics.as_ref().unwrap().totals();
        assert_eq!(
            totals.saturated_cells, 0,
            "{layout:?}: пересвет из-под полезной области попал в статистику"
        );
        assert_eq!(totals.used_cells, 32 * 18, "{layout:?}: ровно ячейки полезной области");
    }
    assert_no_validation_errors(&context);
}

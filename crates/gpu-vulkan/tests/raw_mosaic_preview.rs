//! Паритет предпросмотра RAW-мозаики: шейдер на реальном GPU (RADV, полная validation) против
//! CPU-эталона, пиксель в пиксель (допуск ±1 разряд UNORM8). Каждый случай ломается, если ломается
//! его логика: перепутанная раскладка CFA, забытый чёрный уровень, чтение за полезной областью,
//! ошибка поворота/вписывания, неверная распаковка RAW10.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть способ сообщить о провале; хелперы вне #[test] clippy не считает тестовым кодом"
)]

mod common;

use common::{assert_no_validation_errors, gpu_context_or_skip};
use gpu_vulkan::context::GpuContext;
use gpu_vulkan::error::GpuError;
use gpu_vulkan::offscreen::{OffscreenTarget, Rgba8Image};
use gpu_vulkan::raw_mosaic_preview::reference::{ReferenceRaw, pack_raw10, raw_mosaic_preview_reference};
use gpu_vulkan::raw_mosaic_preview::synthetic::{SyntheticRaw10, SyntheticRaw16};
use gpu_vulkan::raw_mosaic_preview::{
    CfaOrder, FrameHandoff, MosaicPreviewParams, MosaicSource, PreviewFrame, QuarterTurns, RawMosaicPreview, ValidArea,
};
use gpu_vulkan::test_pattern::SurfaceTransform;

const BLACK: u32 = 64;
const WHITE: u32 = 1023;
/// Допуск округления GPU относительно CPU-эталона, разрядов UNORM8.
const TOLERANCE: i16 = 1;

fn params(area: ValidArea, cfa: CfaOrder, rotation: QuarterTurns) -> MosaicPreviewParams {
    MosaicPreviewParams {
        valid_area: area,
        cfa,
        black_levels: [BLACK; 4],
        white_level: WHITE,
        sensor_rotation: rotation,
    }
}

fn full_area(width: u32, height: u32) -> ValidArea {
    ValidArea {
        left: 0,
        top: 0,
        width,
        height,
    }
}

/// Детерминированный кадр, где почти все значения различны, а часть ниже чёрного и выше белого
/// (проверяет зажим 0..1).
fn textured_frame(width: u32, height: u32) -> Vec<u16> {
    (0..height)
        .flat_map(|y| (0..width).map(move |x| ((x * 37 + y * 91 + (x * y) % 53 * 7) % 1200) as u16))
        .collect()
}

fn render(
    context: &GpuContext,
    source: &MosaicSource,
    params: &MosaicPreviewParams,
    transform: SurfaceTransform,
    target_size: (u32, u32),
) -> Rgba8Image {
    let mut preview = RawMosaicPreview::new(context, OffscreenTarget::FORMAT, 2).expect("пайплайны предпросмотра");
    let mut target = OffscreenTarget::new(context, target_size.0, target_size.1).expect("цель");
    target
        .render(|command_buffer, attachment| {
            let frame = PreviewFrame {
                source,
                params,
                transform,
                frame_slot: 0,
                handoff: FrameHandoff::first_read(),
            };
            preview.record(command_buffer, attachment, &frame)
        })
        .expect("рендер предпросмотра");
    target.readback().expect("readback")
}

fn assert_images_close(actual: &Rgba8Image, expected: &Rgba8Image, label: &str) {
    assert_eq!(
        (actual.width, actual.height),
        (expected.width, expected.height),
        "{label}: размер"
    );
    for y in 0..actual.height {
        for x in 0..actual.width {
            let got = actual.pixel(x, y).unwrap();
            let want = expected.pixel(x, y).unwrap();
            assert_eq!(got[3], 255, "{label}: альфа ({x},{y})");
            for channel in 0..3 {
                let difference = (i16::from(got[channel]) - i16::from(want[channel])).abs();
                assert!(
                    difference <= TOLERANCE,
                    "{label}: пиксель ({x},{y}) канал {channel}: GPU {got:?}, эталон {want:?}"
                );
            }
        }
    }
}

fn render_raw16_and_compare(
    context: &GpuContext,
    frame: (u32, u32),
    texels: &[u16],
    params: &MosaicPreviewParams,
    transform: SurfaceTransform,
    target_size: (u32, u32),
    label: &str,
) -> Rgba8Image {
    let source = SyntheticRaw16::new(context, frame.0, frame.1, texels).expect("источник RAW16");
    let actual = render(context, &source.source(), params, transform, target_size);
    let expected = raw_mosaic_preview_reference(
        target_size.0,
        target_size.1,
        transform,
        ReferenceRaw::Raw16 { width: frame.0, texels },
        params,
    );
    assert_images_close(&actual, &expected, label);
    actual
}

#[test]
fn bggr_frame_matches_reference_pixel_for_pixel() {
    let Some(context) = gpu_context_or_skip() else { return };
    let texels = textured_frame(40, 30);
    let image = render_raw16_and_compare(
        &context,
        (40, 30),
        &texels,
        &params(full_area(40, 30), CfaOrder::Bggr, QuarterTurns::None),
        SurfaceTransform::Identity,
        (40, 30),
        "BGGR 1:1",
    );
    // Содержимое не пустое: иначе паритет «чёрное = чёрное» ничего бы не доказывал.
    assert!(
        image.pixels.chunks(4).any(|pixel| pixel[0] > 50 && pixel[2] > 50),
        "картинка почти чёрная"
    );
    assert_no_validation_errors(&context);
}

#[test]
fn every_cfa_layout_follows_its_own_channel_order() {
    let Some(context) = gpu_context_or_skip() else { return };
    let texels = textured_frame(16, 12);
    let mut images = Vec::new();
    for cfa in [CfaOrder::Rggb, CfaOrder::Grbg, CfaOrder::Gbrg, CfaOrder::Bggr] {
        images.push(render_raw16_and_compare(
            &context,
            (16, 12),
            &texels,
            &params(full_area(16, 12), cfa, QuarterTurns::None),
            SurfaceTransform::Identity,
            (16, 12),
            &format!("{cfa:?}"),
        ));
    }
    // Раскладки обязаны давать разные картинки из одних и тех же данных (иначе CFA игнорируется).
    for first in 0..images.len() {
        for second in first + 1..images.len() {
            assert_ne!(
                images[first].pixels, images[second].pixels,
                "раскладки {first} и {second} совпали"
            );
        }
    }
    assert_no_validation_errors(&context);
}

#[test]
fn black_maps_to_zero_white_to_full_and_below_black_is_clamped() {
    let Some(context) = gpu_context_or_skip() else { return };
    // RGGB, одна ячейка: R = белый, G1 = чёрный, G2 = ниже чёрного, B = середина размаха (544 = 64 + 480).
    let texels = [WHITE as u16, BLACK as u16, 10, 544];
    let image = render(
        &context,
        &SyntheticRaw16::new(&context, 2, 2, &texels).unwrap().source(),
        &params(full_area(2, 2), CfaOrder::Rggb, QuarterTurns::None),
        SurfaceTransform::Identity,
        (2, 2),
    );
    let [red, green, blue, alpha] = image.pixel(0, 0).unwrap();
    assert_eq!(
        (red, green, alpha),
        (255, 0, 255),
        "белый -> 255, чёрный и ниже чёрного -> 0"
    );
    assert!(
        (i16::from(blue) - 128).abs() <= 1,
        "середина размаха -> 128, получили {blue}"
    );

    // Среднее двух зелёных: белый и чёрный дают половину.
    let green_mix = [10u16, WHITE as u16, BLACK as u16, 10];
    let mixed = render(
        &context,
        &SyntheticRaw16::new(&context, 2, 2, &green_mix).unwrap().source(),
        &params(full_area(2, 2), CfaOrder::Rggb, QuarterTurns::None),
        SurfaceTransform::Identity,
        (2, 2),
    );
    let [mixed_red, mixed_green, mixed_blue, _] = mixed.pixel(0, 0).unwrap();
    assert_eq!((mixed_red, mixed_blue), (0, 0));
    assert!(
        (i16::from(mixed_green) - 128).abs() <= 1,
        "среднее G = 128, получили {mixed_green}"
    );
    assert_no_validation_errors(&context);
}

#[test]
fn rows_below_the_valid_area_never_reach_the_screen() {
    let Some(context) = gpu_context_or_skip() else { return };
    // Буфер 32x24, настоящие данные только в верхних 16 строках (как 16:9 внутри 4:3), ниже — мусор.
    let (width, height, valid_height) = (32u32, 24u32, 16u32);
    let area = ValidArea {
        left: 0,
        top: 0,
        width,
        height: valid_height,
    };
    let frame_with_garbage = |garbage: u16| -> Vec<u16> {
        let mut texels = textured_frame(width, height);
        for texel in texels.iter_mut().skip((width * valid_height) as usize) {
            *texel = garbage;
        }
        texels
    };
    let render_with = |garbage: u16, label: &str| {
        render_raw16_and_compare(
            &context,
            (width, height),
            &frame_with_garbage(garbage),
            &params(area, CfaOrder::Bggr, QuarterTurns::None),
            SurfaceTransform::Identity,
            (64, 32),
            label,
        )
    };
    let with_white_garbage = render_with(WHITE as u16, "мусор = белый");
    let with_black_garbage = render_with(BLACK as u16, "мусор = чёрный");
    assert_eq!(
        with_white_garbage.pixels, with_black_garbage.pixels,
        "содержимое за полезной областью повлияло на картинку"
    );
    assert_no_validation_errors(&context);
}

#[test]
fn valid_area_with_offset_reads_only_inside_it() {
    let Some(context) = gpu_context_or_skip() else { return };
    let texels = textured_frame(32, 24);
    let area = ValidArea {
        left: 4,
        top: 2,
        width: 20,
        height: 16,
    };
    render_raw16_and_compare(
        &context,
        (32, 24),
        &texels,
        &params(area, CfaOrder::Gbrg, QuarterTurns::None),
        SurfaceTransform::Identity,
        (40, 32),
        "полезная область со смещением",
    );
    assert_no_validation_errors(&context);
}

#[test]
fn sensor_rotation_with_letterbox_matches_reference_and_puts_corner_where_expected() {
    let Some(context) = gpu_context_or_skip() else { return };
    // Кадр 32x16: верхний левый угол сенсора (ячейка 2x2) яркий, остальное — чёрный уровень.
    let (width, height) = (32u32, 16u32);
    let mut texels = vec![BLACK as u16; (width * height) as usize];
    for (x, y) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
        texels[(y * width + x) as usize] = WHITE as u16;
    }
    // RGGB: белая ячейка -> белый пиксель. Сенсор повёрнут на 90° по часовой: угол (0,0) уходит в верхний правый.
    // Цель 40x40: прямая картинка 16x32 вписывается как 20x40 со столбцами letterbox по 10 px.
    let rotated = render_raw16_and_compare(
        &context,
        (width, height),
        &texels,
        &params(full_area(width, height), CfaOrder::Rggb, QuarterTurns::Clockwise90),
        SurfaceTransform::Identity,
        (40, 40),
        "поворот сенсора 90°",
    );
    assert_eq!(
        rotated.pixel(5, 20),
        Some([0, 0, 0, 255]),
        "левая полоса letterbox чёрная"
    );
    assert_eq!(
        rotated.pixel(34, 20),
        Some([0, 0, 0, 255]),
        "правая полоса letterbox чёрная"
    );
    let bright = |x, y| rotated.pixel(x, y).unwrap()[0] > 200;
    assert!(
        bright(29, 1),
        "угол сенсора (0,0) после поворота на 90° — вверху справа"
    );
    assert!(!bright(10, 1) && !bright(29, 38), "прочие углы тёмные");

    // Поворот дисплея поверх сенсорного: проверяем тем же эталоном (все 4 поворота).
    for transform in SurfaceTransform::ROTATIONS {
        render_raw16_and_compare(
            &context,
            (width, height),
            &textured_frame(width, height),
            &params(full_area(width, height), CfaOrder::Bggr, QuarterTurns::Clockwise270),
            transform,
            (48, 36),
            &format!("сенсор 270° + дисплей {transform:?}"),
        );
    }
    assert_no_validation_errors(&context);
}

#[test]
fn raw10_with_padded_rows_matches_reference_and_raw16_with_same_values() {
    let Some(context) = gpu_context_or_skip() else { return };
    let (width, height) = (16u32, 8u32);
    let values: Vec<u16> = textured_frame(width, height)
        .into_iter()
        .map(|value| value & 0x3FF)
        .collect();
    // Строка = 20 полезных байт, шаг 24: 4 байта паддинга забиты 0xFF (если шейдер их прочтёт, картинка изменится).
    let stride = 24;
    let bytes = pack_raw10(&values, width, stride, 0xFF);
    let raw10 = SyntheticRaw10::new(&context, width, height, stride, &bytes).expect("источник RAW10");
    let area = full_area(width, height);
    let cfa_params = params(area, CfaOrder::Bggr, QuarterTurns::Clockwise90);

    let actual = render(
        &context,
        &raw10.source(),
        &cfa_params,
        SurfaceTransform::Identity,
        (32, 32),
    );
    let expected = raw_mosaic_preview_reference(
        32,
        32,
        SurfaceTransform::Identity,
        ReferenceRaw::Raw10 {
            row_stride_bytes: stride,
            bytes: &bytes,
        },
        &cfa_params,
    );
    assert_images_close(&actual, &expected, "RAW10 со сдвигом строки");

    // Независимая проверка распаковки: те же значения через RAW16 дают ту же картинку.
    let raw16 = SyntheticRaw16::new(&context, width, height, &values).expect("источник RAW16");
    let via_raw16 = render(
        &context,
        &raw16.source(),
        &cfa_params,
        SurfaceTransform::Identity,
        (32, 32),
    );
    assert_eq!(
        actual.pixels, via_raw16.pixels,
        "RAW10 и RAW16 с одинаковыми значениями разошлись"
    );
    assert_no_validation_errors(&context);
}

#[test]
fn invalid_requests_are_rejected_before_recording() {
    let Some(context) = gpu_context_or_skip() else { return };
    let texels = textured_frame(8, 8);
    let source = SyntheticRaw16::new(&context, 8, 8, &texels).unwrap();
    let mut preview = RawMosaicPreview::new(&context, OffscreenTarget::FORMAT, 2).unwrap();
    let mut target = OffscreenTarget::new(&context, 8, 8).unwrap();
    let good = params(full_area(8, 8), CfaOrder::Rggb, QuarterTurns::None);

    let mut attempt = |params: &MosaicPreviewParams, transform: SurfaceTransform, slot: usize| {
        target.render(|command_buffer, attachment| {
            let frame = PreviewFrame {
                source: &source.source(),
                params,
                transform,
                frame_slot: slot,
                handoff: FrameHandoff::first_read(),
            };
            preview.record(command_buffer, attachment, &frame)
        })
    };
    let odd_area = params(
        ValidArea {
            left: 1,
            ..full_area(8, 8)
        },
        CfaOrder::Rggb,
        QuarterTurns::None,
    );
    assert!(matches!(
        attempt(&odd_area, SurfaceTransform::Identity, 0),
        Err(GpuError::InvalidArgument { .. })
    ));
    assert!(matches!(
        attempt(&good, SurfaceTransform::HorizontalMirror, 0),
        Err(GpuError::Unsupported { .. })
    ));
    assert!(matches!(
        attempt(&good, SurfaceTransform::Identity, 2),
        Err(GpuError::InvalidArgument { .. })
    ));
    // Второй слот работает так же, как первый.
    attempt(&good, SurfaceTransform::Identity, 1).expect("слот 1");
    assert_no_validation_errors(&context);
}

#[test]
fn redraw_without_ownership_barriers_gives_the_same_picture_and_release_is_recorded_once() {
    let Some(context) = gpu_context_or_skip() else { return };
    let texels = textured_frame(16, 12);
    let source = SyntheticRaw16::new(&context, 16, 12, &texels).unwrap();
    let mosaic = source.source();
    let preview_params = params(full_area(16, 12), CfaOrder::Bggr, QuarterTurns::None);
    let mut preview = RawMosaicPreview::new(&context, OffscreenTarget::FORMAT, 2).unwrap();
    let mut target = OffscreenTarget::new(&context, 16, 12).unwrap();
    let mut draw = |handoff: FrameHandoff, slot: usize| {
        target
            .render(|command_buffer, attachment| {
                let frame = PreviewFrame {
                    source: &mosaic,
                    params: &preview_params,
                    transform: SurfaceTransform::Identity,
                    frame_slot: slot,
                    handoff,
                };
                preview.record(command_buffer, attachment, &frame)
            })
            .expect("рендер");
        target.readback().expect("readback")
    };
    let first = draw(FrameHandoff::first_read(), 0);
    let redraw = draw(FrameHandoff::redraw(), 1);
    assert_eq!(
        first.pixels, redraw.pixels,
        "перерисовка без приёма владения даёт ту же картинку"
    );
    // Замена кадра: возврат прежнего + приём нового в одном сабмите.
    let replaced = draw(
        FrameHandoff {
            release_previous: Some(mosaic),
            ..FrameHandoff::first_read()
        },
        0,
    );
    assert_eq!(first.pixels, replaced.pixels);
    RawMosaicPreview::release_source_blocking(&context, &mosaic).expect("разовый возврат");
    assert_no_validation_errors(&context);
}

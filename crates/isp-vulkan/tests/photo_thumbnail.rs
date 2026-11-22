//! Миниатюра снимка на настоящем GPU (сессия 14g): уменьшение внутри последней полосы проявки совпадает с
//! CPU-эталоном `reference_thumbnail`, применённым к ПОЛНОЙ картинке снимка (её независимое чтение с GPU).
//!
//! # Допуск
//! Размер и ориентация — точно. Значения — не больше `TOLERANCE_LEVELS` градаций sRGB на канал: веса у шейдера и
//! эталона одинаковы (целые), расходятся порядок сумм (дерево нитей группы против прямого прохода) и `pow`
//! видеокарты — единицы ULP f32 → редкое округление в соседнюю градацию.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть способ сообщить о провале"
)]

mod photo_support;
mod quality_support;

use camera_contract::geometry::PixelSize;
use isp_model::cfa::BayerPattern;
use isp_vulkan::photo::{
    DevelopRequest, PhotoConfig, PhotoDeveloper, RawCapturer, RgbaReadback, ThumbnailOutcome, ThumbnailUnavailable,
    reference_thumbnail,
};
use photo_support::{Placement, SourceKind, capture, crop_in_buffer, develop_with, upload, zone_plate_frame};
use quality_support::{assert_no_validation_errors, gpu_context_or_skip};
use still_contract::shot::StillRotation;

const WIDTH: u32 = 64;
const HEIGHT: u32 = 48;
/// Длинная сторона миниатюры в тестах: 64×48 → 16×12 (уменьшение 4:1), нечётный кроп — дробное.
const LONG_SIDE: u32 = 16;
/// Допуск значений, градации sRGB (см. доку файла).
const TOLERANCE_LEVELS: u8 = 1;

fn developer(context: &gpu_vulkan::context::GpuContext) -> PhotoDeveloper {
    let config = PhotoConfig {
        slice_rows: 16,
        thumbnail_long_side_px: LONG_SIDE,
        ..PhotoConfig::default()
    };
    PhotoDeveloper::new(context, &config, WIDTH).unwrap()
}

/// Проявляет кроп и проверяет миниатюру против эталона от картинки снимка. Возвращает размер миниатюры.
fn develop_and_check(
    crop_in_valid: (u32, u32, u32, u32),
    rotation: StillRotation,
    readback: RgbaReadback,
) -> PixelSize {
    let Some(context) = gpu_context_or_skip() else {
        return PixelSize::new(0, 0);
    };
    let mut capturer = RawCapturer::new(&context, &PhotoConfig::default()).unwrap();
    let frame = zone_plate_frame(WIDTH, HEIGHT, BayerPattern::BGGR);
    let gpu_frame = upload(&context, &frame.raw, SourceKind::Raw10, Placement::PADDED);
    let captured = capture(&context, &mut capturer, &gpu_frame);
    let mut developer = developer(&context);
    let (left, top, width, height) = crop_in_valid;
    let crop = crop_in_buffer(gpu_frame.valid_area, left, top, width, height);
    let mut developed = develop_with(
        &context,
        &mut developer,
        &DevelopRequest {
            raw: &captured,
            params: &frame.params,
            crop,
            readback,
            rotation,
        },
        |_, _| {},
    );

    let ThumbnailOutcome::Ready { size, rgba } = developed.photo.take_thumbnail() else {
        panic!("миниатюра обязана быть: {rotation:?}, {crop_in_valid:?}, {readback:?}");
    };
    let (want_size, want) = reference_thumbnail(
        &developed.image_rgba,
        PixelSize::new(width, height),
        rotation,
        LONG_SIDE,
    )
    .unwrap();
    assert_eq!(size, want_size, "размер миниатюры");
    assert_eq!(rgba.len(), want.len());
    let worst = rgba
        .iter()
        .zip(&want)
        .map(|(got, want)| got.abs_diff(*want))
        .max()
        .unwrap();
    assert!(
        worst <= TOLERANCE_LEVELS,
        "миниатюра GPU расходится с эталоном на {worst} градаций ({rotation:?}, {crop_in_valid:?}, {readback:?})"
    );
    // Эталон не пустышка: на зонной пластинке есть и тёмное, и светлое (иначе совпадение ничего не доказывает).
    assert!(
        want.iter().max().unwrap() - want.iter().min().unwrap() > 60,
        "картинка-эталон плоская"
    );
    assert!(rgba.chunks_exact(4).all(|pixel| pixel[3] == 255), "альфа миниатюры");
    // Повторно миниатюру не отдаём.
    assert!(matches!(
        developed.photo.take_thumbnail(),
        ThumbnailOutcome::Unavailable(ThumbnailUnavailable::AlreadyTaken)
    ));
    assert_no_validation_errors(&context);
    size
}

#[test]
fn gpu_thumbnail_matches_the_reference_of_the_full_picture_without_rotation() {
    let size = develop_and_check((0, 0, WIDTH, HEIGHT), StillRotation::None, RgbaReadback::ForJpeg);
    assert!(
        size == PixelSize::new(16, 12) || size == PixelSize::new(0, 0),
        "{size:?}"
    );
}

#[test]
fn gpu_thumbnail_is_rotated_as_the_gallery_shows_it() {
    for rotation in [
        StillRotation::Clockwise90,
        StillRotation::Clockwise180,
        StillRotation::Clockwise270,
    ] {
        let size = develop_and_check((0, 0, WIDTH, HEIGHT), rotation, RgbaReadback::ForJpeg);
        if size != PixelSize::new(0, 0) {
            let swapped = matches!(rotation, StillRotation::Clockwise90 | StillRotation::Clockwise270);
            assert_eq!(
                size,
                if swapped {
                    PixelSize::new(12, 16)
                } else {
                    PixelSize::new(16, 12)
                }
            );
        }
    }
}

#[test]
fn odd_crop_with_a_non_integer_ratio_matches_the_reference_for_both_rotations() {
    // 33×29 → при 90° повёрнутый 29×33: длинная сторона 16, короткая round(29·16/33) = 14; обе оси дробные.
    let upright = develop_and_check((7, 5, 33, 29), StillRotation::None, RgbaReadback::ForJpeg);
    let turned = develop_and_check((7, 5, 33, 29), StillRotation::Clockwise90, RgbaReadback::ForJpeg);
    if upright != PixelSize::new(0, 0) {
        assert_eq!(upright, PixelSize::new(16, 14));
        assert_eq!(turned, PixelSize::new(14, 16));
    }
}

#[test]
fn heif_path_without_host_readback_still_yields_a_thumbnail() {
    // `NotNeeded`: у HEIF копии основной картинки в память нет, миниатюра обязана быть всё равно.
    for rotation in [StillRotation::None, StillRotation::Clockwise90] {
        develop_and_check((0, 0, WIDTH, HEIGHT), rotation, RgbaReadback::NotNeeded);
    }
}

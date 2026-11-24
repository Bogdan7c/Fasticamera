//! Быстрые пути проявления снимка БИТ-В-БИТ равны прежнему пути (сессия 16a, решение 213: фото не меняется ни на бит).
//!
//! «Прежний путь» — [`frozen_config`]: настройки проявителя ДО оптимизаций (замороженный эталон). «Новый путь» —
//! продуктовое умолчание [`PhotoConfig::default`]. Оба проявляют один и тот же RAW, сравниваются ВСЕ байты картинки
//! снимка и ВСЕ байты миниатюры. Тест растёт вместе с оптимизациями: каждая точная правка добавляет в
//! `frozen_config` то, чем прежний путь отличался.
//!
//! Матрица: 4 раскладки CFA × RAW10/RAW16 × модели основы тона (прежняя, AgX, ACES, filmic) × поворот × кроп ×
//! высота полосы; синтетика и реальные кадры.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть способ сообщить о провале"
)]

mod photo_support;
mod quality_support;

use camera_contract::geometry::PixelRect;
use gpu_vulkan::context::GpuContext;
use isp_model::cfa::BayerPattern;
use isp_model::tone::base::ToneBase;
use isp_vulkan::photo::{
    DevelopRequest, PhotoConfig, PhotoDeveloper, RawCapture, RawCapturer, RgbaReadback, ThumbnailOutcome,
};
use isp_vulkan::quality::WorkgroupSizeConfig;
use photo_support::{
    Placement, SourceFrame, SourceKind, capture, crop_in_buffer, develop_with, real_frame, upload, zone_plate_frame,
};
use quality_support::{assert_no_validation_errors, gpu_context_or_skip};
use still_contract::shot::StillRotation;

/// Прежний путь (до оптимизаций сессии 16a): группа проходов 16×16.
fn frozen_config(slice_rows: u32) -> PhotoConfig {
    PhotoConfig {
        slice_rows,
        workgroup_size: WorkgroupSizeConfig { x: 16, y: 16 },
        thumbnail_long_side_px: 16,
        ..PhotoConfig::default()
    }
}

/// Новый путь: продуктовое умолчание (с той же высотой полосы и миниатюрой).
fn new_config(slice_rows: u32) -> PhotoConfig {
    PhotoConfig {
        slice_rows,
        thumbnail_long_side_px: 16,
        ..PhotoConfig::default()
    }
}

/// Результат проявления: байты картинки и миниатюры.
struct Outputs {
    image: Vec<u8>,
    thumbnail: Vec<u8>,
}

fn develop_outputs(
    context: &GpuContext,
    config: &PhotoConfig,
    captured: &RawCapture,
    frame: &SourceFrame,
    crop: PixelRect,
    rotation: StillRotation,
) -> Outputs {
    let mut developer = PhotoDeveloper::new(context, config, 128).unwrap();
    let request = DevelopRequest {
        raw: captured,
        params: &frame.params,
        crop,
        readback: RgbaReadback::NotNeeded,
        rotation,
    };
    let mut developed = develop_with(context, &mut developer, &request, |_, _| {});
    let ThumbnailOutcome::Ready { rgba, .. } = developed.photo.take_thumbnail() else {
        panic!("миниатюра обязана быть");
    };
    Outputs {
        image: developed.image_rgba,
        thumbnail: rgba,
    }
}

/// Сравнивает прежний и новый путь на одном кадре; байты обязаны совпасть ВСЕ.
fn assert_exact(
    context: &GpuContext,
    capturer: &mut RawCapturer,
    frame: &SourceFrame,
    kind: SourceKind,
    crop_in_valid: (u32, u32, u32, u32),
    rotation: StillRotation,
    slice_rows: u32,
) {
    let gpu_frame = upload(context, &frame.raw, kind, Placement::PADDED);
    let captured = capture(context, capturer, &gpu_frame);
    let (left, top, width, height) = crop_in_valid;
    let crop = crop_in_buffer(gpu_frame.valid_area, left, top, width, height);
    let old = develop_outputs(context, &frozen_config(slice_rows), &captured, frame, crop, rotation);
    let new = develop_outputs(context, &new_config(slice_rows), &captured, frame, crop, rotation);
    let label = format!(
        "{} / {kind:?} / {crop_in_valid:?} / {rotation:?} / полоса {slice_rows}",
        frame.name
    );
    assert_eq!(old.image.len(), new.image.len(), "{label}");
    assert!(
        old.image.iter().any(|&byte| byte != old.image[0]),
        "{label}: картинка вырождена, сравнение ничего не доказывает"
    );
    let differing = old.image.iter().zip(&new.image).filter(|(a, b)| a != b).count();
    assert_eq!(differing, 0, "{label}: байтов картинки отличается {differing}");
    assert!(old.thumbnail == new.thumbnail, "{label}: миниатюра отличается");
}

fn tone_models() -> Vec<ToneBase> {
    vec![
        ToneBase::Legacy,
        ToneBase::Agx(isp_model::tone::agx_settings::AgxSettings::default()),
        ToneBase::Aces(Default::default()),
        ToneBase::Filmic(Default::default()),
    ]
}

#[test]
fn every_cfa_source_and_tone_model_is_bit_identical_to_the_frozen_path() {
    let Some(context) = gpu_context_or_skip() else { return };
    let mut capturer = RawCapturer::new(&context, &PhotoConfig::default()).unwrap();
    for pattern in [
        BayerPattern::RGGB,
        BayerPattern::BGGR,
        BayerPattern::GRBG,
        BayerPattern::GBRG,
    ] {
        for base in tone_models() {
            let frame = zone_plate_frame(96, 80, pattern).with_tone_base(base);
            for kind in [SourceKind::Raw10, SourceKind::Raw16] {
                assert_exact(
                    &context,
                    &mut capturer,
                    &frame,
                    kind,
                    (0, 0, 96, 80),
                    StillRotation::None,
                    30,
                );
            }
        }
    }
    assert_no_validation_errors(&context);
}

#[test]
fn rotation_crop_and_slice_height_are_bit_identical_to_the_frozen_path() {
    let Some(context) = gpu_context_or_skip() else { return };
    let mut capturer = RawCapturer::new(&context, &PhotoConfig::default()).unwrap();
    let frame = zone_plate_frame(96, 80, BayerPattern::GRBG);
    for rotation in [
        StillRotation::None,
        StillRotation::Clockwise90,
        StillRotation::Clockwise180,
        StillRotation::Clockwise270,
    ] {
        for slice_rows in [14, 40, 4096] {
            assert_exact(
                &context,
                &mut capturer,
                &frame,
                SourceKind::Raw10,
                (3, 5, 80, 66),
                rotation,
                slice_rows,
            );
        }
    }
    assert_no_validation_errors(&context);
}

#[test]
fn real_frames_are_bit_identical_to_the_frozen_path() {
    let Some(context) = gpu_context_or_skip() else { return };
    let mut capturer = RawCapturer::new(&context, &PhotoConfig::default()).unwrap();
    for name in ["real-main-lamp-jar", "real-main-lamp-tap-highlight"] {
        let frame = real_frame(name);
        let size = frame.raw.size();
        assert_exact(
            &context,
            &mut capturer,
            &frame,
            SourceKind::Raw10,
            (0, 0, size.width.min(128), size.height.min(96)),
            StillRotation::None,
            32,
        );
    }
    assert_no_validation_errors(&context);
}

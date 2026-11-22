//! Кадрирование снимка: проявляется только область `crop` плюс ореол, а результат — РОВНО соответствующая
//! часть проявления целого кадра (бит-в-бит), для любых смещений (в том числе нечётных), на всех краях,
//! при любой высоте полосы и из обоих видов источника (RAW16 и RAW10).

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
use isp_vulkan::photo::{DEMOSAIC_HALO, DevelopRequest, PhotoConfig, RawCapturer, RgbaReadback, max_region_rows};
use photo_support::{
    Placement, SourceKind, capture, compare_rgba, crop_in_buffer, crop_rgba, develop, developer_for, reference_rgba,
    run_reference_for, upload, zone_plate_frame,
};
use quality_support::{assert_no_validation_errors, gpu_context_or_skip};

const WIDTH: u32 = 128;
const HEIGHT: u32 = 96;

/// Кропы в координатах полезной области: (left, top, width, height).
const CROPS: [(u32, u32, u32, u32); 9] = [
    (0, 0, 128, 96),  // весь кадр
    (1, 1, 50, 40),   // нечётное смещение у левого верхнего угла
    (37, 29, 61, 33), // нечётные смещение и размер внутри кадра
    (64, 48, 64, 48), // правый нижний угол
    (0, 40, 128, 10), // горизонтальная лента на всю ширину
    (100, 0, 28, 96), // вертикальная лента у правого края
    (5, 7, 1, 1),     // один пиксель
    (60, 40, 2, 2),   // ячейка CFA внутри кадра, далеко от краёв
    (31, 63, 90, 33), // у нижнего края
];

#[test]
fn every_crop_is_exactly_the_matching_part_of_the_whole_frame() {
    let Some(context) = gpu_context_or_skip() else { return };
    let mut capturer = RawCapturer::new(&context, &PhotoConfig::default()).unwrap();
    let frame = zone_plate_frame(WIDTH, HEIGHT, BayerPattern::GRBG);
    let reference = reference_rgba(&run_reference_for(&frame));
    for kind in [SourceKind::Raw16, SourceKind::Raw10] {
        let gpu_frame = upload(&context, &frame.raw, kind, Placement::PADDED);
        let captured = capture(&context, &mut capturer, &gpu_frame);
        let mut whole_developer = developer_for(&context, 4096, WIDTH);
        let whole = develop(
            &context,
            &mut whole_developer,
            &captured,
            &frame,
            crop_in_buffer(gpu_frame.valid_area, 0, 0, WIDTH, HEIGHT),
            RgbaReadback::NotNeeded,
        );
        for slice_rows in [8, 20, 4096] {
            let mut developer = developer_for(&context, slice_rows, WIDTH);
            for (left, top, width, height) in CROPS {
                let crop = crop_in_buffer(gpu_frame.valid_area, left, top, width, height);
                let developed = develop(
                    &context,
                    &mut developer,
                    &captured,
                    &frame,
                    crop,
                    RgbaReadback::NotNeeded,
                );
                assert_eq!(developed.photo.size(), PixelSize::new(width, height));
                let want = crop_rgba(&whole.image_rgba, WIDTH, left, top, width, height);
                let difference = compare_rgba(&developed.image_rgba, &want, width);
                assert_eq!(
                    difference.differing_bytes,
                    0,
                    "{kind:?} полоса {slice_rows} crop {:?}: часть целого кадра отличается {difference:?}",
                    (left, top, width, height)
                );
                // И против эталона `isp-model` (обрезка эталона), допуск 1 LSB.
                let want_reference = crop_rgba(&reference, WIDTH, left, top, width, height);
                let versus_reference = compare_rgba(&developed.image_rgba, &want_reference, width);
                assert!(
                    versus_reference.max_abs <= 1,
                    "{kind:?} crop {:?}: против эталона {versus_reference:?}",
                    (left, top, width, height)
                );
            }
        }
    }
    assert_no_validation_errors(&context);
}

#[test]
fn interior_crop_develops_only_crop_plus_halo_not_the_whole_frame() {
    let Some(context) = gpu_context_or_skip() else { return };
    let mut capturer = RawCapturer::new(&context, &PhotoConfig::default()).unwrap();
    let frame = zone_plate_frame(WIDTH, HEIGHT, BayerPattern::RGGB);
    let gpu_frame = upload(&context, &frame.raw, SourceKind::Raw16, Placement::PADDED);
    let captured = capture(&context, &mut capturer, &gpu_frame);
    let mut developer = developer_for(&context, 16, WIDTH);
    let (left, top, width, height) = (50, 30, 20, 20);
    let start = developer
        .begin(&DevelopRequest {
            raw: &captured,
            params: &frame.params,
            crop: crop_in_buffer(gpu_frame.valid_area, left, top, width, height),
            readback: RgbaReadback::NotNeeded,
        })
        .unwrap();
    // 20 строк в полосах по 16 → 2 полосы; область по ширине — crop + ореол с двух сторон, не весь кадр (128).
    assert_eq!(start.slice_count, 2);
    assert_eq!(start.image_size, PixelSize::new(width, height));
    assert_eq!(start.widest_region.width, width + 2 * DEMOSAIC_HALO);
    assert!(start.widest_region.width < WIDTH);
    assert!(start.widest_region.height <= max_region_rows(16));
    assert!(developer.is_developing());
    developer.cancel().unwrap();
    assert!(
        !developer.is_developing(),
        "отмена без записанных полос освобождает проявитель"
    );
    assert_no_validation_errors(&context);
}

#[test]
fn plane_memory_for_phone_frame_is_bounded_by_the_slice_not_the_frame() {
    let Some(context) = gpu_context_or_skip() else { return };
    // 4096×3072 при полосе 256 строк: 7 плоскостей f32 по 4096 × 282 (256 + 2·12 ореола + 2 выравнивания).
    let developer = developer_for(&context, 256, 4096);
    assert_eq!(max_region_rows(256), 282);
    assert_eq!(developer.plane_memory_bytes(), 4096 * 282 * 4 * 7);
    println!(
        "плоскости проявителя 4096×3072, полоса 256: {:.1} МиБ (целый кадр — {:.1} МиБ)",
        developer.plane_memory_bytes() as f64 / (1 << 20) as f64,
        (4096_u64 * 3072 * 4 * 7) as f64 / (1 << 20) as f64
    );
    assert!(
        developer.plane_memory_bytes() * 8 < 4096 * 3072 * 4 * 7 * 2,
        "плоскости многократно меньше кадра"
    );
}

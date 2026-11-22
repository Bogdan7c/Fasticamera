//! Сквозное проявление снимка: сырые данные кадра → захват → проявление → RGBA8 против эталона `isp-model`
//! (`run_reference(..).display_encoded`, квантованный тем же правилом, что в шейдере).
//!
//! Допуск — 1 LSB (одна градация 8 бит) и ни градацией больше: матрицы, охват и тон повторяют CPU бит в бит,
//! кроме `pow` (его точность в Vulkan — несколько ULP): сигнал отличается от эталона на ~1e-6, и там, где
//! `сигнал × 255 + 0,5` лежит почти ровно на целом, округление падает на другую сторону — разница ровно
//! 1 LSB (подробнее — `photo::quantize`). Ненулевое число таких пикселей — не ошибка, но оно мало: тест
//! печатает его и требует, чтобы ≥ 99,9 % каналов совпали точно (иначе допуск «1 LSB» прятал бы системный сдвиг).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть способ сообщить о провале"
)]

mod photo_support;
mod quality_support;

use camera_contract::geometry::PixelSize;
use isp_vulkan::photo::{PhotoConfig, RawCapturer, RgbaReadback};
use photo_support::{
    PATTERNS, Placement, SourceFrame, SourceKind, capture, compare_rgba, crop_in_buffer, develop, developer_for,
    real_frame, reference_rgba, run_reference_for, upload, zone_plate_frame,
};
use quality_support::{assert_no_validation_errors, gpu_context_or_skip};

/// Допуск итога против эталона, градации 8 бит.
const MAX_LSB_DIFFERENCE: u8 = 1;
/// Не менее такой доли байтов картинки обязана совпасть ТОЧНО.
const MIN_EXACT_SHARE: f64 = 0.999;

fn assert_matches_reference(name: &str, frame: &SourceFrame, got: &[u8]) {
    let reference = run_reference_for(frame);
    let want = reference_rgba(&reference);
    let width = frame.raw.size().width;
    let difference = compare_rgba(got, &want, width);
    let exact_share = 1.0 - difference.differing_bytes as f64 / got.len() as f64;
    println!(
        "{name}: max |Δ| {} LSB, отличающихся байтов {} из {} ({:.4} % точно), первое {:?}",
        difference.max_abs,
        difference.differing_bytes,
        got.len(),
        exact_share * 100.0,
        difference.first
    );
    assert!(
        difference.max_abs <= MAX_LSB_DIFFERENCE,
        "{name}: max |Δ| {} LSB в {:?}",
        difference.max_abs,
        difference.first
    );
    assert!(
        exact_share >= MIN_EXACT_SHARE,
        "{name}: точно совпало только {exact_share}"
    );
    // Картинка не вырождена: эталон не плоский, иначе совпадение ничего бы не доказало. Альфа — 255.
    let brightest = want
        .chunks_exact(4)
        .flat_map(|pixel| pixel[..3].iter().copied())
        .max()
        .unwrap();
    let darkest = want
        .chunks_exact(4)
        .flat_map(|pixel| pixel[..3].iter().copied())
        .min()
        .unwrap();
    assert!(
        brightest - darkest > 50,
        "{name}: эталон слишком плоский ({darkest}..{brightest})"
    );
    assert!(
        got.chunks_exact(4).all(|pixel| pixel[3] == 255),
        "{name}: альфа должна быть 255"
    );
}

#[test]
fn real_crops_match_reference_through_raw16_and_raw10_in_one_and_many_slices() {
    let Some(context) = gpu_context_or_skip() else { return };
    let mut capturer = RawCapturer::new(&context, &PhotoConfig::default()).unwrap();
    for name in [
        "real-main-lamp-jar",
        "real-main-lamp-towel",
        "real-main-lamp-tap-highlight",
    ] {
        let frame = real_frame(name);
        let size = frame.raw.size();
        for kind in [SourceKind::Raw16, SourceKind::Raw10] {
            let gpu_frame = upload(&context, &frame.raw, kind, Placement::PADDED);
            let captured = capture(&context, &mut capturer, &gpu_frame);
            for slice_rows in [4096, 16] {
                let mut developer = developer_for(&context, slice_rows, size.width);
                let crop = crop_in_buffer(gpu_frame.valid_area, 0, 0, size.width, size.height);
                let developed = develop(
                    &context,
                    &mut developer,
                    &captured,
                    &frame,
                    crop,
                    RgbaReadback::NotNeeded,
                );
                // Шаги записи: полосы вывода (1 или 3) и один шаг отделки (миниатюра, переход, копии; 14g).
                let expected_slices = if slice_rows == 4096 { 1 + 1 } else { 3 + 1 };
                assert_eq!(developed.slice_times.len(), expected_slices);
                assert_matches_reference(
                    &format!("{name}/{kind:?}/slices={slice_rows}"),
                    &frame,
                    &developed.image_rgba,
                );
            }
        }
    }
    assert_no_validation_errors(&context);
}

#[test]
fn zone_plate_matches_reference_for_every_cfa_layout() {
    let Some(context) = gpu_context_or_skip() else { return };
    let mut capturer = RawCapturer::new(&context, &PhotoConfig::default()).unwrap();
    for pattern in PATTERNS {
        let frame = zone_plate_frame(64, 48, pattern);
        let gpu_frame = upload(&context, &frame.raw, SourceKind::Raw16, Placement::PADDED);
        let captured = capture(&context, &mut capturer, &gpu_frame);
        let mut developer = developer_for(&context, 16, 64);
        let crop = crop_in_buffer(gpu_frame.valid_area, 0, 0, 64, 48);
        let developed = develop(
            &context,
            &mut developer,
            &captured,
            &frame,
            crop,
            RgbaReadback::NotNeeded,
        );
        assert_matches_reference(&format!("zone_plate/{pattern:?}"), &frame, &developed.image_rgba);
    }
    assert_no_validation_errors(&context);
}

#[test]
fn larger_zone_plate_in_slices_smaller_than_the_halo_matches_reference() {
    let Some(context) = gpu_context_or_skip() else { return };
    let mut capturer = RawCapturer::new(&context, &PhotoConfig::default()).unwrap();
    let frame = zone_plate_frame(128, 96, isp_model::cfa::BayerPattern::BGGR);
    let gpu_frame = upload(&context, &frame.raw, SourceKind::Raw10, Placement::PADDED);
    let captured = capture(&context, &mut capturer, &gpu_frame);
    // Полосы в 6 и 10 строк — ниже ореола в 12.
    for slice_rows in [6, 10] {
        let mut developer = developer_for(&context, slice_rows, 128);
        let crop = crop_in_buffer(gpu_frame.valid_area, 0, 0, 128, 96);
        let developed = develop(
            &context,
            &mut developer,
            &captured,
            &frame,
            crop,
            RgbaReadback::NotNeeded,
        );
        assert_eq!(developed.photo.size(), PixelSize::new(128, 96));
        assert_matches_reference(
            &format!("zone_plate_128x96/slices={slice_rows}"),
            &frame,
            &developed.image_rgba,
        );
    }
    assert_no_validation_errors(&context);
}

#[test]
fn residual_exposure_gain_of_the_frame_is_repeated_by_the_still() {
    let Some(context) = gpu_context_or_skip() else { return };
    let mut capturer = RawCapturer::new(&context, &PhotoConfig::default()).unwrap();
    let base = real_frame("real-main-lamp-jar");
    let size = base.raw.size();
    let gpu_frame = upload(&context, &base.raw, SourceKind::Raw16, Placement::PADDED);
    let captured = capture(&context, &mut capturer, &gpu_frame);
    let crop = crop_in_buffer(gpu_frame.valid_area, 0, 0, size.width, size.height);
    let mut developer = developer_for(&context, 16, size.width);
    let mean = |rgba: &[u8]| -> f64 {
        rgba.chunks_exact(4).map(|pixel| f64::from(pixel[1])).sum::<f64>() / (rgba.len() / 4) as f64
    };
    let unity = develop(
        &context,
        &mut developer,
        &captured,
        &base,
        crop,
        RgbaReadback::NotNeeded,
    );
    let mut means = vec![(0.0_f32, mean(&unity.image_rgba))];
    // k нашего AE (решение 72: снимок повторяет подтяжку своего кадра): светлее и темнее нейтрали.
    for ev in [0.7_f32, -0.6] {
        let frame = base.with_exposure_ev(ev);
        let developed = develop(
            &context,
            &mut developer,
            &captured,
            &frame,
            crop,
            RgbaReadback::NotNeeded,
        );
        assert_matches_reference(&frame.name, &frame, &developed.image_rgba);
        means.push((ev, mean(&developed.image_rgba)));
    }
    println!("средняя яркость зелёного по подтяжкам (EV → 0..255): {means:?}");
    // Подтяжка действительно меняет картинку (иначе совпадение с эталоном ничего бы не доказало).
    assert!(means[1].1 > means[0].1 + 3.0, "{means:?}");
    assert!(means[2].1 < means[0].1 - 3.0, "{means:?}");
    assert_no_validation_errors(&context);
}

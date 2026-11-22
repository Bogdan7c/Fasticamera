//! Полосы = целый кадр БИТ-В-БИТ: линейные плоскости камеры (после уровней и Menon) и итоговый RGBA8 при
//! любой высоте полосы — одна полоса на весь кадр, полоса ниже ореола, неполная последняя. Эталон для
//! сравнения — тот же конвейер, проявивший кадр одной полосой, а одна полоса, в свою очередь, сверена с
//! `isp-model` (линейный RGB — допуск 0, как у стенда V4) и со стендом V4 (`QualityDemosaic`).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть способ сообщить о провале"
)]

mod photo_support;
mod quality_support;

use gpu_vulkan::compute::LocalSize;
use gpu_vulkan::kit_test_support::{run_one_shot_blocking, run_one_shot_blocking_traced};
use isp_model::cfa::BayerPattern;
use isp_vulkan::photo::{DevelopRequest, PhotoConfig, RawCapturer, RgbaReadback, plan_development};
use isp_vulkan::quality::{CfaCode, QualityDemosaic};
use photo_support::{
    Placement, SourceKind, capture, compare_rgba, crop_in_buffer, develop_with, developer_for, read_planes, real_frame,
    run_reference_for, upload, zone_plate_frame,
};
use quality_support::{
    assert_no_validation_errors, extent, gpu_context_or_skip, max_rgb_difference, read_rgb, upload_plane,
};
use still_contract::shot::StillRotation;

/// Линейный RGB камеры из проявителя совпадает с эталоном CPU бит в бит (как у стенда V4).
const LINEAR_TOLERANCE: f32 = 0.0;

/// Проявляет кадр с заданной высотой полосы; возвращает (линейные пиксели ВЫХОДНЫХ строк, RGBA8).
fn develop_collecting_linear(
    context: &gpu_vulkan::context::GpuContext,
    capture_of: &isp_vulkan::photo::RawCapture,
    frame: &photo_support::SourceFrame,
    crop: camera_contract::geometry::PixelRect,
    slice_rows: u32,
) -> (Vec<[f32; 3]>, Vec<u8>) {
    let valid = capture_of.valid_area();
    let mut developer = developer_for(context, slice_rows, valid.width);
    let crop_in_valid = camera_contract::geometry::PixelRect {
        left: crop.left - valid.left,
        top: crop.top - valid.top,
        size: crop.size,
    };
    let plan = plan_development(capture_of.size(), crop_in_valid, slice_rows).unwrap();
    let mut linear: Vec<[f32; 3]> = Vec::new();
    let request = DevelopRequest {
        raw: capture_of,
        params: &frame.params,
        crop,
        readback: RgbaReadback::NotNeeded,
        rotation: StillRotation::None,
    };
    let developed = develop_with(context, &mut developer, &request, |developer, slice_index| {
        // Последний шаг — отделка (миниатюра, копии): плоскостей полосы у неё нет.
        let Some(slice) = plan.slices.get(slice_index).copied() else {
            return;
        };
        let pixels = (plan.region_width * slice.region_rows) as usize;
        let planes = read_planes(developer.linear_rgb_planes().unwrap(), pixels);
        // Выходные строки полосы, колонки окна вывода.
        for row in slice.first_output_row..slice.first_output_row + slice.output_rows {
            let start = (row * plan.region_width + plan.window_left) as usize;
            linear.extend_from_slice(&planes[start..start + crop.size.width as usize]);
        }
    });
    assert_eq!(linear.len(), (crop.size.width * crop.size.height) as usize);
    (linear, developed.image_rgba)
}

#[test]
fn every_slice_height_is_bit_identical_to_one_slice_for_every_cfa_and_source() {
    let Some(context) = gpu_context_or_skip() else { return };
    let mut capturer = RawCapturer::new(&context, &PhotoConfig::default()).unwrap();
    for pattern in [
        BayerPattern::RGGB,
        BayerPattern::BGGR,
        BayerPattern::GRBG,
        BayerPattern::GBRG,
    ] {
        let frame = zone_plate_frame(96, 80, pattern);
        for kind in [SourceKind::Raw16, SourceKind::Raw10] {
            let gpu_frame = upload(&context, &frame.raw, kind, Placement::PADDED);
            let captured = capture(&context, &mut capturer, &gpu_frame);
            let crop = crop_in_buffer(gpu_frame.valid_area, 0, 0, 96, 80);
            let (whole_linear, whole_rgba) = develop_collecting_linear(&context, &captured, &frame, crop, 4096);
            // 1 полоса = весь кадр; ниже ореола (2, 6, 10); ровно ореол; кратно и некратно высоте (80).
            for slice_rows in [2, 6, 10, 12, 14, 16, 30, 40, 78, 80] {
                let (linear, rgba) = develop_collecting_linear(&context, &captured, &frame, crop, slice_rows);
                let worst = linear
                    .iter()
                    .zip(&whole_linear)
                    .enumerate()
                    .find(|(_, (got, want))| got.map(f32::to_bits) != want.map(f32::to_bits));
                assert!(
                    worst.is_none(),
                    "{pattern:?}/{kind:?}/полоса {slice_rows}: линейный RGB отличается в пикселе {worst:?}"
                );
                let difference = compare_rgba(&rgba, &whole_rgba, 96);
                assert_eq!(
                    difference.differing_bytes, 0,
                    "{pattern:?}/{kind:?}/полоса {slice_rows}: RGBA8 отличается {difference:?}"
                );
            }
        }
    }
    assert_no_validation_errors(&context);
}

#[test]
fn single_slice_linear_rgb_matches_reference_and_the_v4_workload_demosaic_exactly() {
    let Some(context) = gpu_context_or_skip() else { return };
    let mut capturer = RawCapturer::new(&context, &PhotoConfig::default()).unwrap();
    for name in ["real-main-lamp-jar", "real-main-lamp-tap-highlight"] {
        let frame = real_frame(name);
        let size = frame.raw.size();
        let gpu_frame = upload(&context, &frame.raw, SourceKind::Raw10, Placement::PADDED);
        let captured = capture(&context, &mut capturer, &gpu_frame);
        let crop = crop_in_buffer(gpu_frame.valid_area, 0, 0, size.width, size.height);
        let (linear, _) = develop_collecting_linear(&context, &captured, &frame, crop, 4096);

        // 1) Эталон CPU: линейный RGB камеры после уровней, баланса и демозаика.
        let reference = run_reference_for(&frame);
        let (difference, at) = max_rgb_difference(&linear, reference.camera_rgb.pixels(), size.width);
        assert!(
            difference <= LINEAR_TOLERANCE,
            "{name}: линейный RGB против isp-model {difference:e} в {at:?}"
        );

        // 2) Стенд V4: тот же сбалансированный мозаик через `QualityDemosaic` даёт тот же линейный RGB.
        let local = LocalSize::new(16, 16).unwrap();
        let v4 = QualityDemosaic::new(&context, extent(size.width, size.height), local, 1).unwrap();
        upload_plane(v4.planes().mosaic(), reference.white_balanced.samples());
        run_one_shot_blocking(&context, |command_buffer| {
            v4.record_all(&context, command_buffer, 0, CfaCode::from_pattern(frame.params.pattern))
        })
        .unwrap();
        let v4_linear = read_rgb(v4.planes().linear_rgb());
        let (difference, at) = max_rgb_difference(&linear, &v4_linear, size.width);
        assert!(
            difference <= LINEAR_TOLERANCE,
            "{name}: против стенда V4 {difference:e} в {at:?}"
        );
    }
    assert_no_validation_errors(&context);
}

#[test]
fn all_slices_recorded_into_one_command_buffer_without_waiting_equal_slice_by_slice_results() {
    let Some(context) = gpu_context_or_skip() else { return };
    let mut capturer = RawCapturer::new(&context, &PhotoConfig::default()).unwrap();
    let frame = zone_plate_frame(96, 80, BayerPattern::GRBG);
    let gpu_frame = upload(&context, &frame.raw, SourceKind::Raw10, Placement::PADDED);
    let captured = capture(&context, &mut capturer, &gpu_frame);
    let crop = crop_in_buffer(gpu_frame.valid_area, 3, 5, 80, 66);
    let request = DevelopRequest {
        raw: &captured,
        params: &frame.params,
        crop,
        readback: RgbaReadback::ForJpeg,
        rotation: StillRotation::None,
    };
    let mut slow = developer_for(&context, 14, 96);
    let slice_by_slice = develop_with(&context, &mut slow, &request, |_, _| {});

    // Полосы подряд в ОДИН командный буфер, без ожидания между ними: порядок держат только барьеры внутри
    // командного буфера (validation с проверкой синхронизации — FULL — не должна найти гонок).
    let mut fast = developer_for(&context, 14, 96);
    let start = fast.begin(&request).unwrap();
    assert!(start.slice_count > 3, "нужно несколько полос");
    let mut recorded = 0;
    let run = run_one_shot_blocking_traced(&context, |command_buffer| {
        loop {
            match fast
                .record_next_slice(command_buffer)
                .map_err(photo_support::into_gpu_error)?
            {
                isp_vulkan::photo::SliceOutcome::Recorded { .. } => recorded += 1,
                isp_vulkan::photo::SliceOutcome::Finished => return Ok(()),
            }
        }
    })
    .unwrap();
    // Шаги записи: все полосы вывода и шаг отделки (`slice_count` их и считает).
    assert_eq!(recorded, start.slice_count);
    fast.note_slice_submitted(&run.work).unwrap();
    for _ in 0..recorded {
        fast.slice_completed(&run.proof).unwrap();
    }
    let photo = fast.finish().unwrap();
    let image = photo.gpu_image().read_back_blocking().unwrap();
    assert_eq!(compare_rgba(&image, &slice_by_slice.image_rgba, 80).differing_bytes, 0);
    let host = photo.host_rgba8().unwrap();
    assert!(host.rgba_bytes() == slice_by_slice.photo.host_rgba8().unwrap().rgba_bytes());
    assert_no_validation_errors(&context);
}

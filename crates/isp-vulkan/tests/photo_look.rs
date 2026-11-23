//! Слой вида в проявлении СНИМКА (решения 98, 99, 176; снимок берёт вид из запроса): RGBA8 против эталона `isp-model`
//! (допуск 1 LSB, как у `photo_reference.rs`), полосы = целый кадр БИТ-В-БИТ с видом и таблицей (как `photo_slices.rs`),
//! нейтральный вид = основа бит в бит, смена вида между проявками одним проявителем, таблица больше буфера — отказ
//! `begin` и `check_request` до выделений.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть способ сообщить о провале"
)]

mod lens_shading_support;
mod look_support;
mod photo_support;
mod quality_support;

use camera_contract::geometry::PixelRect;
use isp_model::cfa::BayerPattern;
use isp_model::look::LookControls;
use isp_model::tone::agx_settings::AgxSettings;
use isp_model::tone::base::ToneBase;
use isp_vulkan::look::{LookLutError, LookLutLimits};
use isp_vulkan::photo::{DevelopRequest, PhotoConfig, PhotoDeveloper, PhotoError, RawCapturer, RgbaReadback};
use look_contract::LookSettings;
use look_support::{all_bands, controls, identity_lut, knob, known_lut, saturation_vibrance, with_lut};
use photo_support::{
    Placement, SourceFrame, SourceKind, capture, compare_rgba, crop_in_buffer, crop_rgba, develop_with, developer_for,
    real_frame, reference_rgba_any_base, upload, zone_plate_frame_at_level,
};
use quality_support::{assert_no_validation_errors, gpu_context_or_skip};
use still_contract::shot::StillRotation;

const MAX_LSB_DIFFERENCE: u8 = 1;
const MIN_EXACT_SHARE: f64 = 0.999;

fn bases() -> [(&'static str, ToneBase); 2] {
    [
        ("прежняя", ToneBase::Legacy),
        ("AgX", ToneBase::Agx(AgxSettings::default())),
    ]
}

/// Видимые виды снимка: цвет, полосы, теплота, таблица.
fn looks() -> Vec<(&'static str, LookControls)> {
    vec![
        ("насыщенность +1", saturation_vibrance(1.0, 0.0)),
        ("сочность −1", saturation_vibrance(0.0, -1.0)),
        ("полосы всё сразу", all_bands(0.7, -0.6, 0.5)),
        (
            "теплота+цвет",
            controls(LookSettings {
                warmth: knob(0.7),
                saturation: knob(0.4),
                ..LookSettings::NEUTRAL
            }),
        ),
        ("таблица-единица", with_lut(LookControls::default(), identity_lut())),
        (
            "известная таблица + цвет",
            with_lut(saturation_vibrance(0.5, 0.3), known_lut()),
        ),
    ]
}

fn develop_rgba(
    context: &gpu_vulkan::context::GpuContext,
    captured: &isp_vulkan::photo::RawCapture,
    frame: &SourceFrame,
    crop: PixelRect,
    slice_rows: u32,
) -> Vec<u8> {
    let mut developer = developer_for(context, slice_rows, captured.valid_area().width);
    let request = DevelopRequest {
        raw: captured,
        params: &frame.params,
        crop,
        readback: RgbaReadback::NotNeeded,
        rotation: StillRotation::None,
    };
    develop_with(context, &mut developer, &request, |_, _| {}).image_rgba
}

fn assert_close_to_reference(name: &str, got: &[u8], want: &[u8], width: u32) {
    let difference = compare_rgba(got, want, width);
    let exact_share = 1.0 - difference.differing_bytes as f64 / got.len() as f64;
    println!(
        "{name}: max |Δ| {} LSB, точно {:.4} %, первое {:?}",
        difference.max_abs,
        exact_share * 100.0,
        difference.first
    );
    assert!(
        difference.max_abs <= MAX_LSB_DIFFERENCE,
        "{name}: {} LSB в {:?}",
        difference.max_abs,
        difference.first
    );
    assert!(
        exact_share >= MIN_EXACT_SHARE,
        "{name}: точно совпало только {exact_share}"
    );
}

/// Кадры снимка: реальные кропы, зонная пластинка с насыщением (цветовые переходы и насыщенность для вида).
fn source_frames() -> Vec<SourceFrame> {
    vec![
        real_frame("real-main-lamp-jar"),
        real_frame("real-main-lamp-towel"),
        zone_plate_frame_at_level(96, 80, BayerPattern::RGGB, 1.0),
    ]
}

#[test]
fn look_matches_reference_for_both_bases_in_one_and_many_slices_and_differs_from_neutral() {
    let Some(context) = gpu_context_or_skip() else { return };
    let mut capturer = RawCapturer::new(&context, &PhotoConfig::default()).unwrap();
    for source in source_frames() {
        let size = source.raw.size();
        let gpu_frame = upload(&context, &source.raw, SourceKind::Raw16, Placement::PADDED);
        let captured = capture(&context, &mut capturer, &gpu_frame);
        let crop = crop_in_buffer(gpu_frame.valid_area, 0, 0, size.width, size.height);
        for (base_name, base) in bases() {
            let neutral = source.with_tone_base(base);
            let neutral_rgba = develop_rgba(&context, &captured, &neutral, crop, 4096);
            assert_close_to_reference(
                &format!("{}/{base_name}/нейтраль", source.name),
                &neutral_rgba,
                &reference_rgba_any_base(&neutral),
                size.width,
            );
            for (look_name, look) in looks() {
                let frame = source.with_tone_base_and_look(base, look);
                let want = reference_rgba_any_base(&frame);
                for slice_rows in [4096, 16] {
                    let got = develop_rgba(&context, &captured, &frame, crop, slice_rows);
                    assert_close_to_reference(
                        &format!("{}/{base_name}/{look_name}/полосы={slice_rows}", source.name),
                        &got,
                        &want,
                        size.width,
                    );
                    // Вид, который что-то делает, меняет итог (иначе паритет пуст); «единица» — почти нет.
                    if slice_rows == 4096 && look_name != "таблица-единица" && source.name.starts_with("zone")
                    {
                        assert_ne!(
                            got, neutral_rgba,
                            "{}/{base_name}/{look_name}: вид не изменил снимок",
                            source.name
                        );
                    }
                }
            }
        }
    }
    assert_no_validation_errors(&context);
}

#[test]
fn slices_of_every_height_and_an_offset_crop_are_bit_identical_with_look_and_table() {
    let Some(context) = gpu_context_or_skip() else { return };
    let mut capturer = RawCapturer::new(&context, &PhotoConfig::default()).unwrap();
    let look = with_lut(all_bands(0.5, 0.4, -0.3), known_lut());
    for (base_name, base) in bases() {
        let frame =
            zone_plate_frame_at_level(96, 80, BayerPattern::GRBG, 1.0).with_tone_base_and_look(base, look.clone());
        let whole_reference = reference_rgba_any_base(&frame);
        for kind in [SourceKind::Raw16, SourceKind::Raw10] {
            let gpu_frame = upload(&context, &frame.raw, kind, Placement::PADDED);
            let captured = capture(&context, &mut capturer, &gpu_frame);
            // Область снимка не в углу: у полос свои смещения.
            let (left, top, width, height) = (20, 18, 40, 30);
            let crop = crop_in_buffer(gpu_frame.valid_area, left, top, width, height);
            let want = crop_rgba(&whole_reference, 96, left, top, width, height);
            let whole = develop_rgba(&context, &captured, &frame, crop, 4096);
            assert_close_to_reference(&format!("{base_name}/{kind:?}/окно"), &whole, &want, width);
            for slice_rows in [2, 6, 8, 14, 30] {
                let sliced = develop_rgba(&context, &captured, &frame, crop, slice_rows);
                assert_eq!(
                    compare_rgba(&sliced, &whole, width).differing_bytes,
                    0,
                    "{base_name}/{kind:?}/полоса {slice_rows}: RGBA8 отличается от одной полосы"
                );
            }
        }
    }
    assert_no_validation_errors(&context);
}

#[test]
fn the_same_developer_switches_looks_and_neutral_returns_bit_for_bit() {
    let Some(context) = gpu_context_or_skip() else { return };
    let mut capturer = RawCapturer::new(&context, &PhotoConfig::default()).unwrap();
    let source = zone_plate_frame_at_level(96, 80, BayerPattern::RGGB, 1.0);
    let size = source.raw.size();
    let gpu_frame = upload(&context, &source.raw, SourceKind::Raw16, Placement::PADDED);
    let captured = capture(&context, &mut capturer, &gpu_frame);
    let crop = crop_in_buffer(gpu_frame.valid_area, 0, 0, size.width, size.height);
    let mut developer = developer_for(&context, 16, size.width);
    let mut develop = |frame: &SourceFrame| {
        let request = DevelopRequest {
            raw: &captured,
            params: &frame.params,
            crop,
            readback: RgbaReadback::NotNeeded,
            rotation: StillRotation::None,
        };
        develop_with(&context, &mut developer, &request, |_, _| {}).image_rgba
    };
    let base = ToneBase::Agx(AgxSettings::default());
    let neutral = source.with_tone_base(base);
    let with_table = source.with_tone_base_and_look(base, with_lut(saturation_vibrance(0.6, 0.0), known_lut()));
    let colour_only = source.with_tone_base_and_look(base, saturation_vibrance(0.6, 0.0));
    let first = develop(&neutral);
    let second = develop(&with_table);
    let third = develop(&colour_only);
    let fourth = develop(&neutral);
    assert_close_to_reference(
        "таблица+цвет",
        &second,
        &reference_rgba_any_base(&with_table),
        size.width,
    );
    assert_close_to_reference(
        "цвет без таблицы",
        &third,
        &reference_rgba_any_base(&colour_only),
        size.width,
    );
    assert_ne!(first, second, "вид не изменил снимок");
    assert_ne!(second, third, "таблица не изменила снимок");
    assert_eq!(first, fourth, "возврат к нейтрали — тот же снимок бит в бит");
    assert_no_validation_errors(&context);
}

#[test]
fn table_larger_than_the_developer_buffer_is_refused_by_begin_and_check_request_without_side_effects() {
    let Some(context) = gpu_context_or_skip() else { return };
    let mut capturer = RawCapturer::new(&context, &PhotoConfig::default()).unwrap();
    let plain = zone_plate_frame_at_level(96, 80, BayerPattern::RGGB, 0.6);
    let gpu_frame = upload(&context, &plain.raw, SourceKind::Raw16, Placement::PADDED);
    let captured = capture(&context, &mut capturer, &gpu_frame);
    let crop = crop_in_buffer(gpu_frame.valid_area, 0, 0, 96, 80);
    let config = PhotoConfig {
        look_lut: LookLutLimits { max_size: 5 },
        ..PhotoConfig::default()
    };
    let mut developer = PhotoDeveloper::new(&context, &config, 96).unwrap();
    let base = ToneBase::Agx(AgxSettings::default());
    let too_big = plain.with_tone_base_and_look(base, with_lut(LookControls::default(), known_lut()));
    fn request_for<'a>(
        captured: &'a isp_vulkan::photo::RawCapture,
        frame: &'a SourceFrame,
        crop: PixelRect,
    ) -> DevelopRequest<'a> {
        DevelopRequest {
            raw: captured,
            params: &frame.params,
            crop,
            readback: RgbaReadback::NotNeeded,
            rotation: StillRotation::None,
        }
    }
    let expected =
        |error: &PhotoError| matches!(error, PhotoError::LookLut(LookLutError::TooLarge { size: 9, max: 5 }));
    let check_error = developer
        .check_request(&request_for(&captured, &too_big, crop))
        .unwrap_err();
    assert!(expected(&check_error), "{check_error}");
    let begin_error = developer.begin(&request_for(&captured, &too_big, crop)).unwrap_err();
    assert!(expected(&begin_error), "{begin_error}");
    // Отказ не оставил активной проявки: подходящая таблица (5³) проявляется и совпадает с эталоном.
    let fits = plain.with_tone_base_and_look(
        base,
        with_lut(
            LookControls::default(),
            isp_model::look::lut::LookLut::identity(5, 65).unwrap(),
        ),
    );
    let rgba = develop_with(
        &context,
        &mut developer,
        &request_for(&captured, &fits, crop),
        |_, _| {},
    )
    .image_rgba;
    assert_close_to_reference("после отказа", &rgba, &reference_rgba_any_base(&fits), 96);
    assert_no_validation_errors(&context);
}

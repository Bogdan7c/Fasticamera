//! Проявление снимка с основой ACES и своим белым точки (решения 176, 177): RGBA8 против эталона `isp-model`
//! (допуск 1 LSB, как у прежней основы — `photo_reference.rs`) и полосы = целый кадр БИТ-В-БИТ (как
//! `photo_slices.rs`) на кадре с НЕплоской картой виньетирования и областью снимка со смещением: белый точки
//! выходной проход берёт по карте в координатах полезной области, и ошибка в смещении области полосы или
//! снимка изменила бы белый и сломала сравнение.

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
use isp_model::pipeline::run_reference_working;
use isp_model::tone::aces::AcesSettings;
use isp_model::tone::base::ToneBase;
use isp_vulkan::photo::{DevelopRequest, PhotoConfig, RawCapturer, RgbaReadback};
use photo_support::{
    Placement, SourceFrame, SourceKind, capture, compare_rgba, crop_in_buffer, crop_rgba, develop_with, developer_for,
    real_frame, reference_rgba_any_base, upload, zone_plate_frame_at_level,
};
use quality_support::{assert_no_validation_errors, gpu_context_or_skip};
use still_contract::shot::StillRotation;

const MAX_LSB_DIFFERENCE: u8 = 1;
const MIN_EXACT_SHARE: f64 = 0.999;

fn aces(frame: &SourceFrame) -> SourceFrame {
    frame.with_tone_base(ToneBase::Aces(AcesSettings::default()))
}

fn shaded_aces(frame: &SourceFrame) -> SourceFrame {
    aces(frame).with_lens_shading(&lens_shading_support::asymmetric_map(6, 5))
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

#[test]
fn aces_still_matches_reference_for_real_crops_and_saturated_plates_in_one_and_many_slices() {
    let Some(context) = gpu_context_or_skip() else { return };
    let mut capturer = RawCapturer::new(&context, &PhotoConfig::default()).unwrap();
    let mut frames: Vec<SourceFrame> = ["real-main-lamp-jar", "real-main-lamp-tap-highlight"]
        .into_iter()
        .map(|name| aces(&real_frame(name)))
        .collect();
    // Реальные кропы с картой: свой белый точки разный по полю.
    frames.extend(
        ["real-main-lamp-jar", "real-main-lamp-towel"]
            .into_iter()
            .map(|name| shaded_aces(&real_frame(name))),
    );
    for pattern in [
        BayerPattern::RGGB,
        BayerPattern::BGGR,
        BayerPattern::GRBG,
        BayerPattern::GBRG,
    ] {
        frames.push(shaded_aces(&zone_plate_frame_at_level(96, 80, pattern, 1.3)));
    }
    for frame in &frames {
        let size = frame.raw.size();
        let want = reference_rgba_any_base(frame);
        for kind in [SourceKind::Raw16, SourceKind::Raw10] {
            let gpu_frame = upload(&context, &frame.raw, kind, Placement::PADDED);
            let captured = capture(&context, &mut capturer, &gpu_frame);
            let crop = crop_in_buffer(gpu_frame.valid_area, 0, 0, size.width, size.height);
            for slice_rows in [4096, 16] {
                let got = develop_rgba(&context, &captured, frame, crop, slice_rows);
                assert_close_to_reference(
                    &format!("{}/{kind:?}/slices={slice_rows}", frame.name),
                    &got,
                    &want,
                    size.width,
                );
            }
        }
    }
    assert_no_validation_errors(&context);
}

#[test]
fn aces_slices_of_every_height_and_an_offset_crop_are_bit_identical_to_one_slice_and_match_the_reference_window() {
    let Some(context) = gpu_context_or_skip() else { return };
    let mut capturer = RawCapturer::new(&context, &PhotoConfig::default()).unwrap();
    for pattern in [BayerPattern::RGGB, BayerPattern::GRBG] {
        let frame = shaded_aces(&zone_plate_frame_at_level(96, 80, pattern, 1.3));
        // Эталон ACES доходит до света выше общего белого (иначе свой белый точки ничего бы не менял).
        let reference = run_reference_working(&frame.raw, &frame.params).unwrap();
        let brightest = reference
            .front_end
            .camera_rgb
            .pixels()
            .iter()
            .flatten()
            .copied()
            .fold(0.0_f32, f32::max);
        assert!(
            brightest > 1.2,
            "{pattern:?}: кадр не доходит до белого точки выше 1 ({brightest})"
        );
        let whole_reference = reference_rgba_any_base(&frame);
        for kind in [SourceKind::Raw16, SourceKind::Raw10] {
            let gpu_frame = upload(&context, &frame.raw, kind, Placement::PADDED);
            let captured = capture(&context, &mut capturer, &gpu_frame);
            // Область снимка не в углу: у полос свои смещения, а белый точки читается по абсолютным координатам.
            let (left, top, width, height) = (20, 18, 40, 30);
            let crop = crop_in_buffer(gpu_frame.valid_area, left, top, width, height);
            let want = crop_rgba(&whole_reference, 96, left, top, width, height);
            let whole = develop_rgba(&context, &captured, &frame, crop, 4096);
            assert_close_to_reference(&format!("{pattern:?}/{kind:?}/окно"), &whole, &want, width);
            for slice_rows in [2, 6, 8, 14, 30] {
                let sliced = develop_rgba(&context, &captured, &frame, crop, slice_rows);
                assert_eq!(
                    compare_rgba(&sliced, &whole, width).differing_bytes,
                    0,
                    "{pattern:?}/{kind:?}/полоса {slice_rows}: RGBA8 отличается от одной полосы"
                );
            }
        }
    }
    assert_no_validation_errors(&context);
}

#[test]
fn the_same_developer_switches_between_bases_and_each_matches_its_own_reference() {
    let Some(context) = gpu_context_or_skip() else { return };
    let mut capturer = RawCapturer::new(&context, &PhotoConfig::default()).unwrap();
    let legacy = real_frame("real-main-lamp-jar").with_lens_shading(&lens_shading_support::asymmetric_map(6, 5));
    let aces_frame = shaded_aces(&real_frame("real-main-lamp-jar"));
    let size = legacy.raw.size();
    let gpu_frame = upload(&context, &legacy.raw, SourceKind::Raw16, Placement::PADDED);
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
    let first = develop(&legacy);
    let second = develop(&aces_frame);
    let third = develop(&legacy);
    assert_close_to_reference("legacy#1", &first, &reference_rgba_any_base(&legacy), size.width);
    assert_close_to_reference("aces", &second, &reference_rgba_any_base(&aces_frame), size.width);
    assert_ne!(first, second, "основы дали одну картинку");
    assert_eq!(first, third, "возврат на прежнюю основу — та же картинка бит в бит");
    assert_no_validation_errors(&context);
}

/// Слой вида поверх ACES в снимке: цвет, полосы, таблица — RGBA8 против эталона, полосы = целый кадр бит в бит, вид
/// меняет снимок; нейтральный вид — тот же снимок, что без вида, бит в бит.
#[test]
fn look_layer_on_top_of_aces_matches_reference_in_slices_and_changes_the_still() {
    use look_support::{all_bands, known_lut, saturation_vibrance, with_lut};
    let Some(context) = gpu_context_or_skip() else { return };
    let mut capturer = RawCapturer::new(&context, &PhotoConfig::default()).unwrap();
    let plain = aces(&zone_plate_frame_at_level(96, 80, BayerPattern::RGGB, 1.0));
    let size = plain.raw.size();
    let gpu_frame = upload(&context, &plain.raw, SourceKind::Raw16, Placement::PADDED);
    let captured = capture(&context, &mut capturer, &gpu_frame);
    let crop = crop_in_buffer(gpu_frame.valid_area, 0, 0, size.width, size.height);
    let neutral = develop_rgba(&context, &captured, &plain, crop, 4096);
    for (name, look) in [
        ("насыщенность и сочность", saturation_vibrance(0.6, 0.3)),
        ("полосы", all_bands(0.3, 0.4, -0.2)),
        ("таблица", with_lut(saturation_vibrance(0.3, 0.0), known_lut())),
    ] {
        let frame = plain.with_tone_base_and_look(ToneBase::Aces(AcesSettings::default()), look);
        let want = reference_rgba_any_base(&frame);
        let whole = develop_rgba(&context, &captured, &frame, crop, 4096);
        assert_close_to_reference(&format!("вид {name}"), &whole, &want, size.width);
        let sliced = develop_rgba(&context, &captured, &frame, crop, 16);
        assert_eq!(
            compare_rgba(&sliced, &whole, size.width).differing_bytes,
            0,
            "вид {name}: полосы отличаются от целого кадра"
        );
        assert_ne!(whole, neutral, "вид {name} не меняет снимок ACES");
    }
    let again = plain.with_tone_base_and_look(ToneBase::Aces(AcesSettings::default()), LookControls::default());
    assert_eq!(
        develop_rgba(&context, &captured, &again, crop, 4096),
        neutral,
        "нейтральный вид меняет снимок"
    );
    assert_no_validation_errors(&context);
}

/// Настройки модели ACES на краях диапазонов (по одной за раз): RGBA8 против эталона на реальном кропе и насыщенной
/// пластинке, полосы = целый кадр бит в бит; каждая настройка меняет снимок.
#[test]
fn model_settings_at_range_edges_match_reference_and_each_changes_the_still() {
    let Some(context) = gpu_context_or_skip() else { return };
    let mut capturer = RawCapturer::new(&context, &PhotoConfig::default()).unwrap();
    let default = AcesSettings::default();
    let edges = [
        (
            "контраст",
            AcesSettings {
                contrast: 1.5,
                ..default
            },
        ),
        (
            "тени",
            AcesSettings {
                shadows_ev: 3.0,
                ..default
            },
        ),
        (
            "света",
            AcesSettings {
                highlights_ev: -2.0,
                ..default
            },
        ),
        (
            "серый экрана",
            AcesSettings {
                display_middle_gray: 0.3,
                ..default
            },
        ),
        (
            "колено",
            AcesSettings {
                knee_ev: 2.5,
                ..default
            },
        ),
        (
            "без свечения",
            AcesSettings {
                glow_gain: 0.0,
                ..default
            },
        ),
        (
            "красный модификатор",
            AcesSettings {
                red_modifier_scale: 0.0,
                ..default
            },
        ),
        (
            "десатурация RRT",
            AcesSettings {
                rrt_saturation: 0.5,
                ..default
            },
        ),
        (
            "десатурация выхода",
            AcesSettings {
                odt_saturation: 1.0,
                ..default
            },
        ),
        (
            "окружение",
            AcesSettings {
                surround_gamma: 0.75,
                ..default
            },
        ),
    ];
    let sources = [
        real_frame("real-main-lamp-jar"),
        zone_plate_frame_at_level(96, 80, BayerPattern::RGGB, 1.3),
    ];
    let mut default_pictures = Vec::new();
    for (name, settings) in std::iter::once(("умолчание", default)).chain(edges) {
        let mut pictures = Vec::new();
        for source in &sources {
            let frame = source.with_tone_base(ToneBase::Aces(settings));
            let size = frame.raw.size();
            let want = reference_rgba_any_base(&frame);
            let gpu_frame = upload(&context, &frame.raw, SourceKind::Raw16, Placement::PADDED);
            let captured = capture(&context, &mut capturer, &gpu_frame);
            let crop = crop_in_buffer(gpu_frame.valid_area, 0, 0, size.width, size.height);
            let whole = develop_rgba(&context, &captured, &frame, crop, 4096);
            assert_close_to_reference(&format!("{name}/{}", frame.name), &whole, &want, size.width);
            let sliced = develop_rgba(&context, &captured, &frame, crop, 16);
            assert_eq!(
                compare_rgba(&sliced, &whole, size.width).differing_bytes,
                0,
                "{name}/{}: полосы отличаются от целого кадра",
                frame.name
            );
            pictures.push(whole);
        }
        if default_pictures.is_empty() {
            default_pictures = pictures;
        } else {
            assert_ne!(pictures, default_pictures, "{name}: настройка не дошла до снимка");
        }
    }
    assert_no_validation_errors(&context);
}

//! Проявление снимка с основой filmic и своим белым точки (решения 176, 177, 182): RGBA8 против эталона
//! `isp-model` (допуск 1 LSB, как у прежней основы — `photo_reference.rs`) и полосы = целый кадр БИТ-В-БИТ (как
//! `photo_slices.rs`) на кадре с НЕплоской картой виньетирования и областью снимка со смещением. Все четыре нормы,
//! развёртка по оттенку (обесцвечивание в светах и вписывание в охват), ручки тона на краях, слой вида поверх.

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
use isp_model::tone::base::ToneBase;
use isp_model::tone::filmic::{FilmicNorm, FilmicSettings};
use isp_vulkan::photo::{DevelopRequest, PhotoConfig, RawCapturer, RgbaReadback};
use look_contract::LookSettings;
use photo_support::{
    Placement, SourceFrame, SourceKind, capture, compare_rgba, crop_in_buffer, crop_rgba, develop_with, developer_for,
    real_frame, reference_rgba_any_base, upload, zone_plate_frame_at_level,
};
use quality_support::{assert_no_validation_errors, gpu_context_or_skip};
use still_contract::shot::StillRotation;

const MAX_LSB_DIFFERENCE: u8 = 1;
const MIN_EXACT_SHARE: f64 = 0.999;

const ALL_NORMS: [FilmicNorm; 4] = [
    FilmicNorm::PerChannel,
    FilmicNorm::MaxRgb,
    FilmicNorm::Luminance,
    FilmicNorm::PowerNorm,
];

fn settings(norm: FilmicNorm) -> FilmicSettings {
    FilmicSettings {
        norm,
        ..FilmicSettings::default()
    }
}

fn filmic(frame: &SourceFrame, norm: FilmicNorm) -> SourceFrame {
    frame.with_tone_base(ToneBase::Filmic(settings(norm)))
}

fn shaded_filmic(frame: &SourceFrame, norm: FilmicNorm) -> SourceFrame {
    filmic(frame, norm).with_lens_shading(&lens_shading_support::asymmetric_map(6, 5))
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

/// Кадр с развёрткой по оттенку (насыщенные цвета всех оттенков, от тёмных к светлым) на сенсоре реального кропа.
fn hue_sweep_frame() -> SourceFrame {
    let base = real_frame("real-main-lamp-gray-tile");
    let raw = look_support::hue_sweep_mosaic(&base.sensor, base.neutral, 96, 80);
    SourceFrame {
        name: "развёртка по оттенку".to_owned(),
        raw,
        ..base
    }
}

/// Проявляет кадр целиком одной полосой и полосами по 16 строк: RGBA8 против эталона, полосы = целый кадр бит в бит.
/// Возвращает RGBA8 целого кадра.
fn assert_still_matches_reference_in_one_and_many_slices(
    context: &gpu_vulkan::context::GpuContext,
    capturer: &mut RawCapturer,
    frame: &SourceFrame,
) -> Vec<u8> {
    let size = frame.raw.size();
    let want = reference_rgba_any_base(frame);
    let gpu_frame = upload(context, &frame.raw, SourceKind::Raw16, Placement::PADDED);
    let captured = capture(context, capturer, &gpu_frame);
    let crop = crop_in_buffer(gpu_frame.valid_area, 0, 0, size.width, size.height);
    let whole = develop_rgba(context, &captured, frame, crop, 4096);
    assert_close_to_reference(&format!("{}/целиком", frame.name), &whole, &want, size.width);
    let sliced = develop_rgba(context, &captured, frame, crop, 16);
    assert_eq!(
        compare_rgba(&sliced, &whole, size.width).differing_bytes,
        0,
        "{}: полосы отличаются от целого кадра",
        frame.name
    );
    whole
}

#[test]
fn filmic_still_matches_reference_for_real_crops_and_saturated_plates_in_one_and_many_slices() {
    let Some(context) = gpu_context_or_skip() else { return };
    let mut capturer = RawCapturer::new(&context, &PhotoConfig::default()).unwrap();
    let mut frames: Vec<SourceFrame> = Vec::new();
    for norm in ALL_NORMS {
        frames.extend(
            ["real-main-lamp-jar", "real-main-lamp-tap-highlight"]
                .into_iter()
                .map(|name| filmic(&real_frame(name), norm)),
        );
        // Реальные кропы с картой: свой белый точки разный по полю.
        frames.push(shaded_filmic(&real_frame("real-main-lamp-towel"), norm));
        frames.push(shaded_filmic(
            &zone_plate_frame_at_level(96, 80, BayerPattern::RGGB, 1.3),
            norm,
        ));
    }
    for pattern in [BayerPattern::BGGR, BayerPattern::GRBG, BayerPattern::GBRG] {
        frames.push(shaded_filmic(
            &zone_plate_frame_at_level(96, 80, pattern, 1.3),
            FilmicNorm::PowerNorm,
        ));
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
fn filmic_slices_of_every_height_and_an_offset_crop_are_bit_identical_to_one_slice_and_match_the_reference_window() {
    let Some(context) = gpu_context_or_skip() else { return };
    let mut capturer = RawCapturer::new(&context, &PhotoConfig::default()).unwrap();
    for pattern in [BayerPattern::RGGB, BayerPattern::GRBG] {
        let frame = shaded_filmic(&zone_plate_frame_at_level(96, 80, pattern, 1.3), FilmicNorm::PowerNorm);
        // Эталон filmic доходит до света выше общего белого (иначе свой белый точки ничего бы не менял).
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
    let map = lens_shading_support::asymmetric_map(6, 5);
    let legacy = real_frame("real-main-lamp-jar").with_lens_shading(&map);
    let filmic_frame = shaded_filmic(&real_frame("real-main-lamp-jar"), FilmicNorm::PowerNorm);
    let agx_frame = real_frame("real-main-lamp-jar")
        .with_tone_base(ToneBase::Agx(isp_model::tone::agx_settings::AgxSettings::default()))
        .with_lens_shading(&map);
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
    let second = develop(&filmic_frame);
    let third = develop(&agx_frame);
    let fourth = develop(&filmic_frame);
    let fifth = develop(&legacy);
    assert_close_to_reference("legacy#1", &first, &reference_rgba_any_base(&legacy), size.width);
    assert_close_to_reference("filmic", &second, &reference_rgba_any_base(&filmic_frame), size.width);
    assert_close_to_reference("agx", &third, &reference_rgba_any_base(&agx_frame), size.width);
    assert_ne!(first, second, "прежняя основа и filmic дали одну картинку");
    assert_ne!(second, third, "filmic и AgX дали одну картинку");
    assert_eq!(second, fourth, "возврат на filmic — та же картинка бит в бит");
    assert_eq!(first, fifth, "возврат на прежнюю основу — та же картинка бит в бит");
    assert_no_validation_errors(&context);
}

/// Развёртка по оттенку на каждой норме: RGBA8 против эталона (обесцвечивание в светах и вписывание в охват),
/// полосы = целый кадр; нормы дают разные снимки.
#[test]
fn hue_sweep_matches_reference_for_every_norm_and_each_norm_changes_the_still() {
    let Some(context) = gpu_context_or_skip() else { return };
    let mut capturer = RawCapturer::new(&context, &PhotoConfig::default()).unwrap();
    let sweep = hue_sweep_frame();
    let mut stills = Vec::new();
    for norm in ALL_NORMS {
        let frame = filmic(&sweep, norm);
        stills.push((
            norm,
            assert_still_matches_reference_in_one_and_many_slices(&context, &mut capturer, &frame),
        ));
    }
    for (index, (norm, still)) in stills.iter().enumerate() {
        for (other_norm, other) in &stills[index + 1..] {
            assert_ne!(still, other, "нормы {norm:?} и {other_norm:?} дали один снимок");
        }
    }
    assert_no_validation_errors(&context);
}

/// Ручки тона на краях: углы {−1, +1}⁴ (контраст, средние, тени, света) на серой зонной пластинке с пересветом и
/// развёртке по оттенку — снимок совпадает с эталоном, углы дают разные снимки.
#[test]
fn tone_knob_corners_match_reference_and_each_changes_the_still() {
    let Some(context) = gpu_context_or_skip() else { return };
    let mut capturer = RawCapturer::new(&context, &PhotoConfig::default()).unwrap();
    let plate = zone_plate_frame_at_level(96, 80, BayerPattern::RGGB, 1.3);
    let sweep = hue_sweep_frame();
    let mut plate_stills = Vec::new();
    for corner in 0..16_u32 {
        let value = |bit: u32| look_support::knob(if corner >> bit & 1 == 1 { 1.0 } else { -1.0 });
        let look = look_support::controls(LookSettings {
            contrast: value(0),
            midtones: value(1),
            shadows: value(2),
            highlights: value(3),
            ..LookSettings::NEUTRAL
        });
        let base = ToneBase::Filmic(FilmicSettings::default());
        for source in [&plate, &sweep] {
            let frame = source.with_tone_base_and_look(base, look.clone());
            let still = assert_still_matches_reference_in_one_and_many_slices(&context, &mut capturer, &frame);
            if std::ptr::eq(source, &plate) {
                plate_stills.push(still);
            }
        }
    }
    for (index, still) in plate_stills.iter().enumerate() {
        for other in &plate_stills[index + 1..] {
            assert_ne!(still, other, "два угла ручек дали один снимок");
        }
    }
    assert_no_validation_errors(&context);
}

/// Слой вида поверх filmic: цвет, полосы и таблица — снимок совпадает с эталоном и отличается от нейтрального.
#[test]
fn look_layer_on_top_of_filmic_matches_reference_and_changes_the_still() {
    let Some(context) = gpu_context_or_skip() else { return };
    let mut capturer = RawCapturer::new(&context, &PhotoConfig::default()).unwrap();
    let sweep = hue_sweep_frame();
    let base = ToneBase::Filmic(FilmicSettings::default());
    let neutral = assert_still_matches_reference_in_one_and_many_slices(
        &context,
        &mut capturer,
        &sweep.with_tone_base_and_look(base, LookControls::default()),
    );
    let looks = [
        ("насыщенность и сочность", look_support::saturation_vibrance(0.6, 0.3)),
        ("полосы", look_support::all_bands(0.3, 0.4, -0.2)),
        (
            "таблица",
            look_support::with_lut(look_support::saturation_vibrance(0.3, 0.0), look_support::known_lut()),
        ),
    ];
    for (name, look) in looks {
        let still = assert_still_matches_reference_in_one_and_many_slices(
            &context,
            &mut capturer,
            &sweep.with_tone_base_and_look(base, look),
        );
        assert_ne!(still, neutral, "вид {name} не меняет снимок filmic");
    }
    assert_no_validation_errors(&context);
}

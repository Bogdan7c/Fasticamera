//! Тон вида (контраст, средние тона, тени, света) в проявлении СНИМКА (RGBA8) против эталона `isp-model` на всех четырёх
//! основах (AgX, ACES, filmic, прежняя): допуск 1 LSB и доля точных байтов ≥ 99,9 % — как у `photo_look.rs`. Что
//! проверяется: углы ручек (разные по знакам), каждая ручка на краю отдельно, тон вместе с цветом (колёса, насыщенность,
//! полосы, теплота, таблица-«единица»); полосы проявления (целый кадр и узкие) дают один и тот же снимок; тон меняет
//! снимок, а нулевые ручки тона — снимок основы бит в бит.

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
use isp_model::tone::aces::AcesSettings;
use isp_model::tone::agx_settings::AgxSettings;
use isp_model::tone::base::ToneBase;
use isp_model::tone::filmic::FilmicSettings;
use isp_vulkan::photo::{DevelopRequest, PhotoConfig, PhotoDeveloper, RawCapturer, RgbaReadback};
use look_contract::{BandAdjustments, LookSettings};
use look_support::{all_wheels, band, controls, identity_lut, knob, tone_corner_settings, tone_look, with_lut};
use photo_support::{
    Placement, SourceFrame, SourceKind, capture, compare_rgba, crop_in_buffer, develop_with, developer_for, real_frame,
    reference_rgba_any_base, upload, zone_plate_frame_at_level,
};
use quality_support::{assert_no_validation_errors, gpu_context_or_skip};
use still_contract::shot::StillRotation;

const MAX_LSB_DIFFERENCE: u8 = 1;
const MIN_EXACT_SHARE: f64 = 0.999;

fn four_bases() -> [(&'static str, ToneBase); 4] {
    [
        ("AgX", ToneBase::Agx(AgxSettings::default())),
        ("ACES", ToneBase::Aces(AcesSettings::default())),
        ("filmic", ToneBase::Filmic(FilmicSettings::default())),
        ("прежняя", ToneBase::Legacy),
    ]
}

/// Виды снимка: углы тона (по два в каждую сторону), каждая ручка на краях отдельно, тон с цветом.
fn looks() -> Vec<(String, LookControls)> {
    let mut looks: Vec<(String, LookControls)> = Vec::new();
    // Углы ручек: «всё в плюс», «всё в минус не считая обрыва у белого» (контраст+), чередования.
    for corner in [0b1111, 0b0001, 0b0101, 0b1010, 0b0110] {
        looks.push((format!("угол {corner:04b}"), controls(tone_corner_settings(corner))));
    }
    for (name, values) in [
        ("контраст", [1.0, 0.0, 0.0, 0.0]),
        ("средние", [0.0, 1.0, 0.0, 0.0]),
        ("тени", [0.0, 0.0, 1.0, 0.0]),
        ("света", [0.0, 0.0, 0.0, 1.0]),
    ] {
        for sign in [1.0_f32, -1.0] {
            let [contrast, midtones, shadows, highlights] = values.map(|value| value * sign);
            looks.push((
                format!("{name} {sign:+}"),
                tone_look(contrast, midtones, shadows, highlights),
            ));
        }
    }
    let tone = tone_corner_settings(0b0101);
    let with_tone = |colour: LookSettings| LookSettings {
        contrast: tone.contrast,
        midtones: tone.midtones,
        shadows: tone.shadows,
        highlights: tone.highlights,
        ..colour
    };
    looks.push((
        "тон + колёса + насыщенность".to_owned(),
        controls(with_tone(LookSettings {
            wheels: all_wheels(),
            saturation: knob(0.5),
            vibrance: knob(0.3),
            ..LookSettings::NEUTRAL
        })),
    ));
    looks.push((
        "тон + полосы HUE ±60 + теплота + таблица-единица".to_owned(),
        with_lut(
            controls(with_tone(LookSettings {
                bands: BandAdjustments::new(vec![band(0, 1.0, 0.5, -0.4), band(3, -1.0, -0.6, 0.5)]).unwrap(),
                warmth: knob(0.4),
                ..LookSettings::NEUTRAL
            })),
            identity_lut(),
        ),
    ));
    looks
}

fn develop_rgba(
    context: &gpu_vulkan::context::GpuContext,
    captured: &isp_vulkan::photo::RawCapture,
    frame: &SourceFrame,
    crop: PixelRect,
    slice_rows: u32,
) -> Vec<u8> {
    let mut developer: PhotoDeveloper = developer_for(context, slice_rows, captured.valid_area().width);
    let request = DevelopRequest {
        raw: captured,
        params: &frame.params,
        crop,
        readback: RgbaReadback::NotNeeded,
        rotation: StillRotation::None,
    };
    develop_with(context, &mut developer, &request, |_, _| {}).image_rgba
}

/// Возвращает `Err` с описанием, если снимок отличается от эталона больше допуска.
fn close_to_reference(name: &str, got: &[u8], want: &[u8], width: u32) -> Result<(), String> {
    let difference = compare_rgba(got, want, width);
    let exact_share = 1.0 - difference.differing_bytes as f64 / got.len() as f64;
    if difference.max_abs > MAX_LSB_DIFFERENCE || exact_share < MIN_EXACT_SHARE {
        Err(format!(
            "{name}: {} LSB (допуск {MAX_LSB_DIFFERENCE}), точно {:.4} % (нужно {:.1} %), первое {:?}",
            difference.max_abs,
            exact_share * 100.0,
            MIN_EXACT_SHARE * 100.0,
            difference.first
        ))
    } else {
        Ok(())
    }
}

/// Кадры снимка: два реальных кропа и зонная пластинка (цветовые переходы).
fn source_frames() -> Vec<SourceFrame> {
    vec![
        real_frame("real-main-lamp-jar"),
        real_frame("real-main-lamp-towel"),
        zone_plate_frame_at_level(96, 80, BayerPattern::RGGB, 1.0),
    ]
}

#[test]
fn tone_matches_reference_for_every_base_in_one_and_many_slices_and_changes_the_still() {
    let Some(context) = gpu_context_or_skip() else { return };
    let mut capturer = RawCapturer::new(&context, &PhotoConfig::default()).unwrap();
    let mut failures = Vec::new();
    for source in source_frames() {
        let size = source.raw.size();
        let gpu_frame = upload(&context, &source.raw, SourceKind::Raw16, Placement::PADDED);
        let captured = capture(&context, &mut capturer, &gpu_frame);
        let crop = crop_in_buffer(gpu_frame.valid_area, 0, 0, size.width, size.height);
        for (base_name, base) in four_bases() {
            let neutral = source.with_tone_base(base);
            let neutral_rgba = develop_rgba(&context, &captured, &neutral, crop, 4096);
            // Нулевые ручки тона: снимок основы бит в бит.
            let zeroed = source.with_tone_base_and_look(base, tone_look(0.0, 0.0, 0.0, 0.0));
            assert!(zeroed.params.look.tone.is_none());
            assert_eq!(
                develop_rgba(&context, &captured, &zeroed, crop, 4096),
                neutral_rgba,
                "{}/{base_name}: нулевые ручки тона ≠ снимок основы",
                source.name
            );
            for (look_name, look) in looks() {
                let frame = source.with_tone_base_and_look(base, look);
                assert!(frame.params.look.tone.is_some(), "{look_name}: стадия тона включена");
                let want = reference_rgba_any_base(&frame);
                for slice_rows in [4096, 16] {
                    let got = develop_rgba(&context, &captured, &frame, crop, slice_rows);
                    let name = format!("{}/{base_name}/{look_name}/полосы={slice_rows}", source.name);
                    if let Err(failure) = close_to_reference(&name, &got, &want, size.width) {
                        failures.push(failure);
                    }
                    // Тон меняет снимок: иначе паритет пуст (проверяем на зонной пластинке — у неё есть все яркости).
                    if slice_rows == 4096 && source.name.starts_with("zone") {
                        assert_ne!(
                            got, neutral_rgba,
                            "{}/{base_name}/{look_name}: тон не изменил снимок",
                            source.name
                        );
                    }
                }
            }
        }
    }
    assert_no_validation_errors(&context);
    assert!(
        failures.is_empty(),
        "тон в снимке: {} нарушений:\n  {}",
        failures.len(),
        failures.join("\n  ")
    );
}

#[test]
fn tone_slices_of_every_height_and_an_offset_crop_are_bit_identical() {
    let Some(context) = gpu_context_or_skip() else { return };
    let mut capturer = RawCapturer::new(&context, &PhotoConfig::default()).unwrap();
    let look = with_lut(tone_look(0.6, -0.5, 0.7, -0.4), identity_lut());
    for (base_name, base) in four_bases() {
        let frame =
            zone_plate_frame_at_level(96, 80, BayerPattern::GRBG, 1.0).with_tone_base_and_look(base, look.clone());
        for kind in [SourceKind::Raw16, SourceKind::Raw10] {
            let gpu_frame = upload(&context, &frame.raw, kind, Placement::PADDED);
            let captured = capture(&context, &mut capturer, &gpu_frame);
            // Область снимка не в углу: у полос свои смещения.
            let crop = crop_in_buffer(gpu_frame.valid_area, 20, 18, 40, 30);
            let whole = develop_rgba(&context, &captured, &frame, crop, 4096);
            for slice_rows in [2, 6, 8, 14, 30] {
                let sliced = develop_rgba(&context, &captured, &frame, crop, slice_rows);
                assert_eq!(
                    sliced, whole,
                    "{base_name}/{kind:?}: полосы {slice_rows} строк ≠ целый кадр"
                );
            }
        }
    }
    assert_no_validation_errors(&context);
}

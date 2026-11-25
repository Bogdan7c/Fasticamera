//! Сквозные проверки вида: синтетический RAW-кадр проходит весь эталонный конвейер до сигнала экрана.

use std::sync::Arc;

use camera_contract::geometry::PixelSize;

use super::white_tests::{controls, frame, sensor};
use super::*;
use crate::cfa::RgbChannel;
use crate::image::{DisplayEncodedImage, RawMosaic};
use crate::look::LookControls;
use crate::look::lut::LookLut;
use crate::look::model::LookModelSettings;
use crate::params::{IspParams, ProcessingControls};
use crate::pipeline::{run_reference, run_reference_encoded, run_reference_working};
use crate::statistics::{AwbStatistics, AwbStatisticsInput, AwbStatisticsSettings, compute_awb_statistics};
use crate::test_support::LAMP_NEUTRAL;

const SIZE: PixelSize = PixelSize::new(64, 48);
const BLACK: f32 = 64.0;
const WHITE_RANGE: f32 = 1023.0 - 64.0;

/// Кадр: левая половина — серый предмет (камера видит нейтраль AWB), правая — оранжевый.
fn raw_frame() -> RawMosaic {
    let sensor = sensor();
    let pattern = sensor.pattern();
    let mut samples = Vec::new();
    for y in 0..SIZE.height {
        for x in 0..SIZE.width {
            let channel = pattern.channel_at(x, y).index();
            let grey = LAMP_NEUTRAL[channel] as f32 * 0.35;
            let orange = grey * [1.5, 1.0, 0.45][channel];
            let level = if x < SIZE.width / 2 { grey } else { orange };
            samples.push((BLACK + level * WHITE_RANGE).round() as u16);
        }
    }
    let _ = RgbChannel::Red;
    RawMosaic::new(SIZE, pattern, samples).unwrap()
}

fn looked(settings: LookSettings) -> LookControls {
    LookControls::new(settings, Arc::new(LookModelSettings::default()))
}

fn params(controls: &ProcessingControls) -> IspParams {
    let sensor = sensor();
    IspParams::resolve(&sensor, &frame(&sensor), controls).unwrap()
}

fn statistics(raw: &RawMosaic, params: &IspParams) -> AwbStatistics {
    let run = run_reference_working(raw, params).unwrap();
    compute_awb_statistics(
        AwbStatisticsInput::new(&run.front_end.normalized, &run.front_end.shading_corrected).unwrap(),
        &AwbStatisticsSettings::default(),
    )
    .unwrap()
}

fn mean_chroma(image: &DisplayEncodedImage, from_column: u32, to_column: u32) -> f32 {
    let mut total = 0.0;
    let mut count = 0.0;
    for y in 0..image.size().height {
        for x in from_column..to_column {
            let pixel = image.pixel(x, y).map(crate::transfer::srgb_decode);
            total += lch_srgb(pixel);
            count += 1.0;
        }
    }
    total / count
}

/// Цветность OKLab для линейного sRGB.
fn lch_srgb(rgb: [f32; 3]) -> f32 {
    let workspace =
        crate::color::oklab_working::OklabWorkspace::new(&crate::color::chromaticity::Primaries::SRGB).unwrap();
    let [_, a, b] = workspace.to_lab(rgb);
    a.hypot(b)
}

#[test]
fn neutral_look_leaves_the_agx_picture_bit_for_bit_and_a_touched_one_changes_it() {
    let raw = raw_frame();
    let plain = run_reference_working(&raw, &params(&controls())).unwrap();
    let neutral = run_reference_working(&raw, &params(&controls().with_look(looked(LookSettings::NEUTRAL)))).unwrap();
    assert_eq!(
        neutral.working_looked, neutral.working_display,
        "нейтральный вид тронул Rec. 2020"
    );
    assert_eq!(neutral.display_encoded, plain.display_encoded);

    let vivid = run_reference_working(
        &raw,
        &params(&controls().with_look(looked(settings_with(|s| s.saturation = knob(0.8))))),
    )
    .unwrap();
    // Доходит до конца: итоговый сигнал экрана у оранжевой половины стал цветнее, у серой остался серым.
    let orange_before = mean_chroma(&plain.display_encoded, 40, 60);
    let orange_after = mean_chroma(&vivid.display_encoded, 40, 60);
    assert!(orange_after > orange_before * 1.5, "{orange_before} -> {orange_after}");
    assert!(
        mean_chroma(&vivid.display_encoded, 4, 28) < 2e-3,
        "серый окрасился насыщенностью"
    );
}

#[test]
fn look_knobs_do_not_change_ae_or_awb_inputs() {
    let raw = raw_frame();
    let honest = params(&controls());
    let styled = params(&controls().with_look(looked(settings_with(|s| {
        s.saturation = knob(1.0);
        s.contrast = knob(1.0);
        s.midtones = knob(-0.8);
        s.shadows = knob(0.9);
        s.highlights = knob(-0.9);
        s.warmth = knob(-0.7);
        s.tint = knob(0.5);
        s.bands = band_adjustment("orange", 1.0, 1.0, 1.0);
    }))));
    // Картинка при этом другая (тест не пустой).
    assert_ne!(
        run_reference_working(&raw, &honest).unwrap().display_encoded,
        run_reference_working(&raw, &styled).unwrap().display_encoded
    );
    assert_eq!(
        statistics(&raw, &honest),
        statistics(&raw, &styled),
        "стиль виден статистике AWB"
    );
    assert_eq!(
        honest.scene_luminance_weights(),
        styled.scene_luminance_weights(),
        "стиль виден AE"
    );
}

#[test]
fn legacy_base_gets_the_colour_and_tone_stages_and_keeps_pixels_in_range() {
    let raw = raw_frame();
    let legacy_controls = controls().with_legacy_tone();
    let plain = run_reference(&raw, &params(&legacy_controls)).unwrap();
    let neutral = run_reference(
        &raw,
        &params(&legacy_controls.clone().with_look(looked(LookSettings::NEUTRAL))),
    )
    .unwrap();
    assert_eq!(neutral.display_linear, plain.display_linear);

    let settings = settings_with(|s| {
        s.saturation = knob(1.0);
        s.shadows = knob(0.5);
    });
    let styled_params = params(&legacy_controls.with_look(looked(settings)));
    let styled = run_reference(&raw, &styled_params).unwrap();
    assert_ne!(styled.display_linear, plain.display_linear);
    assert!(
        styled
            .display_linear
            .pixels()
            .iter()
            .flatten()
            .all(|c| (0.0..=1.0).contains(c))
    );
    assert!(mean_chroma(&styled.display_encoded, 40, 60) > mean_chroma(&plain.display_encoded, 40, 60) * 1.3);
}

/// Кадр с двумя серыми половинами: левая тёмная (ниже серого экрана), правая светлая (выше него). Уровни — доли
/// белого сенсора под лампой AWB.
fn two_tone_frame(dark_level: f64, bright_level: f64) -> RawMosaic {
    let sensor = sensor();
    let pattern = sensor.pattern();
    let mut samples = Vec::new();
    for y in 0..SIZE.height {
        for x in 0..SIZE.width {
            let channel = pattern.channel_at(x, y).index();
            let level = if x < SIZE.width / 2 { dark_level } else { bright_level };
            let grey = LAMP_NEUTRAL[channel] as f32 * level as f32;
            samples.push((BLACK + grey * WHITE_RANGE).round() as u16);
        }
    }
    RawMosaic::new(SIZE, pattern, samples).unwrap()
}

/// Средний сигнал экрана (канал G) в колонках `from_column..to_column`.
fn mean_signal(image: &DisplayEncodedImage, from_column: u32, to_column: u32) -> f32 {
    let mut total = 0.0;
    let mut count = 0.0;
    for y in 0..image.size().height {
        for x in from_column..to_column {
            total += image.pixel(x, y)[1];
            count += 1.0;
        }
    }
    total / count
}

/// Раньше у прежней основы тени и света были недоступны (шов основы их не поддерживал). Теперь все четыре ручки тона
/// — стадия вида после основы, и прежняя основа получает их сквозным путём до сигнала экрана.
#[test]
fn legacy_base_gets_the_shadows_knob_and_it_moves_only_the_dark_part() {
    let raw = two_tone_frame(0.01, 0.9);
    let legacy = controls().with_legacy_tone();
    let plain = run_reference(&raw, &params(&legacy)).unwrap().display_encoded;
    let lifted = run_reference(
        &raw,
        &params(
            &legacy
                .clone()
                .with_look(looked(settings_with(|s| s.shadows = knob(1.0)))),
        ),
    )
    .unwrap()
    .display_encoded;
    let dark_before = mean_signal(&plain, 4, 28);
    let dark_after = mean_signal(&lifted, 4, 28);
    assert!(
        dark_after > dark_before + 0.05,
        "тёмная часть не посветлела: {dark_before} -> {dark_after}"
    );
    let bright_before = mean_signal(&plain, 36, 60);
    let bright_after = mean_signal(&lifted, 36, 60);
    assert!(
        (bright_after - bright_before).abs() < 2e-3,
        "светлая часть сдвинулась от теней: {bright_before} -> {bright_after}"
    );
    // Зеркально: света трогают светлую часть и не трогают тёмную.
    let darker_highlights = run_reference(
        &raw,
        &params(&legacy.with_look(looked(settings_with(|s| s.highlights = knob(-1.0))))),
    )
    .unwrap()
    .display_encoded;
    assert!(mean_signal(&darker_highlights, 36, 60) < bright_before - 0.02);
    assert!((mean_signal(&darker_highlights, 4, 28) - dark_before).abs() < 2e-3);
}

#[test]
fn identity_lut_changes_nothing_and_a_channel_swap_lut_swaps_the_channels_end_to_end() {
    let raw = raw_frame();
    let legacy = controls().with_legacy_tone();
    let plain = run_reference(&raw, &params(&legacy)).unwrap();

    let identity = LookControls::default().with_lut(LookLut::identity(17, 65).unwrap());
    let with_identity = run_reference(&raw, &params(&legacy.clone().with_look(identity))).unwrap();
    for (before, after) in plain
        .display_linear
        .pixels()
        .iter()
        .zip(with_identity.display_linear.pixels())
    {
        for channel in 0..3 {
            assert!(
                (before[channel] - after[channel]).abs() < 2e-5,
                "{before:?} -> {after:?}"
            );
        }
    }

    // Перестановка R ↔ B в закодированном домене; у прежней основы `W` = цвета выхода, значит поменяются
    // ровно каналы итогового света.
    let size = 9_u32;
    let mut nodes = Vec::new();
    for blue in 0..size {
        for green in 0..size {
            for red in 0..size {
                let last = (size - 1) as f32;
                nodes.push([blue as f32 / last, green as f32 / last, red as f32 / last]);
            }
        }
    }
    let swap = LookControls::default().with_lut(LookLut::new(size, nodes, 65).unwrap());
    let swapped = run_reference(&raw, &params(&legacy.with_look(swap))).unwrap();
    for (before, after) in plain
        .display_linear
        .pixels()
        .iter()
        .zip(swapped.display_linear.pixels())
    {
        assert!(
            (before[0] - after[2]).abs() < 3e-5
                && (before[2] - after[0]).abs() < 3e-5
                && (before[1] - after[1]).abs() < 3e-5
        );
    }
    // Сквозной путь на любой основе доходит до сигнала экрана.
    let encoded = run_reference_encoded(&raw, &params(&controls())).unwrap();
    assert_eq!(encoded.size(), SIZE);
}

#[test]
fn a_lut_larger_than_the_model_limit_is_refused_with_a_distinct_error() {
    let lut = LookLut::identity(33, 65).unwrap();
    let model = LookModelSettings {
        max_lut_size: 17,
        ..LookModelSettings::default()
    };
    let controls = controls().with_look(LookControls::new(LookSettings::NEUTRAL, Arc::new(model)).with_lut(lut));
    let sensor = sensor();
    let error = IspParams::resolve(&sensor, &frame(&sensor), &controls).unwrap_err();
    assert!(error.to_string().contains("предела модели 17"), "{error}");
}

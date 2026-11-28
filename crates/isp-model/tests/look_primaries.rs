//! Приёмка полосок Lift / Gamma / Gain / Offset по R, G, B (решения 339–343) через настоящие модели основы и
//! настоящий кадр: AgX, ACES, filmic и прежняя основа на реальном кропе (`real-main-lamp-*`). Полоски доходят до
//! итогового сигнала и ведут себя по зонам: Gain R «+» краснит света сильнее теней, Lift B «+» синит тени сильнее
//! светов; нейтральные полоски не меняют кадр ни на бит.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::print_stderr,
    reason = "интеграционный тест: падение — способ сообщить о провале, числа печатаются для отчёта"
)]

mod support;

use std::sync::Arc;

use isp_model::color::chromaticity::Primaries;
use isp_model::demosaic::DemosaicMethod;
use isp_model::image::{DisplayGamut, DisplayLinearImage};
use isp_model::look::LookControls;
use isp_model::look::model::LookModelSettings;
use isp_model::params::{FrameInputs, IspParams, ProcessingControls};
use isp_model::pipeline::{run_reference, run_reference_working};
use isp_model::tone::aces::AcesSettings;
use isp_model::tone::agx_settings::AgxSettings;
use isp_model::tone::base::ToneBase;
use isp_model::tone::filmic::FilmicSettings;
use isp_model::transfer::srgb_encode;
use look_contract::{ChannelPrimaries, LookKnob, LookSettings, PrimaryChannel, WheelZone};
use support::real_capture::{RealCapture, load_real_capture};

/// Кадр с большим разбросом яркости: тени и света в одном кропе.
const CROP: &str = "real-main-lamp-jar";
/// Ниже этого значения канала отношение «стало / было» шумит (пиксель почти чёрный) — такой пиксель не считается.
const RATIO_FLOOR: f32 = 2e-3;

fn knob(value: f32) -> LookKnob {
    LookKnob::new(value).unwrap()
}

/// Основы: имя и выбор.
fn bases() -> [(&'static str, ToneBase); 4] {
    [
        ("legacy", ToneBase::Legacy),
        ("agx", ToneBase::Agx(AgxSettings::default())),
        ("aces", ToneBase::Aces(AcesSettings::default())),
        ("filmic", ToneBase::Filmic(FilmicSettings::default())),
    ]
}

/// Итоговый свет экрана кропа с видом `settings` на основе `base`.
fn render(capture: &RealCapture, base: ToneBase, settings: LookSettings) -> DisplayLinearImage {
    let frame = FrameInputs {
        levels: capture.sensor.static_levels(),
    };
    let controls = ProcessingControls::with_defaults(capture.neutral, DemosaicMethod::Quality, DisplayGamut::Srgb)
        .with_look(LookControls::new(settings, Arc::new(LookModelSettings::default())));
    let controls = ProcessingControls {
        tone_base: base,
        ..controls
    };
    let params = IspParams::resolve(&capture.sensor, &frame, &controls).unwrap();
    match base {
        ToneBase::Legacy => run_reference(&capture.raw, &params).unwrap().display_linear,
        _ => run_reference_working(&capture.raw, &params).unwrap().display_linear,
    }
}

fn with_bar(zone: WheelZone, channel: PrimaryChannel, value: f32) -> LookSettings {
    LookSettings {
        primaries: ChannelPrimaries::NEUTRAL.with_value(zone, channel, knob(value)),
        ..LookSettings::NEUTRAL
    }
}

/// Яркость пикселя экрана (веса sRGB).
fn luma(pixel: [f32; 3]) -> f32 {
    0.2126 * pixel[0] + 0.7152 * pixel[1] + 0.0722 * pixel[2]
}

/// Средний сдвиг КОДА экрана (sRGB-кодирование, как видит человек и parade) канала `channel` у трети самых тёмных и
/// трети самых светлых пикселей кропа (по яркости до). Пиксели с каналом у границ охвата не считаются: их двигает
/// обрезка выхода, а не полоска. Сдвиг линейного света тут не годится: Gain в шкале кодирования — почти
/// умножение, и относительно он одинаков в тенях и светах; зоны видны в кодах.
fn code_shift_in_shadows_and_highlights(
    before: &DisplayLinearImage,
    after: &DisplayLinearImage,
    channel: usize,
) -> (f32, f32) {
    let mut counted: Vec<(f32, f32)> = before
        .pixels()
        .iter()
        .zip(after.pixels())
        .filter(|(old, new)| {
            [old[channel], new[channel]]
                .iter()
                .all(|value| *value > RATIO_FLOOR && *value < 1.0 - RATIO_FLOOR)
        })
        .map(|(old, new)| (luma(*old), srgb_encode(new[channel]) - srgb_encode(old[channel])))
        .collect();
    counted.sort_by(|first, second| first.0.total_cmp(&second.0));
    let third = counted.len() / 3;
    assert!(third > 100, "слишком мало пикселей для сравнения: {}", counted.len());
    let mean = |slice: &[(f32, f32)]| slice.iter().map(|(_, shift)| shift).sum::<f32>() / slice.len() as f32;
    (mean(&counted[..third]), mean(&counted[counted.len() - third..]))
}

#[test]
fn gain_red_reddens_highlights_more_than_shadows_on_every_base() {
    let capture = load_real_capture(&support::assets_dir(), CROP);
    for (name, base) in bases() {
        let before = render(&capture, base, LookSettings::NEUTRAL);
        let after = render(&capture, base, with_bar(WheelZone::Gain, PrimaryChannel::Red, 1.0));
        let (shadows, highlights) = code_shift_in_shadows_and_highlights(&before, &after, 0);
        eprintln!("gain R +1  {name:7} | код R: тени {shadows:+.4}, света {highlights:+.4}");
        assert!(highlights > 0.02, "{name}: света не покраснели: {highlights}");
        assert!(
            highlights > 1.5 * shadows.max(0.0) + 0.01,
            "{name}: тени {shadows} не слабее светов {highlights}"
        );
    }
}

#[test]
fn lift_blue_blues_shadows_more_than_highlights_on_every_base() {
    let capture = load_real_capture(&support::assets_dir(), CROP);
    for (name, base) in bases() {
        let before = render(&capture, base, LookSettings::NEUTRAL);
        let after = render(&capture, base, with_bar(WheelZone::Lift, PrimaryChannel::Blue, 1.0));
        let (shadows, highlights) = code_shift_in_shadows_and_highlights(&before, &after, 2);
        eprintln!("lift B +1  {name:7} | код B: тени {shadows:+.4}, света {highlights:+.4}");
        assert!(shadows > 0.02, "{name}: тени не посинели: {shadows}");
        assert!(
            shadows > 1.2 * highlights.max(0.0),
            "{name}: света {highlights} не слабее теней {shadows}"
        );
    }
}

#[test]
fn neutral_bars_leave_the_whole_frame_bit_for_bit_on_every_base() {
    let capture = load_real_capture(&support::assets_dir(), CROP);
    for (name, base) in bases() {
        let plain = render(&capture, base, LookSettings::NEUTRAL);
        let neutral_bars = render(
            &capture,
            base,
            LookSettings {
                primaries: ChannelPrimaries::NEUTRAL,
                ..LookSettings::NEUTRAL
            },
        );
        assert_eq!(plain.pixels(), neutral_bars.pixels(), "{name}");
        // И стадия не пустая: любая тронутая полоска меняет кадр.
        let touched = render(&capture, base, with_bar(WheelZone::Offset, PrimaryChannel::Green, 0.5));
        assert_ne!(plain.pixels(), touched.pixels(), "{name}: полоска не дошла до кадра");
    }
}

#[test]
fn a_green_bar_moves_only_the_green_trace_of_the_p3_basis_in_the_frame() {
    // Выход кропа — sRGB, поэтому красный и синий P3-следы считаем независимой матрицей `sRGB → P3`. Мягкое
    // сжатие охвата и обрезка выхода у AgX/ACES/filmic дают небольшие поправки; Gain G «+» на полной силе
    // двигает зелёный след на порядок сильнее.
    let capture = load_real_capture(&support::assets_dir(), CROP);
    let to_p3 = Primaries::DISPLAY_P3
        .rgb_to_xyz()
        .unwrap()
        .inverse()
        .unwrap()
        .mul(&Primaries::SRGB.rgb_to_xyz().unwrap())
        .to_f32();
    for (name, base) in bases() {
        let before = render(&capture, base, LookSettings::NEUTRAL);
        let after = render(&capture, base, with_bar(WheelZone::Gain, PrimaryChannel::Green, 1.0));
        let mut mean_change = [0.0_f64; 3];
        let mut counted = 0.0_f64;
        for (old, new) in before.pixels().iter().zip(after.pixels()) {
            let (old_p3, new_p3) = (to_p3.apply(*old), to_p3.apply(*new));
            // Пиксели у границ охвата выхода обрезаются: их красный след двигает обрезка, а не полоска.
            if old
                .iter()
                .chain(new)
                .any(|channel| *channel <= 1e-3 || *channel >= 0.999)
            {
                continue;
            }
            for channel in 0..3 {
                mean_change[channel] += f64::from((new_p3[channel] - old_p3[channel]).abs());
            }
            counted += 1.0;
        }
        assert!(counted > 500.0, "{name}: мало пикселей внутри охвата: {counted}");
        let [red, green, blue] = mean_change.map(|sum| sum / counted);
        eprintln!("gain G +1  {name:7} | средний |dP3|: R {red:.5}, G {green:.5}, B {blue:.5}");
        assert!(green > 0.01, "{name}: зелёный след не сдвинулся: {green}");
        assert!(
            red < 0.1 * green && blue < 0.1 * green,
            "{name}: чужой след задет: R {red}, B {blue}, G {green}"
        );
    }
}

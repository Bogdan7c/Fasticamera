//! Сквозной тест эталона: синтетический RAW доходит до закодированного сигнала экрана,
//! и итог совпадает с формулами всех стадий.
//!
//! Кадр: серая карта под заданным светом (RAW = black + яркость · нейтраль · (white − black)),
//! пересвеченный блик (все каналы = white) и насыщенный цветной объект. Калибровка — реальные
//! DNG-матрицы основной камеры OnePlus 11 (из фикстуры C7).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "тесты: падение и есть результат проверки"
)]

mod support;

use camera_contract::geometry::PixelSize;
use isp_model::cfa::{BayerPattern, RgbChannel};
use isp_model::color::dng::CameraNeutral;
use isp_model::demosaic::DemosaicMethod;
use isp_model::image::{DisplayGamut, RawMosaic};
use isp_model::metrics::display_linear_to_lab;
use isp_model::params::{FrameInputs, IspParams, ProcessingControls};
use isp_model::pipeline::{ReferenceRun, run_reference};
use isp_model::sensor::SensorDescription;
use isp_model::transfer::srgb_encode;
use support::real_capture::load_real_capture;

const SIDE: u32 = 64;
/// Яркость серой карты в нормированном свете сенсора (после баланса Y = эта величина).
const GRAY_CARD_LEVEL: f64 = 0.15;
const BLACK: f64 = 64.0;
const WHITE: f64 = 1023.0;

/// Кадр: левая половина — серая карта, правый верх — пересвет, правый низ — цветной объект.
fn synthetic_raw(neutral: [f64; 3], colored_camera_rgb: [f64; 3]) -> RawMosaic {
    let pattern = BayerPattern::BGGR;
    let samples = (0..SIDE * SIDE)
        .map(|index| {
            let (x, y) = (index % SIDE, index / SIDE);
            let channel = pattern.channel_at(x, y).index();
            let linear = if x < SIDE / 2 {
                GRAY_CARD_LEVEL * neutral[channel]
            } else if y < SIDE / 2 {
                1.0
            } else {
                colored_camera_rgb[channel]
            };
            (BLACK + linear * (WHITE - BLACK)).round() as u16
        })
        .collect();
    RawMosaic::new(PixelSize::new(SIDE, SIDE), pattern, samples).unwrap()
}

fn calibrated_sensor() -> SensorDescription {
    load_real_capture(&support::assets_dir(), "real-main-lamp-gray-tile").sensor
}

fn process(
    raw: &RawMosaic,
    neutral: [f64; 3],
    demosaic: DemosaicMethod,
    gamut: DisplayGamut,
) -> (ReferenceRun, IspParams) {
    let sensor = calibrated_sensor();
    let frame = FrameInputs {
        levels: sensor.static_levels(),
    };
    let controls = ProcessingControls::with_defaults(CameraNeutral::new(neutral).unwrap(), demosaic, gamut);
    let params = IspParams::resolve(&sensor, &frame, &controls).unwrap();
    (run_reference(raw, &params).unwrap(), params)
}

/// Центр серой карты в координатах выхода (у Preview выход вдвое меньше).
fn gray_card_center(run: &ReferenceRun) -> (u32, u32) {
    let size = run.display_encoded.size();
    (size.width / 4, size.height / 2)
}

const LAMP: [f64; 3] = [0.515625, 1.0, 0.4990234375];
const DAYLIGHT: [f64; 3] = [0.46, 1.0, 0.68];
const SHADE: [f64; 3] = [0.40, 1.0, 0.85];
/// Нейтраль, где самый яркий канал — красный (очень тёплый свет): баланс нормируется не по G.
const RED_BRIGHTEST: [f64; 3] = [1.3, 1.0, 0.6];

#[test]
fn gray_card_stays_gray_under_any_light_on_both_paths_and_gamuts() {
    for neutral in [LAMP, DAYLIGHT, SHADE, RED_BRIGHTEST] {
        let raw = synthetic_raw(neutral, [0.3, 0.05, 0.02]);
        for demosaic in [DemosaicMethod::Preview, DemosaicMethod::Quality] {
            for gamut in [DisplayGamut::Srgb, DisplayGamut::DisplayP3] {
                let (run, _) = process(&raw, neutral, demosaic, gamut);
                let (x, y) = gray_card_center(&run);
                let lab = display_linear_to_lab(run.display_linear.pixel(x, y), gamut).unwrap();
                let chroma = lab.a.hypot(lab.b);
                // Квантование RAW до 10 бит даёт ошибку отношения каналов ~1e-3 → цветность < 0,3 ΔE.
                assert!(
                    chroma < 0.3,
                    "{neutral:?} {demosaic:?} {gamut:?}: a* {} b* {}",
                    lab.a,
                    lab.b
                );
            }
        }
    }
}

#[test]
fn gray_card_brightness_follows_tone_and_transfer_formulas() {
    for neutral in [LAMP, RED_BRIGHTEST] {
        let raw = synthetic_raw(neutral, [0.3, 0.05, 0.02]);
        let (run, params) = process(&raw, neutral, DemosaicMethod::Quality, DisplayGamut::DisplayP3);
        let (x, y) = gray_card_center(&run);
        // После баланса (усиления max(N)/N) серый = яркость карты · max(N); это и есть Y сцены.
        let brightest = neutral.into_iter().fold(f64::MIN, f64::max);
        let expected = srgb_encode(params.tone.map_channel((GRAY_CARD_LEVEL * brightest) as f32));
        for channel in run.display_encoded.pixel(x, y) {
            assert!(
                (channel - expected).abs() < 2e-3,
                "{neutral:?}: {channel} против {expected}"
            );
        }
    }
}

#[test]
fn clipped_highlight_is_white_not_pink() {
    let raw = synthetic_raw(LAMP, [0.3, 0.05, 0.02]);
    for demosaic in [DemosaicMethod::Preview, DemosaicMethod::Quality] {
        let (run, _) = process(&raw, LAMP, demosaic, DisplayGamut::DisplayP3);
        let size = run.display_encoded.size();
        let highlight = run.display_encoded.pixel(size.width * 3 / 4, size.height / 4);
        for channel in highlight {
            assert!((channel - 1.0).abs() < 1e-4, "{demosaic:?}: блик {highlight:?}");
        }
        // Без обрезки после баланса красный и синий были бы ~2× зелёного.
        let balanced = run.white_balanced.sample(SIDE * 3 / 4, SIDE / 4);
        assert_eq!(balanced, 1.0);
    }
}

/// Оттенок в шестигранной модели HSV, градусы.
fn hexcone_hue_degrees([red, green, blue]: [f32; 3]) -> f32 {
    let max = red.max(green).max(blue);
    let span = max - red.min(green).min(blue);
    let sector = if max == red {
        ((green - blue) / span).rem_euclid(6.0)
    } else if max == green {
        (blue - red) / span + 2.0
    } else {
        (red - green) / span + 4.0
    };
    sector * 60.0
}

#[test]
fn saturated_object_keeps_its_hue_through_tone_and_stays_in_range() {
    let raw = synthetic_raw(LAMP, [0.3, 0.05, 0.02]);
    let (run, _) = process(&raw, LAMP, DemosaicMethod::Quality, DisplayGamut::Srgb);
    let size = run.display_linear.size();
    let (x, y) = (size.width * 3 / 4, size.height * 3 / 4);
    let scene = run.scene_in_gamut.pixel(x, y);
    let display = run.display_linear.pixel(x, y);
    // Решение 47: тон сохраняет оттенок на 100 % — тот же оттенок HSV до и после кривой.
    let hue_shift = (hexcone_hue_degrees(display) - hexcone_hue_degrees(scene)).abs();
    assert!(
        hue_shift < 0.1,
        "сдвиг оттенка {hue_shift}°: сцена {scene:?}, экран {display:?}"
    );
    assert!(
        display[0] > display[1] && display[1] > display[2],
        "красно-оранжевый объект: {display:?}"
    );
    for channel in run.display_encoded.pixel(x, y) {
        assert!((0.0..=1.0).contains(&channel));
    }
    // Охват: после сжатия ни одного отрицательного света.
    assert!(
        run.scene_in_gamut
            .pixels()
            .iter()
            .flatten()
            .all(|&channel| channel >= 0.0)
    );
}

#[test]
fn preview_reaches_display_at_half_resolution_and_quality_at_full() {
    let raw = synthetic_raw(DAYLIGHT, [0.05, 0.2, 0.3]);
    let (preview, _) = process(&raw, DAYLIGHT, DemosaicMethod::Preview, DisplayGamut::DisplayP3);
    let (quality, _) = process(&raw, DAYLIGHT, DemosaicMethod::Quality, DisplayGamut::DisplayP3);
    assert_eq!(preview.display_encoded.size(), PixelSize::new(SIDE / 2, SIDE / 2));
    assert_eq!(quality.display_encoded.size(), PixelSize::new(SIDE, SIDE));
    assert_eq!(preview.display_encoded.space().gamut, DisplayGamut::DisplayP3);
    // Внутри ровной серой области оба пути дают одно и то же.
    let (px, py) = gray_card_center(&preview);
    let (qx, qy) = gray_card_center(&quality);
    for channel in 0..3 {
        let difference =
            preview.display_encoded.pixel(px, py)[channel] - quality.display_encoded.pixel(qx, qy)[channel];
        assert!(difference.abs() < 1e-4);
    }
}

#[test]
fn white_balance_gain_matches_neutral_in_pipeline() {
    let raw = synthetic_raw(SHADE, [0.1, 0.1, 0.1]);
    let (run, params) = process(&raw, SHADE, DemosaicMethod::Preview, DisplayGamut::Srgb);
    assert!((params.white_balance.gain(RgbChannel::Blue) - (1.0 / 0.85) as f32).abs() < 1e-6);
    // Синий пиксель серой карты (0,0 у BGGR) после баланса = яркость карты.
    assert!((f64::from(run.white_balanced.sample(0, 0)) - GRAY_CARD_LEVEL).abs() < 2e-3);
}

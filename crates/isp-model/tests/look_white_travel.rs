//! Теплота и оттенок через полное разрешение кадра (`IspParams::resolve`) с настоящим DNG-профилем телефона: ручка
//! обязана менять картинку на ВСЁМ ходу при холодном, обычном и тёплом свете и ни при каком сочетании не должна
//! «отваливаться» (`WhiteShiftLimit::NotApplied` — кадр молча рисуется без ручки). Решение 246, пункт 2 сессии 14l.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "интеграционный тест: падение — способ сообщить о провале"
)]

mod support;

use std::sync::Arc;

use isp_model::color::dng::CameraNeutral;
use isp_model::color::temperature::Kelvin;
use isp_model::color::white_point::{CctDuv, Duv, chromaticity_from_cct_duv};
use isp_model::demosaic::DemosaicMethod;
use isp_model::image::DisplayGamut;
use isp_model::look::LookControls;
use isp_model::look::model::LookModelSettings;
use isp_model::look::white::WhiteShiftLimit;
use isp_model::params::{FrameInputs, IspParams, ProcessingControls};
use look_contract::{LookKnob, LookSettings};
use support::assets_dir;
use support::real_capture::{RealCapture, load_real_capture};

/// Света сцены: свеча/лампа, дневной, тень, синее небо.
const LIGHTS_KELVIN: [f64; 7] = [2000.0, 2850.0, 4000.0, 5500.0, 6600.0, 8000.0, 12_000.0];

fn neutral_of(capture: &RealCapture, kelvin: f64) -> CameraNeutral {
    let white = chromaticity_from_cct_duv(CctDuv {
        temperature: Kelvin(kelvin),
        duv: Duv(0.0),
    })
    .unwrap();
    capture.sensor.color_profile().neutral_for_white(white).unwrap()
}

fn resolve(capture: &RealCapture, kelvin: f64, warmth: f32, tint: f32) -> IspParams {
    let settings = LookSettings {
        warmth: LookKnob::new(warmth).unwrap(),
        tint: LookKnob::new(tint).unwrap(),
        ..LookSettings::NEUTRAL
    };
    let controls = ProcessingControls::with_defaults(
        neutral_of(capture, kelvin),
        DemosaicMethod::Preview,
        DisplayGamut::DisplayP3,
    )
    .with_look(LookControls::new(settings, Arc::new(LookModelSettings::default())));
    let frame = FrameInputs {
        levels: capture.sensor.static_levels(),
    };
    IspParams::resolve(&capture.sensor, &frame, &controls).unwrap()
}

/// Отношение усилений красного и синего рисования: монотонная мера «теплее / холоднее».
fn red_to_blue(params: &IspParams) -> f32 {
    let gains = params.white_balance.as_array();
    gains[0] / gains[2]
}

#[test]
fn warmth_never_falls_off_and_always_moves_the_rendering_under_any_light() {
    let capture = load_real_capture(&assets_dir(), "real-main-lamp-towel");
    for kelvin in LIGHTS_KELVIN {
        let mut previous: Option<f32> = None;
        for step in -10..=10 {
            let params = resolve(&capture, kelvin, step as f32 / 10.0, 0.0);
            let limit = params.look.report.white_shift_limit;
            assert!(
                !matches!(limit, WhiteShiftLimit::NotApplied(_)),
                "{kelvin} К, теплота {step}0: ручка отвалилась, {limit:?}"
            );
            let ratio = red_to_blue(&params);
            if let Some(previous) = previous {
                assert!(
                    ratio > previous * 1.002,
                    "{kelvin} К, теплота {step}0: R/B {ratio} после {previous} — мёртвый участок"
                );
            }
            previous = Some(ratio);
        }
    }
}

#[test]
fn warmth_and_tint_together_never_fall_off_at_any_corner() {
    let capture = load_real_capture(&assets_dir(), "real-main-lamp-towel");
    for kelvin in LIGHTS_KELVIN {
        for warmth in [-1.0, -0.5, 0.0, 0.5, 1.0] {
            let mut previous: Option<f32> = None;
            for step in -5..=5 {
                let tint = step as f32 / 5.0;
                let params = resolve(&capture, kelvin, warmth, tint);
                let limit = params.look.report.white_shift_limit;
                assert!(
                    !matches!(limit, WhiteShiftLimit::NotApplied(_)),
                    "{kelvin} К, теплота {warmth}, оттенок {tint}: ручка отвалилась, {limit:?}"
                );
                // Больше оттенка — пурпурнее: зелёного относительно красного и синего меньше.
                let gains = params.white_balance.as_array();
                let green_share = gains[1] / (gains[0] * gains[2]).sqrt();
                if let Some(previous) = previous {
                    // Упёршаяся в пригодное ручка стоит на месте (это видно в исходе), но назад не прыгает.
                    let at_the_stop = matches!(limit, WhiteShiftLimit::ReducedToUsable { .. });
                    assert!(
                        green_share < previous || (at_the_stop && green_share <= previous * 1.001),
                        "{kelvin} К, теплота {warmth}, оттенок {tint}: зелёное {green_share} после {previous}"
                    );
                }
                previous = Some(green_share);
            }
        }
    }
}

/// Упор в пригодное — редкий угол (тёплая лампа, теплота в минус, оттенок в край), он назван в исходе и плавен:
/// соседние значения ручки около упора дают соседние картинки, а не прыжок к несдвинутой белой.
#[test]
fn the_stop_at_the_usable_white_is_reported_and_has_no_jump() {
    let capture = load_real_capture(&assets_dir(), "real-main-lamp-towel");
    let at_the_edge = resolve(&capture, 2000.0, -1.0, 1.0);
    assert!(
        matches!(
            at_the_edge.look.report.white_shift_limit,
            WhiteShiftLimit::ReducedToUsable { fraction } if (0.05..0.95).contains(&fraction)
        ),
        "{:?}",
        at_the_edge.look.report.white_shift_limit
    );
    let mut previous: Option<[f32; 3]> = None;
    for step in 0..=100 {
        let gains = resolve(&capture, 2000.0, -1.0, step as f32 / 100.0)
            .white_balance
            .as_array();
        if let Some(previous) = previous {
            for channel in 0..3 {
                let ratio = gains[channel] / previous[channel];
                assert!(
                    (0.6..1.7).contains(&ratio),
                    "оттенок {step}: канал {channel} прыгнул {} → {}",
                    previous[channel],
                    gains[channel]
                );
            }
        }
        previous = Some(gains);
    }
}

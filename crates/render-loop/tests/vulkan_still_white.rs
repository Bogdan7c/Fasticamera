//! Снимок и белая (RADV, headless, полная validation): DNG `AsShotNeutral` соответствует той белой, что была
//! применена к показанному кадру (указание владельца), а ручки вида WARMTH/TINT нейтраль DNG не двигают.

#![cfg(not(target_os = "android"))]
#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть сигнал о провале"
)]

mod continuity_rig;
mod still_rig;
mod support;
mod vulkan_support;

use camera_contract::control::{ColorTemperature, Tint, WhiteBalanceSetting};
use camera_contract::white::{WhiteDirective, WhiteTransition};
use continuity_rig::{calibration, finish};
use isp_model::color::dng::DngColorProfile;
use isp_model::color::temperature::Kelvin;
use isp_model::color::white_point::{CctDuv, Duv, chromaticity_from_cct_duv};
use render_loop::config::StillDevelopConfig;
use still_contract::develop::DevelopEvent;
use still_rig::{JPEG_AND_DNG, plan, press_during, setup};

/// Нейтраль камеры стенда для белой `kelvin`, нормированная по наибольшему каналу (так её пишет DNG).
fn expected_neutral(kelvin: f64) -> [f64; 3] {
    let profile = DngColorProfile::from_calibration_sets(&calibration()).expect("профиль цвета");
    let white = chromaticity_from_cct_duv(CctDuv {
        temperature: Kelvin(kelvin),
        duv: Duv(0.0),
    })
    .expect("белая на кривой Планка");
    let neutral = profile.neutral_for_white(white).expect("нейтраль").values();
    let maximum = neutral.iter().copied().fold(f64::MIN_POSITIVE, f64::max);
    neutral.map(|channel| channel / maximum)
}

fn secured_neutral(new_white: Option<f64>) -> Option<[f64; 3]> {
    let config = StillDevelopConfig::default();
    let mut bench = setup(config, 12)?;
    bench.frames(10);
    bench.inject_timings();
    if let Some(kelvin) = new_white {
        // Владелец сменил белую (кнопка света): с ближайшего кадра ISP считает с ней.
        bench.rig.white.directives.publish(WhiteDirective {
            white: WhiteBalanceSetting {
                temperature: ColorTemperature::from_kelvin(kelvin).expect("температура"),
                tint: Tint::NEUTRAL,
            },
            transition: WhiteTransition::Immediate,
            request: None,
        });
    }
    bench.frames(8);
    bench.inject_timings();
    let sink = bench.begin(plan(1, press_during(14), JPEG_AND_DNG));
    assert!(bench.frames_until(30, || sink.has_secured()), "Secured не пришёл");
    let neutral = match sink.events().first() {
        Some(DevelopEvent::Secured { facts, .. }) => facts.neutral.0,
        other => panic!("первым событием должен быть Secured: {other:?}"),
    };
    sink.events().clear();
    finish(&mut bench.rig, &[bench.epoch]);
    Some(neutral)
}

#[test]
fn the_dng_neutral_follows_the_white_applied_to_the_shown_frame() {
    let Some(held) = secured_neutral(None) else {
        return;
    };
    let Some(changed) = secured_neutral(Some(3000.0)) else {
        return;
    };
    let close = |got: [f64; 3], want: [f64; 3]| got.iter().zip(want).all(|(got, want)| (got - want).abs() < 1e-9);
    assert!(
        close(held, expected_neutral(continuity_rig::MANUAL_KELVIN)),
        "{held:?} против {:?}",
        expected_neutral(continuity_rig::MANUAL_KELVIN)
    );
    assert!(
        close(changed, expected_neutral(3000.0)),
        "{changed:?} против {:?}",
        expected_neutral(3000.0)
    );
    assert!(!close(held, changed), "смена белой видна в нейтрали DNG");
}

/// Что вышло из одного снимка: нейтраль DNG и проявленная картинка (RGBA8).
struct DevelopedStill {
    neutral: [f64; 3],
    picture: Vec<u8>,
}

/// Снимок с видом `look` при одной и той же белой стенда: нейтраль DNG из `Secured` и картинка из итога.
fn still_with_look(look: &look_contract::LookSettings) -> Option<DevelopedStill> {
    let mut bench = setup(StillDevelopConfig::default(), 12)?;
    bench.frames(10);
    bench.inject_timings();
    let (_, _) = still_rig::shown_frame(&mut bench, 3);
    let mut still = plan(1, press_during(3), JPEG_AND_DNG);
    still.look = look.clone();
    let sink = bench.begin(still);
    assert!(bench.frames_until(30, || sink.has_secured()), "Secured не пришёл");
    let neutral = match sink.events().first() {
        Some(DevelopEvent::Secured { facts, .. }) => facts.neutral.0,
        other => panic!("первым событием должен быть Secured: {other:?}"),
    };
    still_rig::offer_readback(&mut bench, 1);
    assert!(bench.frames_until(60, || sink.finals() == 1), "итог снимка не пришёл");
    let picture = match sink.events().last() {
        Some(DevelopEvent::PrimaryReadback { image, .. }) => image.rgba_bytes().to_vec(),
        other => panic!("итог должен быть PrimaryReadback: {other:?}"),
    };
    // События держат буферы GPU; отпускаем до проверки утечек.
    sink.events().clear();
    finish(&mut bench.rig, &[bench.epoch]);
    Some(DevelopedStill { neutral, picture })
}

#[test]
fn look_warmth_and_tint_change_the_developed_picture_but_never_the_dng_neutral() {
    let knob = |value: f32| look_contract::LookKnob::new(value).expect("ручка в диапазоне");
    let tinted = look_contract::LookSettings {
        warmth: knob(0.8),
        tint: knob(-0.6),
        ..look_contract::LookSettings::NEUTRAL
    };
    let Some(plain) = still_with_look(&look_contract::LookSettings::NEUTRAL) else {
        return;
    };
    let Some(with_look) = still_with_look(&tinted) else {
        return;
    };
    // Без этого проверка ничего не доказывает: ручки вида обязаны менять саму картинку.
    assert_ne!(
        plain.picture, with_look.picture,
        "WARMTH и TINT в самом деле меняют проявленную картинку"
    );
    // А нейтраль DNG при той же белой побитово одна: вид к ней не прикасается.
    assert_eq!(
        plain.neutral.map(f64::to_bits),
        with_look.neutral.map(f64::to_bits),
        "AsShotNeutral не зависит от WARMTH и TINT"
    );
}

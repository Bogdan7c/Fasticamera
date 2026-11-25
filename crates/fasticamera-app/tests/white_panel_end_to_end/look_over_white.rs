//! Сценарий k, вторая половина: ручки вида WARMTH и TINT — сдвиг ПОВЕРХ белой, взятой через панель. С нулевыми ручками
//! серая плитка нейтральна (см. `neutrality`), с ненулевыми сдвинута ровно на вид. Что значит «ровно», говорит эталон
//! ISP (`isp_model::look`): картинка с видом обязана совпасть с картинкой без вида при белой, сдвинутой эталоном.

use std::sync::Arc;

use camera_contract::control::WhiteBalanceSetting;
use camera_contract::white::WhiteOrigin;
use isp_model::color::temperature::Kelvin;
use isp_model::color::white_point::{CctDuv, Duv, cct_duv_from_chromaticity, chromaticity_from_cct_duv};
use isp_model::look::model::LookModelSettings;
use isp_model::look::white::shift_white;
use isp_model::look::white_shift;
use look_contract::{LookKnob, LookSettings};
use render_loop::backend::DisplayBackend;
use render_loop::viewfinder::{ViewfinderCommand, ViewfinderCommandOutcome};

use super::session::{PHONE, WhiteSession, lamp_after, reference_images, white_of};
use super::white_rig::{NEUTRAL_TOLERANCE, spread};

/// Вид с заметными теплотой и оттенком: плюс на обеих ручках, остальное нейтрально.
fn warm_and_purple_look() -> Arc<LookSettings> {
    let knob = |value: f32| LookKnob::new(value).expect("ручка в диапазоне");
    Arc::new(LookSettings {
        warmth: knob(0.6),
        tint: knob(0.8),
        ..LookSettings::NEUTRAL
    })
}

/// Белая, которую эталон подставит при рисовании с видом `look`.
fn white_shifted_by_the_model(white: WhiteBalanceSetting, look: &LookSettings) -> WhiteBalanceSetting {
    let chromaticity = chromaticity_from_cct_duv(CctDuv {
        temperature: Kelvin(white.temperature.kelvin()),
        duv: Duv(white.tint.duv()),
    })
    .expect("белая переводится в цветность");
    let shifted = shift_white(chromaticity, white_shift(&LookModelSettings::default(), look)).expect("сдвиг в таблице");
    let shifted = cct_duv_from_chromaticity(shifted.white).expect("сдвинутая белая переводится обратно");
    white_of(shifted.temperature.0, shifted.duv.0)
}

#[test]
fn warmth_and_tint_shift_the_picture_over_the_panel_white_exactly_as_the_model_says() {
    let Some(mut session) = WhiteSession::open() else {
        return;
    };
    // Белое взято через панель и равно свету листа: плитка нейтральна.
    session.source.react_with(lamp_after(2, 2_800.0));
    session.open_card();
    session.tap_phone();
    session.frames(5);
    let held = session
        .wait_origin("белое взято у телефона", WhiteOrigin::Source(PHONE))
        .setting;
    assert!(spread(session.gray_tile_of_the_last_frame()) <= NEUTRAL_TOLERANCE);

    let look = warm_and_purple_look();
    assert_eq!(
        session
            .stand
            .engine
            .rig
            .backend
            .apply_viewfinder_command(ViewfinderCommand::SetLook(Arc::clone(&look))),
        ViewfinderCommandOutcome::Applied
    );
    session.frames(5);
    let with_look = session.gray_tile_of_the_last_frame();
    assert_eq!(session.held().setting, held, "вид белую не трогает");
    assert!(
        spread(with_look) > NEUTRAL_TOLERANCE * 3.0,
        "вид заметно сдвинул плитку: {with_look:?}"
    );

    // Эталон: тот же номер кадра, вида нет, белая сдвинута моделью.
    let frames = session.frame_count();
    let expected = session.gray_tile_of(
        reference_images(white_shifted_by_the_model(held, &look), frames as i64)
            .last()
            .and_then(Option::as_ref)
            .expect("эталонный кадр показан"),
    );
    for (channel, (actual, expected)) in with_look.into_iter().zip(expected).enumerate() {
        assert!(
            (actual - expected).abs() <= NEUTRAL_TOLERANCE,
            "канал {channel}: с видом {actual}, по эталону {expected}"
        );
    }
    session.stand.finish();
}

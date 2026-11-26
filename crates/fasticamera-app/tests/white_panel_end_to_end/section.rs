//! Белое в разделе калибровки на настоящем движке (решения 289–294): палец → экран → движок → рендер. Вход в раздел
//! ставит слежение «Auto» на паузу (белое стоит, источник не спрашивают), выход возвращает слежение с того же белого,
//! капсула в полоске открывает карточку, её шкала и «Phone» двигают белое движка.

use camera_core::white::WhiteFollowState;
use camera_screen::modes::CaptureMode;
use camera_screen::white_config::WhiteScreenConfig;
use camera_screen::white_display::kelvin_text;
use camera_screen::{ScreenAction, SectionCard};
use ui_kit::tokens::TextRoleKind;

use super::follow::{FOLLOW_SETTLE_FRAMES, auto_session, follow_state};
use super::panel_view::text_items;
use super::session::{PHONE, WhiteSession, lamp_after};
use super::white_engine::setting_of;

fn enter_calibrate(session: &mut WhiteSession) {
    session
        .stand
        .send_action(ScreenAction::ModeSelected(CaptureMode::Calibrate));
    session.stand.ui_frames(10);
}

fn leave_calibrate(session: &mut WhiteSession) {
    session
        .stand
        .send_action(ScreenAction::ModeSelected(CaptureMode::Photo));
    session.stand.ui_frames(10);
}

fn asked(session: &WhiteSession) -> u64 {
    session.stand.status().three_a.white.follow.asked
}

/// Тап по капсуле белого в полоске прибора раздела; карточка доезжает до места.
fn open_section_card(session: &mut WhiteSession) {
    let capsule = session
        .stand
        .calibration_targets()
        .white_capsule
        .expect("капсула белого в полоске");
    session.stand.tap(capsule);
    session.stand.ui_frames(45);
    assert_eq!(
        session.stand.screen().calibration().card(),
        SectionCard::White,
        "карточка белого открылась"
    );
}

#[test]
fn entering_calibrate_pauses_auto_without_asking_the_source_and_leaving_follows_again_from_the_same_white() {
    let Some(mut session) = auto_session() else {
        return;
    };
    let held_before = session.held();
    let asked_before = asked(&session);
    assert!(
        matches!(follow_state(&session), WhiteFollowState::Following { .. }),
        "Auto следит"
    );

    enter_calibrate(&mut session);
    session.stand.wait_status("слежение на паузе", |status| {
        matches!(status.three_a.white.follow.state, WhiteFollowState::Paused { .. })
    });
    // Свет в комнате ушёл, а раздел открыт: белое стоит, источник не спрашивают.
    session.source.react_with(lamp_after(0, 6_500.0));
    session.frames(FOLLOW_SETTLE_FRAMES);
    assert_eq!(
        asked(&session),
        asked_before,
        "пока открыт раздел, источник не спрашивают"
    );
    assert_eq!(session.held(), held_before, "белое стоит там же");
    assert!(
        follow_state(&session).is_on(),
        "режим «Auto» остаётся включённым и запомненным"
    );

    leave_calibrate(&mut session);
    session.stand.wait_status("слежение вернулось", |status| {
        matches!(status.three_a.white.follow.state, WhiteFollowState::Following { .. })
    });
    session.frames(FOLLOW_SETTLE_FRAMES);
    assert!(asked(&session) > asked_before, "после выхода источник снова спрашивают");
    assert_eq!(
        session.held().setting,
        setting_of(6_500.0),
        "белое доехало до нового света"
    );
    session.stand.finish();
}

#[test]
fn the_pause_survives_the_app_going_to_the_background_and_back_while_calibrate_is_open() {
    let Some(mut session) = auto_session() else {
        return;
    };
    enter_calibrate(&mut session);
    session.stand.wait_status("слежение на паузе", |status| {
        matches!(status.three_a.white.follow.state, WhiteFollowState::Paused { .. })
    });
    let asked_before = asked(&session);

    session.stand.pause();
    session.stand.resume();
    session.source.react_with(lamp_after(0, 6_500.0));
    session.frames(FOLLOW_SETTLE_FRAMES);

    assert!(
        matches!(follow_state(&session), WhiteFollowState::Paused { .. }),
        "после возврата из фона раздел всё ещё открыт, пауза держится: {:?}",
        follow_state(&session)
    );
    assert_eq!(asked(&session), asked_before, "и источник не спрашивали");
    session.stand.finish();
}

#[test]
fn the_capsule_card_ruler_moves_the_held_white_turns_auto_off_and_the_capsule_shows_the_new_number() {
    let Some(mut session) = auto_session() else {
        return;
    };
    enter_calibrate(&mut session);
    open_section_card(&mut session);
    let before = session.held();

    let ruler = session
        .stand
        .calibration_targets()
        .white_ruler
        .expect("шкала карточки белого в разделе");
    let from = ruler;
    let to = ui_core::geometry::Point::new(from.x - 70.0, from.y);
    session.stand.drag(from, to, 400);
    session.stand.ui_frames(30);

    let held = session.wait_origin("рука", camera_contract::white::WhiteOrigin::Manual);
    assert_ne!(
        held.setting.temperature, before.setting.temperature,
        "шкала сдвинула белое движка"
    );
    assert_eq!(
        follow_state(&session),
        WhiteFollowState::Off,
        "ручная правка выключила «Auto» по прежним правилам"
    );
    // Шкала ещё доводит значение после отрыва пальца: берём белое, когда оно успокоилось.
    session.frames(FOLLOW_SETTLE_FRAMES);
    let held = session.held();
    // Капсула в полоске показывает то же число, что держит движок.
    let number = format!(
        "{} K",
        kelvin_text(held.setting.temperature, &WhiteScreenConfig::default())
    );
    assert_eq!(
        text_items(session.stand.ui_list(), &number, TextRoleKind::Caption).len(),
        1,
        "капсула показывает «{number}»"
    );
    session.stand.finish();
}

#[test]
fn phone_in_the_section_card_takes_the_phone_white() {
    let Some(mut session) = WhiteSession::open() else {
        return;
    };
    enter_calibrate(&mut session);
    open_section_card(&mut session);
    session.source.react_with(lamp_after(0, 4_100.0));

    let phone = session.stand.calibration_targets().white_row[0];
    session.stand.tap(phone);
    session.stand.ui_frames(3);

    session.wait_origin("белое телефона", camera_contract::white::WhiteOrigin::Source(PHONE));
    session.frames(20);
    assert_eq!(
        session.held().setting,
        setting_of(4_100.0),
        "«Phone» взял свет, названный источником"
    );
    session.stand.finish();
}

#[test]
fn the_section_card_has_no_auto_button_and_the_row_is_phone_then_the_lights() {
    let Some(mut session) = WhiteSession::open() else {
        return;
    };
    enter_calibrate(&mut session);
    open_section_card(&mut session);
    let lights = camera_screen::ScreenConfig::default().white.lights.len();
    assert_eq!(session.stand.calibration_targets().white_row.len(), 1 + lights);
    assert!(
        text_items(session.stand.ui_list(), "Auto", TextRoleKind::Caption).is_empty(),
        "фишки Auto в разделе нет"
    );
    session.stand.finish();
}

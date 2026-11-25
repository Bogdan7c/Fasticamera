//! Сценарий c: долгое нажатие на кнопку белого берёт белое телефона без панели; короткий тап открывает панель и
//! картинку не трогает; долгое нажатие при открытой панели берёт белое, панель остаётся.

use camera_contract::three_a::PointOfInterest;
use camera_contract::white::WhiteOrigin;
use camera_screen::PanelState;
use camera_screen::tools::ToolId;
use shell_contract::haptics::HapticPattern;

use super::panel_view::{colors, strip_number};
use super::session::{PHONE, WhiteSession, lamp_after, opening_setting};
use super::white_engine::setting_of;

#[test]
fn a_long_press_on_the_button_with_the_panel_closed_takes_the_phone_white_without_opening_the_panel() {
    let Some(mut session) = WhiteSession::open() else {
        return;
    };
    let (white, _) = colors();
    assert_eq!(session.panel(), PanelState::Closed);
    let requests_before = session.source.requests().len();
    session.source.react_with(lamp_after(2, 4_500.0));

    session.long_press_button();
    session.frames(6);
    let held = session.wait_origin("белое взято долгим нажатием", WhiteOrigin::Source(PHONE));
    assert_eq!(held.setting, setting_of(4_500.0));
    assert_eq!(
        session.source.requests().len(),
        requests_before + 1,
        "источника спросили ровно раз"
    );
    assert_eq!(session.panel(), PanelState::Closed, "панель не открылась");
    assert_eq!(
        session.stand.haptics_played(),
        vec![HapticPattern::MediumImpact],
        "ровно одна вибрация подтверждения"
    );
    session.assert_picture_is(setting_of(4_500.0), "долгое нажатие при закрытой панели");
    let number = strip_number(session.stand.ui_list(), 4_500.0).expect("число 4500 на кнопке");
    assert_eq!(number.color, white, "белое телефона — белое число");
    assert_eq!(
        session.stand.status().three_a.point,
        PointOfInterest::WholeFrame,
        "под кнопкой фокус не наводился"
    );
    session.stand.finish();
}

#[test]
fn a_short_tap_opens_the_panel_and_leaves_the_picture_and_the_white_exactly_as_they_were() {
    let Some(mut session) = WhiteSession::open() else {
        return;
    };
    let before = session.held();
    let requests_before = session.source.requests().len();
    let take_before = session.stand.status().three_a.white.last_take;
    let intents_before = session.stand.effect_counters().camera.sent;
    session.open_card();
    assert_eq!(session.panel(), PanelState::Submenu(ToolId::White));
    assert_eq!(session.held(), before, "белое то же, происхождение то же");
    assert_eq!(session.stand.haptics_played(), vec![], "открытие без вибрации");
    assert_eq!(
        session.source.requests().len(),
        requests_before,
        "открытие панели источника не спрашивает"
    );
    assert_eq!(
        session.stand.effect_counters().camera.sent,
        intents_before,
        "камере не ушло ни одного намерения"
    );
    session.assert_picture_is(opening_setting(), "открытие панели картинку не меняет");
    // Команды «взять» не было: движок не помнит ни одной своей, источника не спрашивали (кадры уже прошли).
    assert_eq!(
        session.stand.status().three_a.white.last_take,
        take_before,
        "открытие панели не берёт белое"
    );
    assert_eq!(
        session.source.requests().len(),
        requests_before,
        "открытие панели источника не спрашивает"
    );
    assert_eq!(
        session.stand.effect_counters().camera.sent,
        intents_before,
        "камере не ушло ни одного намерения"
    );
    // Закрытие панели той же кнопкой тоже ничего не меняет.
    session.tap_button();
    assert_eq!(session.panel(), PanelState::Closed);
    assert_eq!(session.held(), before);
    session.assert_picture_is(opening_setting(), "закрытие панели картинку не меняет");
    session.stand.finish();
}

#[test]
fn a_long_press_with_the_panel_open_takes_the_white_and_keeps_the_panel_open() {
    let Some(mut session) = WhiteSession::open() else {
        return;
    };
    session.open_card();
    session.tap_light(0);
    session.wait_origin("рука поставила Sun", WhiteOrigin::Manual);
    session.frames(4);
    session.source.react_with(lamp_after(2, 4_500.0));

    session.long_press_button();
    session.frames(6);
    let held = session.wait_origin("долгое нажатие взяло белое", WhiteOrigin::Source(PHONE));
    assert_eq!(held.setting, setting_of(4_500.0));
    assert_eq!(
        session.panel(),
        PanelState::Submenu(ToolId::White),
        "панель осталась открытой"
    );
    let buzzes: Vec<_> = session
        .stand
        .haptics_played()
        .into_iter()
        .filter(|pattern| *pattern == HapticPattern::MediumImpact)
        .collect();
    assert_eq!(buzzes.len(), 1, "одна вибрация подтверждения");
    session.assert_picture_is(setting_of(4_500.0), "долгое нажатие при открытой панели");
    session.stand.finish();
}

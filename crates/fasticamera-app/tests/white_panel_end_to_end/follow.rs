//! Слежение за белым с экрана (WB44–WB51): палец → экран → движок → рендер → пиксели. Свет «в комнате» меняет
//! фальшивый источник: он отвечает на каждый вопрос слежения.

use camera_contract::three_a::AeAfLock;
use camera_contract::white::WhiteOrigin;
use camera_core::white::WhiteFollowState;
use camera_screen::WhiteAxis;
use camera_screen::white_display::PHONE_LABEL;
use ui_draw::color::DisplayColor;
use ui_kit::tokens::TextRoleKind;

use super::panel_view::{colors, strip_number, strip_number_any_opacity, text_items};
use super::session::{PHONE, WhiteSession, lamp_after, opening_setting};
use super::white_engine::setting_of;

const AUTO_UNIT: &str = "AUTO";
const AUTO_LABEL: &str = "Auto";
/// Вопрос (до 6 кадров) + доставка + переезд 0,3 с (18 кадров) с запасом.
const FOLLOW_SETTLE_FRAMES: i64 = 40;

fn follow_state(session: &WhiteSession) -> WhiteFollowState {
    session.stand.status().three_a.white.follow.state
}

/// Цвет единственной видимой надписи этой роли; `None` — надписи нет.
fn color_of(session: &WhiteSession, text: &str, role: TextRoleKind) -> Option<DisplayColor> {
    let mut found = text_items(session.stand.ui_list(), text, role);
    assert!(found.len() <= 1, "«{text}» нарисовано дважды");
    found.pop().map(|item| item.color)
}

/// Подпись под числом на кнопке у затвора.
fn unit_color(session: &WhiteSession, text: &str) -> Option<DisplayColor> {
    color_of(session, text, TextRoleKind::StripUnit)
}

/// Подпись кнопки ряда карточки.
fn row_color(session: &WhiteSession, text: &str) -> Option<DisplayColor> {
    color_of(session, text, TextRoleKind::Caption)
}

/// Карточка открыта, «Auto» нажат, движок подтвердил.
fn auto_session() -> Option<WhiteSession> {
    let mut session = WhiteSession::open()?;
    session.open_card();
    session.tap_auto();
    session.stand.wait_status("движок следит", |status| {
        status.three_a.white.follow.state.is_on()
    });
    session.frames(4);
    Some(session)
}

#[test]
fn a_tap_on_auto_makes_the_picture_follow_the_light_and_shows_it_on_the_button_and_in_the_row() {
    let Some(mut session) = WhiteSession::open() else {
        return;
    };
    let (white, yellow) = colors();
    session.open_card();
    assert_eq!(
        row_color(&session, PHONE_LABEL),
        Some(yellow),
        "до Auto подсвечен Phone"
    );
    assert_eq!(unit_color(&session, AUTO_UNIT), None, "до Auto под числом «K»");
    let sent_before = session.stand.effect_counters().white.sent;

    session.tap_auto();
    session.stand.wait_status("движок следит", |status| {
        status.three_a.white.follow.state.is_on()
    });
    assert_eq!(
        session.stand.effect_counters().white.sent,
        sent_before + 1,
        "одно намерение"
    );
    assert_eq!(
        session.stand.haptics_played(),
        vec![],
        "тап по кнопке ряда — без вибрации"
    );
    session.source.react_with(lamp_after(0, 5_600.0));
    session.frames(FOLLOW_SETTLE_FRAMES);
    session.wait_origin("белое от телефона", WhiteOrigin::Source(PHONE));
    assert_eq!(session.held().setting, setting_of(5_600.0));
    session.assert_picture_is(setting_of(5_600.0), "картинка доехала за светом");

    let number = strip_number(session.stand.ui_list(), 5_600.0).expect("живое число на кнопке");
    assert_eq!(number.color, white, "число белое");
    assert_eq!(unit_color(&session, AUTO_UNIT), Some(white), "под числом AUTO");
    assert_eq!(row_color(&session, AUTO_LABEL), Some(yellow), "Auto подсвечен");
    assert_eq!(row_color(&session, PHONE_LABEL), Some(white), "Phone погас");
    // 5600 К — число кнопки Sun, но кнопку никто не нажимал: белое лишь проезжало мимо.
    assert_eq!(
        row_color(&session, "Sun"),
        Some(white),
        "кнопки света при Auto не горят"
    );

    // Свет ушёл дальше — картинка за ним, без единого касания.
    session.source.react_with(lamp_after(0, 3_000.0));
    session.frames(FOLLOW_SETTLE_FRAMES);
    session.assert_picture_is(setting_of(3_000.0), "и дальше за светом");
    session.stand.finish();
}

#[test]
fn the_number_does_not_flicker_or_shake_while_auto_asks_the_phone() {
    let Some(mut session) = auto_session() else {
        return;
    };
    // Вопросы идут, ответ приходит через три кадра: «ожидание» на экране не должно появиться ни на одном кадре.
    session.source.react_with(lamp_after(3, 2_800.0));
    let requests_before = session.source.requests().len();
    let seen = session.watch(60, |list| {
        strip_number_any_opacity(list, 2_800.0).map(|item| (item.opacity, item.transform.tx))
    });
    assert!(
        session.source.requests().len() >= requests_before + 4,
        "телефон действительно спрашивали"
    );
    let first = seen.first().copied().flatten().expect("число на экране");
    assert!(
        seen.iter().all(|state| *state == Some(first)),
        "число неподвижно и не мерцает: {seen:?}"
    );
    assert_eq!(session.stand.haptics_played(), vec![], "и молчит");
    session.stand.finish();
}

#[test]
fn every_human_action_on_the_screen_turns_auto_off_and_the_picture_stays_with_the_hand() {
    // Кнопка света.
    let Some(mut session) = auto_session() else {
        return;
    };
    session.tap_light(3);
    let held = session.wait_origin("рука", WhiteOrigin::Manual);
    session.source.react_with(lamp_after(0, 7_000.0));
    session.frames(FOLLOW_SETTLE_FRAMES);
    assert_eq!(follow_state(&session), WhiteFollowState::Off);
    assert_eq!(session.held(), held);
    session.assert_picture_is(held.setting, "кнопка света при Auto");
    let (_, yellow) = colors();
    let number = strip_number(session.stand.ui_list(), held.setting.temperature.kelvin()).expect("число");
    assert_eq!(number.color, yellow, "ручное — жёлтое");
    assert_eq!(unit_color(&session, AUTO_UNIT), None, "AUTO ушло");
    session.stand.finish();

    // Шкала под пальцем: слежение не тянет белое обратно, пока палец ведёт.
    let mut session = auto_session().expect("GPU есть");
    session.select_axis(WhiteAxis::Temperature);
    session.source.react_with(lamp_after(0, 7_000.0));
    session.drag_ruler(-6.0, 12);
    session.wait_origin("шкала", WhiteOrigin::Manual);
    session.frames(FOLLOW_SETTLE_FRAMES);
    assert_eq!(follow_state(&session), WhiteFollowState::Off);
    let held = session.held();
    assert_eq!(held.origin, WhiteOrigin::Manual);
    assert_ne!(held.setting, setting_of(7_000.0));
    session.assert_picture_is(held.setting, "шкала при Auto");
    session.stand.finish();

    // «Phone» и долгое нажатие на кнопку у затвора: взять разово и держать (WB51).
    for long_press in [false, true] {
        let mut session = auto_session().expect("GPU есть");
        session.source.react_with(lamp_after(2, 4_500.0));
        if long_press {
            session.long_press_button();
        } else {
            session.tap_phone();
        }
        session.stand.wait_status("слежение выключено", |status| {
            !status.three_a.white.follow.state.is_on()
        });
        session.frames(6);
        session.source.react_with(lamp_after(0, 7_000.0));
        session.frames(FOLLOW_SETTLE_FRAMES);
        assert_eq!(
            session.held().setting,
            setting_of(4_500.0),
            "взято разово (долгое: {long_press})"
        );
        session.assert_picture_is(setting_of(4_500.0), "взято разово и держится");
        session.stand.finish();
    }
}

#[test]
fn a_long_press_on_the_frame_freezes_the_white_and_a_tap_releases_it() {
    let Some(mut session) = auto_session() else {
        return;
    };
    // Закрыть карточку, чтобы жест попал в кадр.
    session.tap_button();
    let frame_center = super::touch_targets::viewfinder_center(session.stand.screen());
    session.stand.hold(frame_center, 1_200);
    session.stand.wait_status("AE/AF заблокированы", |status| {
        status.three_a.lock == AeAfLock::Locked
    });
    let held = session.held();
    session.source.react_with(lamp_after(0, 7_000.0));
    session.frames(FOLLOW_SETTLE_FRAMES);
    assert_eq!(session.held(), held, "под блокировкой белое стоит");
    session.assert_picture_is(opening_setting(), "картинка прежняя");
    assert!(
        unit_color(&session, AUTO_UNIT).is_some(),
        "режим на кнопке виден и под блокировкой"
    );

    session.stand.tap(frame_center);
    session.stand.wait_status("блокировка снята", |status| {
        status.three_a.lock == AeAfLock::Unlocked
    });
    session.frames(FOLLOW_SETTLE_FRAMES);
    session.assert_picture_is(setting_of(7_000.0), "после снятия — за светом");
    session.stand.finish();
}

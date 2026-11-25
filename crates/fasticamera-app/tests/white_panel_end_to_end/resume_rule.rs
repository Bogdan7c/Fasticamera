//! Правило возобновления `keep_last` и панель белого (WB36): пальцем по-прежнему можно взять белое телефона —
//! долгим нажатием на кнопку и кнопкой «Phone», хотя при возврате в приложение источник не спрашивается.

use camera_contract::white::WhiteOrigin;
use camera_core::white::WhiteResumeRule;

use super::session::{PHONE, WhiteSession, lamp_after, opening_setting};
use super::white_engine::setting_of;

/// Ждёт, пока движок держит белую температуры `kelvin` (с точностью до сотой доли кельвина).
fn wait_kelvin(session: &WhiteSession, what: &str, kelvin: f64) {
    session.stand.wait_status(what, |status| {
        (status.three_a.white.held.setting.temperature.kelvin() - kelvin).abs() < 0.01
    });
}

#[test]
fn under_keep_last_the_return_asks_nobody_but_the_long_press_and_the_phone_button_take_the_phone_white() {
    let Some(mut session) = WhiteSession::open() else {
        return;
    };
    // Человек выбрал keep_last; белое телефона уже получено при открытии (2800 К).
    session
        .stand
        .handle()
        .set_white_resume_rule(WhiteResumeRule::KeepLast)
        .expect("команда принята");
    session.stand.wait_status("выбор принят", |status| {
        status.three_a.white.resume_rule == WhiteResumeRule::KeepLast
    });
    let held = session.held();
    let requests_before = session.source.requests().len();

    // Пауза и возврат: источник бы ответил 7000 К, но не спрошен, белое и картинка те же.
    session.source.react_with(lamp_after(0, 7_000.0));
    session.stand.pause();
    session.stand.resume();
    session.frames(8);
    assert_eq!(session.held(), held, "белое то же бит-в-бит");
    assert_eq!(session.source.requests().len(), requests_before, "источник не спрошен");
    session.assert_picture_is(opening_setting(), "после возврата под keep_last");

    // Долгое нажатие на кнопку белого берёт белое телефона.
    session.source.react_with(lamp_after(2, 4_500.0));
    session.long_press_button();
    session.frames(6);
    wait_kelvin(&session, "долгое нажатие взяло белое", 4_500.0);
    assert_eq!(session.held().origin, WhiteOrigin::Source(PHONE));
    assert_eq!(session.source.requests().len(), requests_before + 1);
    session.assert_picture_is(setting_of(4_500.0), "долгое нажатие под keep_last");

    // Кнопка «Phone» на карточке берёт белое телефона.
    session.open_card();
    session.source.react_with(lamp_after(2, 3_600.0));
    session.tap_phone();
    session.frames(6);
    wait_kelvin(&session, "кнопка Phone взяла белое", 3_600.0);
    assert_eq!(session.source.requests().len(), requests_before + 2);
    session.assert_picture_is(setting_of(3_600.0), "кнопка Phone под keep_last");
    session.stand.finish();
}

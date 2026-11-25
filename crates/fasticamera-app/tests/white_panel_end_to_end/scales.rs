//! Сценарии f, g, h: шкалы Кельвинов и оттенка. Протяжка шкалы меняет ТОЛЬКО свою ось (вторая остаётся ровно той,
//! что дал источник), шкалы следуют за белым (WB21: первое движение идёт от точного значения без скачка), а ответ
//! источника, пришедший в одном кадре с первым движением шкалы, не затирается устаревшим оттенком экрана.

use camera_contract::control::WhiteBalanceSetting;
use camera_contract::white::WhiteOrigin;
use camera_screen::tools::ToolId;
use camera_screen::{PanelState, WhiteAxis};

use super::panel_view::{card_kelvin_number, card_tint_number, strip_number};
use super::session::{PHONE, WhiteSession, answer_with, white_of};

/// Белое, которое называет источник в этих сценариях: 3140 К и оттенок +0,0164 Duv (+16 единиц показа).
fn source_white() -> WhiteBalanceSetting {
    white_of(3_140.0, 0.0164)
}

fn mired(kelvin: f64) -> f64 {
    1_000_000.0 / kelvin
}

/// Сеанс, где телефон по тапу «Phone» отдаёт 3140 К / +0,0164 и белое уже взято; панель открыта.
fn session_with_the_source_white_taken() -> Option<WhiteSession> {
    let mut session = WhiteSession::open()?;
    session.open_card();
    session.source.react_with(answer_with(source_white(), 2));
    session.tap_phone();
    session.frames(6);
    let held = session.wait_origin("белое взято у телефона", WhiteOrigin::Source(PHONE));
    assert_eq!(held.setting, source_white());
    Some(session)
}

/// f. Шкала Кельвинов: кадр = эталон с выставленным движком значением; оттенок остался ровно 0,0164.
#[test]
fn dragging_the_kelvin_scale_sets_the_reference_picture_and_keeps_the_tint_exactly() {
    let Some(mut session) = session_with_the_source_white_taken() else {
        return;
    };
    session.drag_ruler(-2.0, 40);
    let held = session.wait_origin("шкала Кельвинов поставила ручное белое", WhiteOrigin::Manual);
    assert!(
        held.setting.temperature.kelvin() > 3_140.0,
        "палец влево поднял Кельвины"
    );
    assert_eq!(
        held.setting.tint,
        source_white().tint,
        "оттенок ровно 0,0164, а не округлённый и не ноль"
    );
    session.assert_picture_is(held.setting, "шкала Кельвинов");
    let shown = card_kelvin_number(session.stand.ui_list(), held.setting.temperature.kelvin());
    assert_eq!(shown.len(), 1, "число на карточке показывает белое движка");
    session.stand.finish();
}

/// f. Шкала оттенка: кадр = эталон; Кельвины остались ровно 3140.
#[test]
fn dragging_the_tint_scale_sets_the_reference_picture_and_keeps_the_kelvin_exactly() {
    let Some(mut session) = session_with_the_source_white_taken() else {
        return;
    };
    session.select_axis(WhiteAxis::Tint);
    session.drag_ruler(2.0, 40);
    let held = session.wait_origin("шкала оттенка поставила ручное белое", WhiteOrigin::Manual);
    assert!(
        held.setting.tint.duv() < source_white().tint.duv(),
        "палец вправо уменьшил оттенок"
    );
    assert_eq!(
        held.setting.temperature,
        source_white().temperature,
        "Кельвины ровно 3140, а не округлённые"
    );
    session.assert_picture_is(held.setting, "шкала оттенка");
    session.stand.finish();
}

/// Как белое пришло к человеку: от этого не должно зависеть ничего в шкалах.
enum Arrival {
    /// Долгое нажатие при закрытой панели.
    LongPressWithClosedPanel,
    /// Тап «Phone» в открытой панели.
    PhoneButton,
    /// Кнопка света.
    LightButton,
}

/// g. Шкалы следуют за белым: числа точные, кадр не меняется от открытия и закрытия панели, первое движение пальца
/// на 1 pt стоит ровно цену 1 pt от точного значения, после отпускания значение не уезжает.
fn check_the_scales_follow(arrival: Arrival, expected: WhiteBalanceSetting, shown_tint: &str) {
    let Some(mut session) = WhiteSession::open() else {
        return;
    };
    match arrival {
        Arrival::LongPressWithClosedPanel => {
            session.source.react_with(answer_with(expected, 2));
            session.long_press_button();
            session.frames(6);
            session.wait_origin("белое взято долгим нажатием", WhiteOrigin::Source(PHONE));
            assert_eq!(session.panel(), PanelState::Closed);
        }
        Arrival::PhoneButton => {
            session.open_card();
            session.source.react_with(answer_with(expected, 2));
            session.tap_phone();
            session.frames(6);
            session.wait_origin("белое взято кнопкой Phone", WhiteOrigin::Source(PHONE));
            session.tap_button();
            assert_eq!(session.panel(), PanelState::Closed);
        }
        Arrival::LightButton => {
            session.open_card();
            session.tap_light(0);
            session.wait_origin("рука", WhiteOrigin::Manual);
            session.tap_button();
            assert_eq!(session.panel(), PanelState::Closed);
        }
    }
    session.frames(4);
    let held = session.held();
    assert_eq!(held.setting, expected, "движок держит ровно эти числа");
    session.assert_picture_is(expected, "до открытия панели");

    // Открыли панель: числа на карточке те же, картинка бит-в-бит та же.
    session.open_card();
    assert_eq!(
        card_kelvin_number(session.stand.ui_list(), expected.temperature.kelvin()).len(),
        1,
        "число Кельвинов"
    );
    assert_eq!(
        card_tint_number(session.stand.ui_list(), expected.tint.duv()).len(),
        1,
        "число оттенка «{shown_tint}»"
    );
    assert!(strip_number(session.stand.ui_list(), expected.temperature.kelvin()).is_some());
    session.assert_picture_is(expected, "панель открыта");
    session.tap_button();
    session.assert_picture_is(expected, "панель закрыта снова");
    session.open_card();
    assert_eq!(session.held(), held, "открытие и закрытие белое не тронули");

    // Движение на 1 pt за кадр: цена 1 pt — 0,5 майреда, первые 10 pt съедает порог начала протяжки (пан), поэтому
    // 40 pt = 30 pt × 0,5 = ровно 15 майред ОТ ТОЧНОГО значения: ни скачка к риске (риски по 5 майред), ни к округлённому.
    session.drag_ruler(-1.0, 40);
    let after = session.wait_origin("шкала поставила ручное белое", WhiteOrigin::Manual);
    let shift = mired(expected.temperature.kelvin()) - mired(after.setting.temperature.kelvin());
    assert!(
        (shift - 15.0).abs() < 0.01,
        "30 pt после порога по 0,5 майреда = 15 майред, получили {shift}"
    );
    assert_eq!(after.setting.tint, expected.tint, "оттенок не тронут");
    // После отпускания значение не уезжает: шкала стоит там, где отпустили.
    session.frames(40);
    assert_eq!(session.held(), after, "после отпускания значение не уехало");
    session.assert_picture_is(after.setting, "кадр после протяжки");
    assert_eq!(session.panel(), PanelState::Submenu(ToolId::White));
    session.stand.finish();
}

#[test]
fn the_scales_follow_the_white_taken_by_a_long_press_with_the_panel_closed() {
    check_the_scales_follow(Arrival::LongPressWithClosedPanel, source_white(), "+16");
}

#[test]
fn the_scales_follow_the_white_taken_by_the_phone_button() {
    check_the_scales_follow(Arrival::PhoneButton, source_white(), "+16");
}

#[test]
fn the_scales_follow_the_white_set_by_a_light_button() {
    let sun = WhiteSession::open().map(|session| session.light_setting(0));
    if let Some(sun) = sun {
        check_the_scales_follow(Arrival::LightButton, sun, "+3");
    }
}

/// h. Гонка: ответ источника и первое движение шкалы К — в одном кадре UI. Оттенок источника не затёрт.
#[test]
fn a_source_answer_in_the_same_frame_as_the_first_kelvin_move_does_not_overwrite_its_tint() {
    let Some(mut session) = WhiteSession::open() else {
        return;
    };
    session.open_card();
    session.source.react_with(answer_with(source_white(), 2));
    session.tap_phone();
    // Только кадры камеры: движок берёт ответ источника, а экран об этом ещё не знает (кадров UI не было), и факты
    // на кадре с пальцем остаются прежними (в продукте — снимок статуса, снятый раньше ответа).
    session.stand.facts_freeze.freeze();
    session.camera_frames_only(6);
    let held = session.held();
    assert_eq!(held.origin, WhiteOrigin::Source(PHONE));
    assert_eq!(held.setting, source_white(), "движок уже держит ответ источника");

    // Палец ложится на шкалу и делает два шага ДО кадра UI: экран разбирает всё это в одном кадре.
    session.press_ruler_and_move(-2.0, 0);
    session.move_ruler_finger(-20.0);
    session.move_ruler_finger(-20.0);
    session.stand.ui_frames(1);
    session.stand.facts_freeze.thaw();
    session.release_ruler();
    let after = session.wait_origin("шкала Кельвинов поставила ручное белое", WhiteOrigin::Manual);
    assert_ne!(
        after.setting.temperature, held.setting.temperature,
        "Кельвины сдвинулись"
    );
    assert_eq!(
        after.setting.tint,
        source_white().tint,
        "оттенок источника не затёрт устаревшим оттенком экрана"
    );
    session.stand.finish();
}

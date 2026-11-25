//! Сценарии a и b: тап по кнопке света и тап «Phone» — палец → экран → движок → рендер → пиксели.

use camera_contract::white::WhiteOrigin;
use camera_core::white::{WhiteRequestOutcome, WhiteTakeState};
use camera_screen::ScreenConfig;
use camera_screen::white_display::kelvin_text;

use super::panel_view::{colors, strip_number};
use super::session::{PHONE, WhiteSession, lamp_after, opening_setting, reference_images};
use super::white_engine::setting_of;

/// a. Каждая из пяти кнопок света: кадр = эталон с её числом (в том числе с оттенком), число на кнопке жёлтое.
#[test]
fn a_tap_on_each_light_button_gives_the_reference_picture_with_its_number_and_a_yellow_number() {
    let config = ScreenConfig::default();
    let (_, yellow) = colors();
    for (index, light) in config.white.lights.iter().enumerate() {
        let Some(mut session) = WhiteSession::open() else {
            return;
        };
        session.open_card();
        assert_eq!(
            session.held().origin,
            WhiteOrigin::Source(PHONE),
            "до тапа белое от телефона"
        );
        let before = reference_images(opening_setting(), session.frame_count() as i64);

        session.tap_light(index);
        let held = session.wait_origin("рука поставила белое кнопкой", WhiteOrigin::Manual);
        assert_eq!(
            held.setting,
            session.light_setting(index),
            "{}: движок держит числа кнопки",
            light.label
        );
        session.assert_picture_is(held.setting, &light.label);
        // Эталон не пустышка: картинка кнопки света отличается от картинки открытия.
        let total = session.frame_count();
        let shown = reference_images(held.setting, total as i64);
        assert_ne!(
            shown[total - 1],
            before[before.len() - 1],
            "{}: картинка сменилась",
            light.label
        );

        let number = strip_number(session.stand.ui_list(), light.kelvin)
            .unwrap_or_else(|| panic!("{}: на кнопке нет числа {}", light.label, light.kelvin));
        assert_eq!(number.color, yellow, "{}: ручное число жёлтое", light.label);
        session.stand.finish();
    }
}

/// b. Тап «Phone»: кадр = эталон с ответом источника, число на кнопке белое.
#[test]
fn a_tap_on_phone_gives_the_reference_picture_with_the_source_answer_and_a_white_number() {
    let Some(mut session) = WhiteSession::open() else {
        return;
    };
    let (white, _) = colors();
    session.open_card();
    // Сначала рука: число жёлтое, белое не от телефона.
    session.tap_light(0);
    session.wait_origin("рука", WhiteOrigin::Manual);
    session.frames(4);
    let sun = strip_number(session.stand.ui_list(), 5_600.0).expect("число Sun на кнопке");
    assert_ne!(sun.color, white, "до «Phone» число жёлтое");

    session.source.react_with(lamp_after(2, 4_500.0));
    session.tap_phone();
    session.frames(6);
    let held = session.wait_origin("белое взято у телефона", WhiteOrigin::Source(PHONE));
    assert_eq!(held.setting, setting_of(4_500.0));
    session.assert_picture_is(setting_of(4_500.0), "ответ телефона");
    let number = strip_number(session.stand.ui_list(), 4_500.0).expect("число 4500 на кнопке");
    assert_eq!(number.color, white, "белое телефона — белое число");
    session.stand.finish();
}

/// b'. Ответ источника вне пределов движка прижат: показ и кадр прижатые, не сырое число источника.
#[test]
fn an_answer_beyond_the_limits_is_clamped_in_the_number_and_in_the_picture() {
    let range_cases = [(20_000.0, 12_000.0), (1_200.0, 2_000.0)];
    for (asked, clamped) in range_cases {
        let Some(mut session) = WhiteSession::open() else {
            return;
        };
        session.open_card();
        session.source.react_with(lamp_after(2, asked));
        session.tap_phone();
        session.frames(6);
        let held = session.wait_origin("белое взято", WhiteOrigin::Source(PHONE));
        assert_eq!(held.setting.temperature.kelvin(), clamped, "движок прижал {asked} К");
        let take = session.stand.status().three_a.white.last_take.expect("исход команды");
        assert!(
            matches!(
                take.state,
                WhiteTakeState::Finished(WhiteRequestOutcome::Applied { .. })
            ),
            "{take:?}"
        );
        session.assert_picture_is(setting_of(clamped), "прижатое белое");
        let shown = kelvin_text(held.setting.temperature, &ScreenConfig::default().white);
        assert_eq!(shown, format!("{clamped:.0}"), "текст показа прижат");
        assert!(
            strip_number(session.stand.ui_list(), clamped).is_some(),
            "число на кнопке прижато"
        );
        assert!(
            strip_number(session.stand.ui_list(), asked).is_none(),
            "сырое число источника на экран не попало"
        );
        session.stand.finish();
    }
}

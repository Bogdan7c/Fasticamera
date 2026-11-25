//! Модель показа белого: числа, знак оттенка, подсветка кнопок, оценка телефона, манера ожидания.

use camera_contract::control::{ColorTemperature, Tint, WhiteBalanceSetting};
use camera_contract::white::{HeldWhite, WhiteOrigin, WhiteSourceId, WhiteUnavailableReason};
use camera_contract::white_reading::WhiteReading;

use crate::white_config::WhiteScreenConfig;
use crate::white_display::{
    WhiteButtonManner, auto_is_active, button_manner, kelvin_text, light_is_active, number_is_manual,
    phone_estimate_text, phone_is_active, tint_text, tint_units, white_unit_label,
};
use crate::white_facts::{WhiteFollowFacts, WhiteTakeFacts, WhiteTakeState};

fn kelvin(value: f64) -> ColorTemperature {
    ColorTemperature::from_kelvin(value).expect("температура")
}

fn tint(duv: f64) -> Tint {
    Tint::from_duv(duv).expect("оттенок")
}

fn white(kelvin_value: f64, duv: f64) -> WhiteBalanceSetting {
    WhiteBalanceSetting {
        temperature: kelvin(kelvin_value),
        tint: tint(duv),
    }
}

fn held(origin: WhiteOrigin) -> HeldWhite {
    HeldWhite {
        setting: white(5_600.0, 0.0),
        origin,
    }
}

#[test]
fn kelvin_is_rounded_to_ten_only_for_display() {
    let config = WhiteScreenConfig::default();
    assert_eq!(kelvin_text(kelvin(3_140.0), &config), "3140");
    assert_eq!(kelvin_text(kelvin(3_143.0), &config), "3140");
    assert_eq!(kelvin_text(kelvin(3_146.0), &config), "3150");
    assert_eq!(kelvin_text(kelvin(12_000.0), &config), "12000");
    // Шаг показа берётся из конфига, а не зашит.
    let coarse = WhiteScreenConfig {
        display_step_kelvin: 100.0,
        ..WhiteScreenConfig::default()
    };
    assert_eq!(kelvin_text(kelvin(3_146.0), &coarse), "3100");
}

#[test]
fn tint_is_a_signed_whole_number_with_a_real_minus_and_a_bare_zero() {
    let config = WhiteScreenConfig::default();
    assert_eq!(tint_text(tint(0.0), &config), "0");
    assert_eq!(tint_text(tint(0.003), &config), "+3");
    assert_eq!(tint_text(tint(-0.012), &config), "\u{2212}12");
    // Меньше полуединицы показа — ноль без знака (никакого «−0»).
    assert_eq!(tint_text(tint(-0.0004), &config), "0");
    assert_eq!(tint_units(tint(0.05), &config), 50);
    let doubled = WhiteScreenConfig {
        tint_units_per_duv: 2_000.0,
        ..WhiteScreenConfig::default()
    };
    assert_eq!(tint_text(tint(0.003), &doubled), "+6");
}

#[test]
fn a_light_button_is_lit_by_equality_of_its_numbers_with_a_small_tolerance() {
    let config = WhiteScreenConfig::default();
    let sun = &config.lights[0];
    assert!(light_is_active(
        sun,
        white(5_600.0, 0.003),
        WhiteFollowFacts::Off,
        &config
    ));
    // Крошечная ошибка округления в движке не гасит кнопку.
    assert!(light_is_active(
        sun,
        white(5_601.0, 0.00301),
        WhiteFollowFacts::Off,
        &config
    ));
    // Шаг шкалы (5 майред ≈ 150 К на 5600) — уже другое белое.
    assert!(!light_is_active(
        sun,
        white(5_750.0, 0.003),
        WhiteFollowFacts::Off,
        &config
    ));
    // Тот же свет, другой оттенок — не она.
    assert!(!light_is_active(
        sun,
        white(5_600.0, 0.006),
        WhiteFollowFacts::Off,
        &config
    ));
    // Другая кнопка при тех же числах не горит.
    assert!(!light_is_active(
        &config.lights[1],
        white(5_600.0, 0.003),
        WhiteFollowFacts::Off,
        &config
    ));
}

#[test]
fn light_tolerance_is_measured_in_mired_so_it_is_tighter_in_kelvin_for_warm_light() {
    let config = WhiteScreenConfig::default();
    let lamp = &config.lights[3];
    // 3200 К: 0,5 майред ≈ 5 К. 8 К — уже мимо; 3 К — ещё она.
    assert!(light_is_active(
        lamp,
        white(3_203.0, 0.0),
        WhiteFollowFacts::Off,
        &config
    ));
    assert!(!light_is_active(
        lamp,
        white(3_208.0, 0.0),
        WhiteFollowFacts::Off,
        &config
    ));
}

#[test]
fn phone_is_lit_only_when_the_white_came_from_the_phone() {
    assert!(phone_is_active(
        held(WhiteOrigin::Source(WhiteSourceId::PHONE)),
        WhiteFollowFacts::Off
    ));
    assert!(!phone_is_active(
        held(WhiteOrigin::Source(WhiteSourceId::new("net"))),
        WhiteFollowFacts::Off
    ));
    assert!(!phone_is_active(held(WhiteOrigin::Manual), WhiteFollowFacts::Off));
    assert!(!phone_is_active(held(WhiteOrigin::Fallback), WhiteFollowFacts::Off));
}

#[test]
fn the_number_is_yellow_only_for_a_manual_white() {
    assert!(number_is_manual(held(WhiteOrigin::Manual), WhiteFollowFacts::Off));
    assert!(!number_is_manual(held(WhiteOrigin::Fallback), WhiteFollowFacts::Off));
    assert!(!number_is_manual(
        held(WhiteOrigin::Source(WhiteSourceId::PHONE)),
        WhiteFollowFacts::Off
    ));
}

#[test]
fn auto_is_lit_exactly_while_follow_is_on() {
    assert!(auto_is_active(WhiteFollowFacts::On));
    assert!(!auto_is_active(WhiteFollowFacts::Off));
}

#[test]
fn while_follow_is_on_phone_and_every_light_are_dark_even_if_their_numbers_match() {
    let config = WhiteScreenConfig::default();
    let sun = &config.lights[0];
    // Происхождение «телефон» и числа Sun совпали бы, но слежение включено: горит только «Auto».
    assert!(!phone_is_active(
        held(WhiteOrigin::Source(WhiteSourceId::PHONE)),
        WhiteFollowFacts::On
    ));
    assert!(!light_is_active(
        sun,
        white(5_600.0, 0.003),
        WhiteFollowFacts::On,
        &config
    ));
    // Без слежения те же факты горят (защита от «тест проходит, потому что функция всегда false»).
    assert!(light_is_active(
        sun,
        white(5_600.0, 0.003),
        WhiteFollowFacts::Off,
        &config
    ));
    assert!(phone_is_active(
        held(WhiteOrigin::Source(WhiteSourceId::PHONE)),
        WhiteFollowFacts::Off
    ));
}

#[test]
fn while_follow_is_on_the_number_is_white_even_before_the_first_answer_of_the_source() {
    // Держимое происхождение ещё «ручное» (ответа на включение не было), но число уже белое.
    assert!(!number_is_manual(held(WhiteOrigin::Manual), WhiteFollowFacts::On));
    assert!(number_is_manual(held(WhiteOrigin::Manual), WhiteFollowFacts::Off));
}

#[test]
fn the_unit_under_the_number_is_auto_only_while_follow_is_on() {
    assert_eq!(white_unit_label(WhiteFollowFacts::Off), "K");
    assert_eq!(white_unit_label(WhiteFollowFacts::On), "AUTO");
}

#[test]
fn the_phone_estimate_is_shown_only_for_a_measured_reading() {
    let config = WhiteScreenConfig::default();
    let measured = WhiteReading::Measured {
        white: white(3_104.0, 0.001),
    };
    assert_eq!(phone_estimate_text(Some(measured), &config).as_deref(), Some("≈ 3100"));
    assert_eq!(phone_estimate_text(Some(WhiteReading::NotMeasuredYet), &config), None);
    assert_eq!(
        phone_estimate_text(
            Some(WhiteReading::Unavailable {
                reason: WhiteUnavailableReason::AutomationSilent
            }),
            &config
        ),
        None
    );
    assert_eq!(phone_estimate_text(None, &config), None);
}

#[test]
fn the_button_waits_only_for_its_own_take_command() {
    let take = |state| Some(WhiteTakeFacts { serial: 1, state });
    // Заявки открытия своей команды не создают: `take` пуст — не мерцает.
    assert_eq!(button_manner(None), WhiteButtonManner::Normal);
    assert_eq!(button_manner(take(WhiteTakeState::Waiting)), WhiteButtonManner::Waiting);
    for settled in [
        WhiteTakeState::Taken,
        WhiteTakeState::Refused,
        WhiteTakeState::Cancelled,
    ] {
        assert_eq!(button_manner(take(settled)), WhiteButtonManner::Normal, "{settled:?}");
    }
}

/// Каждый символ каждого текста панели белого обязан лежать в атласе шрифта: символ вне атласа роняет кадр целиком.
#[test]
fn every_character_of_the_white_panel_is_in_the_atlas() {
    use crate::white_config::first_character_outside_atlas;
    use crate::white_display::{APPROX_SIGN, AUTO_LABEL, AUTO_UNIT, KELVIN_UNIT, PHONE_LABEL, TINT_LABEL_PREFIX};

    let config = WhiteScreenConfig::default();
    // Все цифры, знаки числа, единица, «Tint», знак «минус» U+2212 и «≈» из самого показа, а не из копии в тесте.
    let mut texts = vec![
        "0123456789".to_owned(),
        "+ ".to_owned(),
        KELVIN_UNIT.to_owned(),
        TINT_LABEL_PREFIX.to_owned(),
        PHONE_LABEL.to_owned(),
        AUTO_LABEL.to_owned(),
        AUTO_UNIT.to_owned(),
        APPROX_SIGN.to_owned(),
        tint_text(tint(-0.012), &config),
        tint_text(tint(0.0164), &config),
    ];
    let measured = WhiteReading::Measured {
        white: white(3_140.0, 0.0),
    };
    texts.extend(phone_estimate_text(Some(measured), &config));
    texts.extend(config.lights.iter().map(|light| light.label.clone()));
    for text in &texts {
        assert_eq!(
            first_character_outside_atlas(text),
            None,
            "текст «{text}» не лежит в атласе"
        );
    }
    assert!(
        tint_text(tint(-0.012), &config).contains('\u{2212}'),
        "минус именно U+2212"
    );
    // Ловушка самой проверки: символ вне атласа она находит.
    assert_eq!(first_character_outside_atlas("Sun\u{2603}"), Some('\u{2603}'));
}

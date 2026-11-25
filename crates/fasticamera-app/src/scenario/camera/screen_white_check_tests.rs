//! Правила проверок белого шагов `screen`: каждое нарушение называется, честный отказ не краснеет, подмена не проходит.

use camera_contract::control::{ColorTemperature, Tint, WhiteBalanceSetting};
use camera_contract::white::{HeldWhite, WhiteOrigin, WhiteSourceId};
use camera_screen::{WhiteTakeFacts, WhiteTakeState};

use super::*;

fn held(kelvin: f64, duv: f64, origin: WhiteOrigin) -> HeldWhite {
    HeldWhite {
        setting: WhiteBalanceSetting {
            temperature: ColorTemperature::from_kelvin(kelvin).expect("температура"),
            tint: Tint::from_duv(duv).expect("оттенок"),
        },
        origin,
    }
}

fn view(kelvin: u32, origin: WhiteOrigin, refusals: u64) -> WhiteView {
    WhiteView {
        origin,
        kelvin,
        refusals,
    }
}

const PHONE: WhiteOrigin = WhiteOrigin::Source(WhiteSourceId::PHONE);

/// Шаг «взять»: белое было Manual 5600, команда номер 8 (до шага — номер 7).
fn take_delta(state: WhiteTakeState, held_now: HeldWhite, refusals_now: u64) -> WhiteDelta {
    let before = held(5_600.0, 0.003, WhiteOrigin::Manual);
    WhiteDelta {
        held_before: before,
        held_now,
        take_now: Some(WhiteTakeFacts { serial: 8, state }),
        take_serial_before: Some(7),
        view_before: view(5_600, WhiteOrigin::Manual, 0),
        view_now: view(
            held_now.setting.temperature.kelvin().round() as u32,
            held_now.origin,
            refusals_now,
        ),
    }
}

#[test]
fn a_hand_set_white_must_carry_the_button_numbers_and_the_manual_origin() {
    let check = WhiteCheck::SetByHand {
        kelvin: 5_600.0,
        duv: 0.003,
    };
    let good = take_delta(WhiteTakeState::Taken, held(5_600.0, 0.003, WhiteOrigin::Manual), 0);
    assert!(check.holds(&good));
    // Другая кнопка (Cloud 6500), то же происхождение: промах по соседней кнопке не проходит.
    let neighbour = take_delta(WhiteTakeState::Taken, held(6_500.0, 0.003, WhiteOrigin::Manual), 0);
    assert!(!check.holds(&neighbour));
    // Те же числа, но их поставил источник, а не рука.
    let by_source = take_delta(WhiteTakeState::Taken, held(5_600.0, 0.003, PHONE), 0);
    assert!(!check.holds(&by_source));
    // Нет кнопки (число NaN): шаг заведомо краснеет.
    let missing = WhiteCheck::SetByHand {
        kelvin: f64::NAN,
        duv: f64::NAN,
    };
    assert!(!missing.holds(&good));
}

#[test]
fn the_screen_must_show_the_number_and_origin_the_engine_holds() {
    let check = WhiteCheck::ScreenShowsEngineWhite;
    let mut delta = take_delta(WhiteTakeState::Taken, held(3_140.0, 0.0, PHONE), 0);
    assert!(check.holds(&delta));
    delta.view_now.kelvin = 5_600;
    let problem = check.problem(&delta).expect("расхождение числа");
    assert!(problem.contains("3140") && problem.contains("5600"), "{problem}");
    delta.view_now = view(3_140, WhiteOrigin::Manual, 0);
    assert!(!check.holds(&delta), "число то же, а происхождение на экране другое");
}

#[test]
fn a_taken_white_must_come_from_the_phone_source_and_not_shake_the_screen() {
    let check = WhiteCheck::PhoneTakeSettled;
    assert!(check.holds(&take_delta(WhiteTakeState::Taken, held(3_140.0, 0.01, PHONE), 0)));
    // «Взято», а происхождение осталось рукой: ответ не применился.
    assert!(!check.holds(&take_delta(
        WhiteTakeState::Taken,
        held(3_140.0, 0.01, WhiteOrigin::Manual),
        0
    )));
    // «Взято», но экран отметил отказ.
    assert!(!check.holds(&take_delta(WhiteTakeState::Taken, held(3_140.0, 0.01, PHONE), 1)));
}

#[test]
fn an_honest_refusal_passes_but_only_with_the_white_unchanged_and_one_shake() {
    let check = WhiteCheck::PhoneTakeSettled;
    let unchanged = held(5_600.0, 0.003, WhiteOrigin::Manual);
    assert!(check.holds(&take_delta(WhiteTakeState::Refused, unchanged, 1)));
    // Отказ без тряски экрана: пользователь ничего не заметил.
    assert!(!check.holds(&take_delta(WhiteTakeState::Refused, unchanged, 0)));
    // Двойная тряска на одну команду.
    assert!(!check.holds(&take_delta(WhiteTakeState::Refused, unchanged, 2)));
    // Отказ, но белое при этом изменилось.
    assert!(!check.holds(&take_delta(WhiteTakeState::Refused, held(4_000.0, 0.0, PHONE), 1)));
}

#[test]
fn an_unfinished_old_or_cancelled_take_is_never_a_settled_outcome() {
    let check = WhiteCheck::PhoneTakeSettled;
    let unchanged = held(5_600.0, 0.003, WhiteOrigin::Manual);
    assert!(!check.holds(&take_delta(WhiteTakeState::Waiting, unchanged, 0)));
    assert!(!check.holds(&take_delta(WhiteTakeState::Cancelled, unchanged, 0)));
    // Исход старой команды (номер тот же, что до шага) за исход этого шага не считается.
    let mut stale = take_delta(WhiteTakeState::Refused, unchanged, 1);
    stale.take_serial_before = Some(8);
    assert!(!check.holds(&stale));
    // Команды не было вовсе.
    stale.take_now = None;
    assert!(!check.holds(&stale));
}

#[test]
fn the_verdict_word_tells_taken_from_refused_for_the_report() {
    let unchanged = held(5_600.0, 0.003, WhiteOrigin::Manual);
    assert_eq!(
        take_verdict_word(&take_delta(WhiteTakeState::Taken, unchanged, 0)),
        "взято"
    );
    assert_eq!(
        take_verdict_word(&take_delta(WhiteTakeState::Refused, unchanged, 1)),
        "отказ"
    );
    let mut none = take_delta(WhiteTakeState::Refused, unchanged, 1);
    none.take_now = None;
    assert_eq!(take_verdict_word(&none), "нет");
}

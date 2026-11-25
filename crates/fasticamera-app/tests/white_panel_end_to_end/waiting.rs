//! Сценарий e: пока источник молчит, число на кнопке мерцает; ручное действие во время ожидания (кнопка света или
//! шкала) побеждает: кадр = ручное, поздний ответ источника ничего не меняет, тряски нет.

use camera_contract::auto_exposure::CameraTag;
use camera_contract::fixtures::camera_id;
use camera_contract::white::{SceneIlluminant, WhiteAnswer, WhiteConfidence, WhiteOrigin, WhiteSourceOutcome};
use camera_core::white::{WhiteCancelCause, WhiteRequestOutcome, WhiteRequestState, WhiteTakeState};
use shell_contract::haptics::HapticPattern;
use ui_draw::draw_list::DrawList;

use super::fake_source::Reaction;
use super::panel_view::strip_number_any_opacity;
use super::session::{OPENING_KELVIN, WhiteSession, white_of};

/// Прозрачность числа на кнопке (число с заданными Кельвинами).
fn number_opacity_of(kelvin: f64) -> impl Fn(&DrawList) -> Option<f32> {
    move |list| strip_number_any_opacity(list, kelvin).map(|item| item.opacity.get())
}

fn number_x_of(kelvin: f64) -> impl Fn(&DrawList) -> Option<f32> {
    move |list| strip_number_any_opacity(list, kelvin).map(|item| item.transform.tx)
}

/// Разброс значений: максимум минус минимум.
fn spread(values: &[Option<f32>]) -> f32 {
    let present: Vec<f32> = values.iter().flatten().copied().collect();
    let max = present.iter().copied().fold(f32::MIN, f32::max);
    let min = present.iter().copied().fold(f32::MAX, f32::min);
    max - min
}

/// Номер заявки, которую источник ещё не ответил.
fn waiting_request(session: &WhiteSession) -> camera_contract::white::WhiteRequestId {
    match session.stand.status().three_a.white.request {
        WhiteRequestState::Waiting { request, .. } => request,
        other => panic!("ждём ответа источника: {other:?}"),
    }
}

/// Поздний ответ на СТАРУЮ заявку приходит в общий ящик ответов (так он пришёл бы из рендера).
fn publish_late_answer(session: &WhiteSession, request: camera_contract::white::WhiteRequestId, kelvin: f64) {
    session.stand.engine.answers.publish(WhiteAnswer {
        request,
        camera: CameraTag::of(&camera_id("2")),
        measured_frame: None,
        outcome: WhiteSourceOutcome::Ready {
            illuminant: SceneIlluminant::Uniform(white_of(kelvin, 0.0)),
            confidence: WhiteConfidence::FULL,
        },
    });
}

fn start_waiting() -> Option<WhiteSession> {
    let mut session = WhiteSession::open()?;
    session.source.react_with(Reaction::Silent);
    session.long_press_button();
    session.frames(4);
    Some(session)
}

#[test]
fn while_the_source_is_silent_the_number_on_the_button_flickers() {
    let Some(mut session) = start_waiting() else {
        return;
    };
    // Заявка ждёт: число мерцает, то есть его прозрачность меняется от кадра к кадру.
    let _ = waiting_request(&session);
    let opacities = session.watch(40, number_opacity_of(OPENING_KELVIN));
    assert!(spread(&opacities) > 0.1, "число мерцает: {opacities:?}");
    assert_eq!(
        session.held().origin,
        WhiteOrigin::Source(super::session::PHONE),
        "белое пока прежнее"
    );
    session.stand.finish();
}

#[test]
fn a_light_button_pressed_while_waiting_wins_and_the_late_answer_changes_nothing() {
    let Some(mut session) = start_waiting() else {
        return;
    };
    let old_request = waiting_request(&session);
    session.tap_button();
    session.tap_light(0);
    let held = session.wait_origin("рука победила ожидание", WhiteOrigin::Manual);
    assert_eq!(held.setting, session.light_setting(0));

    publish_late_answer(&session, old_request, 9_000.0);
    session.stand.wait_status("поздний ответ признан чужим", |status| {
        status.three_a.white.stale_answers >= 1
    });
    let status = session.stand.status();
    assert_eq!(
        status.three_a.white.last_take.map(|take| take.state),
        Some(WhiteTakeState::Finished(WhiteRequestOutcome::Cancelled {
            by: WhiteCancelCause::ManualWhite
        })),
        "ожидание закрыто рукой"
    );
    assert_eq!(session.held(), held, "поздний ответ белое не сдвинул");
    session.assert_picture_is(held.setting, "ручное сильнее ожидания");

    // Мерцание кончилось, тряски нет, вибрации ошибки нет.
    let kelvin = held.setting.temperature.kelvin();
    let still = session.watch(40, number_x_of(kelvin));
    assert!(spread(&still) < 0.01, "тряски нет: {still:?}");
    let opacities = session.watch(10, number_opacity_of(kelvin));
    assert!(
        opacities.iter().flatten().all(|opacity| *opacity > 0.95),
        "мерцания нет: {opacities:?}"
    );
    assert!(!session.stand.haptics_played().contains(&HapticPattern::Error));
    session.stand.finish();
}

#[test]
fn the_scale_dragged_while_waiting_wins_and_the_late_answer_changes_nothing() {
    let Some(mut session) = start_waiting() else {
        return;
    };
    let old_request = waiting_request(&session);
    session.open_card();
    session.drag_ruler(-2.0, 40);
    let held = session.wait_origin("шкала победила ожидание", WhiteOrigin::Manual);
    assert!(
        held.setting.temperature.kelvin() > OPENING_KELVIN,
        "палец влево поднял Кельвины"
    );

    publish_late_answer(&session, old_request, 9_000.0);
    session.stand.wait_status("поздний ответ признан чужим", |status| {
        status.three_a.white.stale_answers >= 1
    });
    assert_eq!(session.held(), held, "поздний ответ белое не сдвинул");
    session.assert_picture_is(held.setting, "шкала сильнее ожидания");
    assert!(
        !session.stand.haptics_played().contains(&HapticPattern::Error),
        "тряски и вибрации ошибки нет"
    );
    // Своей заявки больше нет: мерцания нет.
    session.stand.finish();
}

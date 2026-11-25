//! Сценарий d: отказ источника (не знает, нет, сломался, не успел, не спрашивали вовсе). В каждом случае кадр
//! бит-в-бит прежний, вибрация «ошибка» одна, тряска видна на кнопке при закрытой панели и на «Phone» в панели,
//! а потом всё возвращается на место.

use camera_contract::white::WhiteSourceOutcome;
use camera_contract::white::{WhiteOrigin, WhiteSourceFailure, WhiteUnavailableReason, WhiteUnknownReason};
use camera_core::white::{WhiteRequestOutcome, WhiteTakeState};
use scene_estimation::white_source::WhiteRequestRefusal;
use shell_contract::haptics::HapticPattern;
use ui_draw::draw_list::DrawList;

use super::fake_source::Reaction;
use super::panel_view::{caption, strip_number_any_opacity};
use super::session::{CARD_SETTLE_UI_FRAMES, OPENING_KELVIN, WhiteSession, opening_setting};

/// Кадров, за которые тряска успевает начаться и кончиться (тряска короче секунды).
const SHAKE_WATCH_FRAMES: i64 = 70;
/// Пауза между кадрами, пока ждём исход: срок ожидания источника идёт по настоящим часам движка (3 с), а кадров сцены
/// конечное число — гнать их без пауз значит исчерпать сцену раньше срока.
const POLL_PAUSE: std::time::Duration = std::time::Duration::from_millis(50);
/// Сдвиг, который считается тряской, px: заметно больше погрешности округления.
const SHAKE_MIN_SPREAD_PX: f32 = 1.0;

/// Что даёт каждый случай отказа: реакция источника и исход в статусе движка.
fn refusal_cases() -> Vec<(&'static str, Reaction, WhiteRequestOutcome)> {
    vec![
        (
            "не знаю",
            Reaction::Answer {
                outcome: WhiteSourceOutcome::Unknown {
                    reason: WhiteUnknownReason::SceneNotMeasurable,
                },
                after_frames: 1,
            },
            WhiteRequestOutcome::Unknown {
                reason: WhiteUnknownReason::SceneNotMeasurable,
            },
        ),
        (
            "источника нет",
            Reaction::Refuse(WhiteRequestRefusal::Unavailable(
                WhiteUnavailableReason::AutomationSilent,
            )),
            WhiteRequestOutcome::Unavailable {
                reason: WhiteUnavailableReason::AutomationSilent,
            },
        ),
        (
            "сломался",
            Reaction::Answer {
                outcome: WhiteSourceOutcome::Failed {
                    failure: WhiteSourceFailure::Executor,
                },
                after_frames: 1,
            },
            WhiteRequestOutcome::Failed {
                failure: WhiteSourceFailure::Executor,
            },
        ),
        ("не успел", Reaction::Silent, WhiteRequestOutcome::TimedOut),
    ]
}

/// Горизонтальное положение числа на кнопке сверху (px; нет числа — `None`).
fn strip_number_x(list: &DrawList) -> Option<f32> {
    strip_number_any_opacity(list, OPENING_KELVIN).map(|item| item.transform.tx)
}

/// Горизонтальное положение подписи «Phone» на карточке (px).
fn phone_caption_x(list: &DrawList) -> Option<f32> {
    caption(list, "Phone").first().map(|item| item.transform.tx)
}

/// Тряска видна, если положение хоть раз заметно уходило от покоя, а в конце вернулось на место.
fn assert_shook_and_returned(positions: &[Option<f32>], rest: f32, what: &str) {
    let moved = positions
        .iter()
        .flatten()
        .any(|position| (position - rest).abs() >= SHAKE_MIN_SPREAD_PX);
    assert!(moved, "{what}: тряски не было, положения {positions:?}");
    let last = positions.last().copied().flatten().expect("кнопка осталась на экране");
    assert!(
        (last - rest).abs() < 0.01,
        "{what}: после тряски кнопка на месте ({last} против {rest})"
    );
}

/// Гоним кадры, пока исход команды «взять» не закрылся (источнику нужны кадры камеры, а срок ожидания идёт по
/// настоящим часам движка), потом ещё `SHAKE_WATCH_FRAMES` кадров: тряска начинается, когда экран увидел исход.
/// Возвращает положения `probe` на каждом кадре.
fn watch_until_the_take_is_over(
    session: &mut WhiteSession,
    probe: impl Fn(&DrawList) -> Option<f32>,
) -> Vec<Option<f32>> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let mut positions = Vec::new();
    loop {
        positions.extend(session.watch(1, &probe));
        let over = session
            .stand
            .status()
            .three_a
            .white
            .last_take
            .is_some_and(|take| matches!(take.state, WhiteTakeState::Finished(_)));
        if over {
            break;
        }
        assert!(std::time::Instant::now() < deadline, "исход команды так и не закрылся");
        std::thread::sleep(POLL_PAUSE);
    }
    positions.extend(session.watch(SHAKE_WATCH_FRAMES, &probe));
    positions
}

fn errors_played(session: &WhiteSession) -> usize {
    session
        .stand
        .haptics_played()
        .iter()
        .filter(|pattern| **pattern == HapticPattern::Error)
        .count()
}

/// Закрытая панель: долгое нажатие на кнопку; тряска видна на самом кнопке.
fn closed_panel_refusal(name: &str, reaction: Reaction, expected: WhiteRequestOutcome) {
    let Some(mut session) = WhiteSession::open() else {
        return;
    };
    let held_before = session.held();
    let rest = strip_number_x(session.stand.ui_list()).expect("число на кнопке стоит");
    session.source.react_with(reaction);

    session.long_press_button();
    let shake = watch_until_the_take_is_over(&mut session, strip_number_x);
    let take = session.stand.status().three_a.white.last_take.expect("исход");
    assert_eq!(
        take.state,
        WhiteTakeState::Finished(expected),
        "{name}: исход различим в статусе"
    );
    assert_eq!(session.held(), held_before, "{name}: белое прежнее");
    assert_eq!(errors_played(&session), 1, "{name}: одна вибрация ошибки");
    assert_shook_and_returned(&shake, rest, &format!("{name}, кнопка при закрытой панели"));
    session.assert_picture_is(opening_setting(), &format!("{name}: кадр прежний"));
    session.stand.finish();
}

/// Открытая панель: тап «Phone»; тряска видна на самой кнопке «Phone».
fn open_panel_refusal(name: &str, reaction: Reaction, expected: WhiteRequestOutcome) {
    let Some(mut session) = WhiteSession::open() else {
        return;
    };
    session.open_card();
    let held_before = session.held();
    let rest = phone_caption_x(session.stand.ui_list()).expect("подпись «Phone» стоит");
    session.source.react_with(reaction);

    session.tap_phone();
    let shake = watch_until_the_take_is_over(&mut session, phone_caption_x);
    let take = session.stand.status().three_a.white.last_take.expect("исход");
    assert_eq!(
        take.state,
        WhiteTakeState::Finished(expected),
        "{name}: исход различим в статусе"
    );
    assert_eq!(session.held(), held_before, "{name}: белое прежнее");
    assert_eq!(errors_played(&session), 1, "{name}: одна вибрация ошибки");
    assert_shook_and_returned(&shake, rest, &format!("{name}, кнопка Phone в панели"));
    session.assert_picture_is(opening_setting(), &format!("{name}: кадр прежний"));
    session.stand.finish();
}

#[test]
fn a_refusal_shakes_the_button_on_the_closed_panel_buzzes_once_and_leaves_the_frame_untouched() {
    for (name, reaction, expected) in refusal_cases() {
        closed_panel_refusal(name, reaction, expected);
    }
}

#[test]
fn a_refusal_shakes_the_phone_button_in_the_open_panel_buzzes_once_and_leaves_the_frame_untouched() {
    for (name, reaction, expected) in refusal_cases() {
        open_panel_refusal(name, reaction, expected);
    }
}

/// Поток закрыт: команда не доходит до источника (`NotAsked`), но человек всё равно видит отказ. Белое сначала
/// ставит рука, чтобы возобновление камеры не спрашивало телефон само и «кадр прежний» можно было проверить.
#[test]
fn with_the_stream_closed_the_command_is_not_asked_yet_the_refusal_is_just_as_visible() {
    let Some(mut session) = WhiteSession::open() else {
        return;
    };
    session.open_card();
    session.tap_light(0);
    let manual = session.wait_origin("рука", WhiteOrigin::Manual);
    session.frames(4);
    session.tap_button();
    let requests_before = session.source.requests().len();
    session.stand.pause();
    session.stand.ui_frames(CARD_SETTLE_UI_FRAMES);
    // Покой меряем уже при закрытом потоке: ряд значков без камеры может стоять иначе, чем при живой.
    let rest = {
        let list = session.stand.ui_list();
        let kelvin = manual.setting.temperature.kelvin();
        strip_number_any_opacity(list, kelvin)
            .expect("число на кнопке")
            .transform
            .tx
    };

    session.long_press_button();
    let mut positions = Vec::new();
    for _ in 0..SHAKE_WATCH_FRAMES {
        session.stand.ui_frames(1);
        let kelvin = manual.setting.temperature.kelvin();
        positions.push(strip_number_any_opacity(session.stand.ui_list(), kelvin).map(|item| item.transform.tx));
    }
    let take = session
        .stand
        .status()
        .three_a
        .white
        .last_take
        .expect("команда дошла до движка");
    assert!(matches!(take.state, WhiteTakeState::NotAsked { .. }), "{take:?}");
    assert_eq!(
        session.source.requests().len(),
        requests_before,
        "источнику ничего не передавали"
    );
    assert_eq!(session.held(), manual, "белое прежнее");
    assert_eq!(errors_played(&session), 1);
    assert_shook_and_returned(&positions, rest, "закрытый поток");

    // Камера вернулась: белое рука держит, кадр = эталон прежнего белого.
    session.stand.resume();
    session.frames(2);
    session.assert_picture_is(manual.setting, "кадр после возврата потока");
    session.stand.finish();
}

//! Тесты экспозиции: квантование 1/3 EV, пределы, эффекты и отсутствие эха.

use camera_contract::control::ExposureBias;

use crate::action::ScreenAction;
use crate::effect::{Effect, ScreenCameraIntent};
use crate::exposure::{ExposureChange, ExposureRangeFact};
use crate::facts::CameraFacts;
use crate::focus::FocusIndicator;
use crate::outcome::{ActionOutcome, NoChangeReason, Rejection};
use crate::test_support::{Screen, at_ms, live_facts};

fn bias(ev: f32) -> ExposureBias {
    ExposureBias::from_ev(ev).expect("конечное значение")
}

fn drag(screen: &mut Screen, ev: f32) -> crate::ScreenTransition {
    screen.step(ScreenAction::ExposureDragged {
        requested: bias(ev),
        at: at_ms(0),
    })
}

fn sent_bias(effects: &[Effect]) -> f32 {
    match effects {
        [Effect::Camera(ScreenCameraIntent::SetExposureBias(value))] => value.ev(),
        other => panic!("ожидалось одно SetExposureBias, получено {other:?}"),
    }
}

#[test]
fn drag_is_quantized_to_third_ev_steps_and_sent_to_the_camera() {
    let mut screen = Screen::ready();

    let transition = drag(&mut screen, 0.4);
    assert_eq!(transition.outcome, ActionOutcome::Applied);
    assert_eq!(screen.state.exposure().steps(), 1);
    assert!((sent_bias(&transition.effects) - 1.0 / 3.0).abs() < 1e-6);

    let transition = drag(&mut screen, -1.0);
    assert_eq!(screen.state.exposure().steps(), -3);
    assert!((sent_bias(&transition.effects) + 1.0).abs() < 1e-6);
}

#[test]
fn drag_is_clamped_to_plus_minus_two_ev() {
    let mut screen = Screen::ready();
    let transition = drag(&mut screen, 9.0);
    assert_eq!(screen.state.exposure().steps(), 6);
    assert!((sent_bias(&transition.effects) - 2.0).abs() < 1e-5);
    let transition = drag(&mut screen, -9.0);
    assert_eq!(screen.state.exposure().steps(), -6);
    assert!((sent_bias(&transition.effects) + 2.0).abs() < 1e-5);
}

#[test]
fn drag_to_the_same_quantized_value_sends_nothing() {
    let mut screen = Screen::ready();
    drag(&mut screen, 0.33);
    // 0,30 и 0,36 квантуются к тому же шагу: повторной отправки в камеру нет.
    let transition = drag(&mut screen, 0.30);
    assert_eq!(
        transition.outcome,
        ActionOutcome::NoChange(NoChangeReason::ValueUnchanged)
    );
    assert!(transition.effects.is_empty());
}

#[test]
fn programmatic_restore_changes_the_value_without_any_camera_effect() {
    let mut screen = Screen::ready();

    let transition = screen.step(ScreenAction::ExposureRestored { bias: bias(1.0) });

    assert!(transition.effects.is_empty(), "эхо в камеру запрещено");
    assert_eq!(screen.state.exposure().steps(), 3);
    // Значение квантуется и ограничивается так же, как у пользователя.
    screen.step(ScreenAction::ExposureRestored { bias: bias(7.0) });
    assert_eq!(screen.state.exposure().steps(), 6);
}

#[test]
fn camera_range_is_intersected_with_config_limits() {
    let mut screen = Screen::ready();
    screen.step(ScreenAction::CameraFactsChanged(CameraFacts {
        exposure_range: ExposureRangeFact::Supported {
            min_ev: -1.0,
            max_ev: 3.0,
        },
        ..live_facts()
    }));
    let limits = screen.state.exposure().limits();
    assert_eq!(
        (limits.min_steps(), limits.max_steps()),
        (-3, 6),
        "нижний — камера, верхний — конфиг"
    );

    drag(&mut screen, 2.0);
    assert_eq!(screen.state.exposure().steps(), 6);
    drag(&mut screen, -2.0);
    assert_eq!(screen.state.exposure().steps(), -3);
}

#[test]
fn narrowing_camera_range_pulls_the_value_in_and_tells_the_camera() {
    let mut screen = Screen::ready();
    drag(&mut screen, 2.0);
    assert_eq!(screen.state.exposure().steps(), 6);

    let transition = screen.step(ScreenAction::CameraFactsChanged(CameraFacts {
        exposure_range: ExposureRangeFact::Supported {
            min_ev: -1.0,
            max_ev: 1.0,
        },
        ..live_facts()
    }));

    assert_eq!(screen.state.exposure().steps(), 3);
    assert!(
        (sent_bias(&transition.effects) - 1.0).abs() < 1e-5,
        "камера должна узнать о сдвиге"
    );
}

#[test]
fn camera_without_compensation_rejects_the_drag() {
    let mut screen = Screen::ready();
    screen.step(ScreenAction::CameraFactsChanged(CameraFacts {
        exposure_range: ExposureRangeFact::Unsupported,
        ..live_facts()
    }));
    let transition = drag(&mut screen, 1.0);
    assert_eq!(
        transition.outcome,
        ActionOutcome::Rejected(Rejection::ExposureUnsupported)
    );
    assert!(transition.effects.is_empty());
    assert_eq!(screen.state.exposure().steps(), 0);
    assert_eq!(screen.state.rejections().get(Rejection::ExposureUnsupported), 1);
}

#[test]
fn drag_without_a_live_camera_is_rejected() {
    let mut screen = Screen::unprepared(crate::ScreenConfig::default());
    let transition = drag(&mut screen, 1.0);
    assert_eq!(transition.outcome, ActionOutcome::Rejected(Rejection::CameraNotLive));
    assert_eq!(screen.state.exposure().steps(), 0);
}

#[test]
fn reset_returns_to_zero_and_tells_the_camera_only_when_there_is_something_to_reset() {
    let mut screen = Screen::ready();
    let idle = screen.step(ScreenAction::ExposureResetRequested);
    assert_eq!(idle.outcome, ActionOutcome::NoChange(NoChangeReason::ValueUnchanged));
    assert!(idle.effects.is_empty());

    drag(&mut screen, 1.0);
    let transition = screen.step(ScreenAction::ExposureResetRequested);
    assert_eq!(screen.state.exposure().steps(), 0);
    assert_eq!(sent_bias(&transition.effects), 0.0);
}

#[test]
fn dragging_the_sun_keeps_the_focus_indicator_alive() {
    let mut screen = Screen::ready();
    screen.tap_center(at_ms(0));
    screen.step(ScreenAction::ExposureDragged {
        requested: bias(0.7),
        at: at_ms(2_500),
    });

    // Без продления квадрат погас бы на 3-й секунде (3000 мс по умолчанию).
    screen.step(ScreenAction::Tick(at_ms(4_000)));
    assert!(matches!(screen.state.focus().indicator(), FocusIndicator::Shown { .. }));
    screen.step(ScreenAction::Tick(at_ms(5_600)));
    assert_eq!(screen.state.focus().indicator(), FocusIndicator::Hidden);
}

#[test]
fn holding_the_sun_without_a_step_change_still_keeps_the_indicator_alive_and_tells_the_camera_nothing() {
    let mut screen = Screen::ready();
    screen.tap_center(at_ms(0));
    drag(&mut screen, 1.0);
    let steps = screen.state.exposure().steps();

    // Палец внутри того же шага: значение то же, но срок продлевается (3000 мс считаются от 2500).
    let same_value = screen.step(ScreenAction::ExposureDragged {
        requested: bias(1.0),
        at: at_ms(2_500),
    });
    assert_eq!(
        same_value.outcome,
        ActionOutcome::NoChange(NoChangeReason::ValueUnchanged)
    );
    // Событие пана без запрошенного значения (палец у предела или в пределах шага).
    let held = screen.step(ScreenAction::FocusSunHeld { at: at_ms(4_000) });
    assert_eq!(held.outcome, ActionOutcome::Applied);
    assert!(
        held.effects.is_empty(),
        "удержание «солнца» не должно ничего слать камере"
    );
    assert_eq!(screen.state.exposure().steps(), steps);

    screen.step(ScreenAction::Tick(at_ms(6_500)));
    assert!(matches!(screen.state.focus().indicator(), FocusIndicator::Shown { .. }));
    screen.step(ScreenAction::Tick(at_ms(7_100)));
    assert_eq!(screen.state.focus().indicator(), FocusIndicator::Hidden);
}

#[test]
fn holding_the_sun_after_the_indicator_is_hidden_does_not_bring_it_back() {
    let mut screen = Screen::ready();
    screen.tap_center(at_ms(0));
    screen.step(ScreenAction::Tick(at_ms(3_100)));
    assert_eq!(screen.state.focus().indicator(), FocusIndicator::Hidden);

    let held = screen.step(ScreenAction::FocusSunHeld { at: at_ms(3_200) });
    assert_eq!(held.outcome, ActionOutcome::NoChange(NoChangeReason::ValueUnchanged));
    assert!(held.effects.is_empty());
    assert_eq!(screen.state.focus().indicator(), FocusIndicator::Hidden);
}

#[test]
fn exposure_panel_close_button_closes_only_the_exposure_panel() {
    let mut screen = Screen::ready();
    screen.step(ScreenAction::StripToolTapped(crate::tools::ToolId::Exposure));
    assert_eq!(screen.state.panel(), crate::PanelState::ExposurePanel);
    screen.step(ScreenAction::ExposurePanelCloseRequested);
    assert_eq!(screen.state.panel(), crate::PanelState::Closed);
    let transition = screen.step(ScreenAction::ExposurePanelCloseRequested);
    assert_eq!(
        transition.outcome,
        ActionOutcome::NoChange(NoChangeReason::ValueUnchanged)
    );
}

#[test]
fn user_setting_reports_whether_the_value_really_changed() {
    let mut screen = Screen::ready();
    let mut exposure = screen.state.exposure;
    assert_eq!(exposure.set_by_user(3), ExposureChange::Changed);
    assert_eq!(exposure.steps(), 3);
    assert_eq!(exposure.set_by_user(3), ExposureChange::Unchanged);
    assert_eq!(exposure.steps(), 3);
    // Состояние экрана при этом не тронуто: проверка работала на копии.
    assert_eq!(screen.state.exposure().steps(), 0);
    screen.step(ScreenAction::ExposureResetRequested);
}

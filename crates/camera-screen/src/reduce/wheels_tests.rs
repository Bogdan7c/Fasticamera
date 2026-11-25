//! Колёса цвета в редьюсере: выбор вида и колеса, толчок и кольцо (решения 204–207).
//! Проверяются наблюдаемые итоги: итог действия, точные точечные намерения наружу и то, что не должно меняться.

use look_contract::{ColorWheel, LookIntent, LookSettings, WheelPush, WheelZone};

use crate::action::ScreenAction;
use crate::calibration::ControlViewKind;
use crate::effect::Effect;
use crate::look_test_support::{knob, look_facts, look_facts_edited};
use crate::outcome::{ActionOutcome, NoChangeReason, Rejection};
use crate::test_support::Screen;

fn push(a: f32, b: f32) -> WheelPush {
    WheelPush::new(a, b).expect("точка теста внутри круга")
}

fn facts_with_wheel(zone: WheelZone, wheel: ColorWheel) -> look_contract::LookFacts {
    let mut settings = LookSettings::NEUTRAL;
    settings.wheels = settings.wheels.with_wheel(zone, wheel);
    look_facts_edited(settings)
}

// ---------------------------------------------------------------- выбор вида и колеса

#[test]
fn the_column_starts_on_scales_with_lift_selected_and_the_choice_changes_only_state() {
    let mut screen = Screen::calibrating(look_facts());
    assert_eq!(screen.state.calibration().control_view(), ControlViewKind::Scales);
    assert_eq!(screen.state.calibration().selected_wheel(), WheelZone::Lift);

    let view = screen.step(ScreenAction::ControlViewSelected(ControlViewKind::Wheels));
    let wheel = screen.step(ScreenAction::WheelSelected(WheelZone::Gain));

    assert_eq!(
        (view.outcome, wheel.outcome),
        (ActionOutcome::Applied, ActionOutcome::Applied)
    );
    assert!(
        view.effects.is_empty() && wheel.effects.is_empty(),
        "выбор — состояние экрана, а не намерение"
    );
    assert_eq!(screen.state.calibration().control_view(), ControlViewKind::Wheels);
    assert_eq!(screen.state.calibration().selected_wheel(), WheelZone::Gain);
}

#[test]
fn choosing_the_already_selected_view_or_wheel_is_a_no_change() {
    let mut screen = Screen::calibrating(look_facts());

    let same_view = screen.step(ScreenAction::ControlViewSelected(ControlViewKind::Scales));
    let same_wheel = screen.step(ScreenAction::WheelSelected(WheelZone::Lift));

    assert_eq!(
        same_view.outcome,
        ActionOutcome::NoChange(NoChangeReason::ValueUnchanged)
    );
    assert_eq!(
        same_wheel.outcome,
        ActionOutcome::NoChange(NoChangeReason::ValueUnchanged)
    );
}

#[test]
fn the_scales_scroll_position_is_remembered_only_in_the_section_and_changes_nothing_else() {
    let mut screen = Screen::calibrating(look_facts());
    assert_eq!(screen.state.calibration().scales_scroll_offset_pt(), 0.0);

    let first = screen.step(ScreenAction::ScalesScrollRemembered { offset_pt: 137.5 });
    let same = screen.step(ScreenAction::ScalesScrollRemembered { offset_pt: 137.5 });

    assert_eq!(first.outcome, ActionOutcome::Applied);
    assert!(
        first.effects.is_empty(),
        "запоминание позиции — состояние экрана, не намерение"
    );
    assert_eq!(same.outcome, ActionOutcome::NoChange(NoChangeReason::ValueUnchanged));
    assert_eq!(screen.state.calibration().scales_scroll_offset_pt(), 137.5);
    assert_eq!(
        screen.state.calibration().control_view(),
        ControlViewKind::Scales,
        "вид и выбранное колесо не тронуты"
    );
    assert_eq!(screen.state.calibration().selected_wheel(), WheelZone::Lift);

    let mut outside = Screen::with_look(look_facts());
    let rejected = outside.step(ScreenAction::ScalesScrollRemembered { offset_pt: 50.0 });
    assert_eq!(
        rejected.outcome,
        ActionOutcome::Rejected(Rejection::NotInCalibrationMode)
    );
    assert_eq!(
        outside.state.calibration().scales_scroll_offset_pt(),
        0.0,
        "вне раздела не запоминается"
    );
}

#[test]
fn view_and_wheel_choices_are_rejected_outside_the_calibration_section() {
    let mut screen = Screen::with_look(look_facts());

    let view = screen.step(ScreenAction::ControlViewSelected(ControlViewKind::Wheels));
    let wheel = screen.step(ScreenAction::WheelSelected(WheelZone::Gamma));

    assert_eq!(view.outcome, ActionOutcome::Rejected(Rejection::NotInCalibrationMode));
    assert_eq!(wheel.outcome, ActionOutcome::Rejected(Rejection::NotInCalibrationMode));
    assert_eq!(
        screen.state.calibration().control_view(),
        ControlViewKind::Scales,
        "выбор не изменился"
    );
    assert_eq!(screen.state.calibration().selected_wheel(), WheelZone::Lift);
}

// ---------------------------------------------------------------- толчок

#[test]
fn dragging_the_push_sends_a_point_intent_for_that_wheel_only() {
    // Другие колёса уже правлены: намерение про Gamma их не упоминает.
    let mut screen = Screen::calibrating(facts_with_wheel(
        WheelZone::Gain,
        ColorWheel {
            push: push(0.2, 0.1),
            ring: knob(0.0),
        },
    ));

    let transition = screen.step(ScreenAction::WheelPushDragged {
        zone: WheelZone::Gamma,
        push: push(-0.3, 0.4),
    });

    assert_eq!(transition.outcome, ActionOutcome::Applied);
    assert_eq!(
        transition.effects,
        [Effect::Look(LookIntent::SetWheelPush {
            zone: WheelZone::Gamma,
            push: push(-0.3, 0.4)
        })]
    );
}

#[test]
fn the_same_push_as_in_the_facts_is_a_no_change_without_effects() {
    let wheel = ColorWheel {
        push: push(0.25, -0.5),
        ring: knob(0.0),
    };
    let mut screen = Screen::calibrating(facts_with_wheel(WheelZone::Offset, wheel));

    let transition = screen.step(ScreenAction::WheelPushDragged {
        zone: WheelZone::Offset,
        push: push(0.25, -0.5),
    });

    assert_eq!(
        transition.outcome,
        ActionOutcome::NoChange(NoChangeReason::ValueUnchanged)
    );
    assert!(transition.effects.is_empty());
}

#[test]
fn a_push_of_one_wheel_does_not_count_as_unchanged_for_another_wheel() {
    let wheel = ColorWheel {
        push: push(0.25, -0.5),
        ring: knob(0.0),
    };
    let mut screen = Screen::calibrating(facts_with_wheel(WheelZone::Offset, wheel));

    let transition = screen.step(ScreenAction::WheelPushDragged {
        zone: WheelZone::Lift,
        push: push(0.25, -0.5),
    });

    assert_eq!(
        transition.outcome,
        ActionOutcome::Applied,
        "у Lift точка другая (нейтраль)"
    );
}

#[test]
fn two_wheel_drags_before_facts_update_send_two_independent_point_intents() {
    // Два пальца в одном кадре: толчок и кольцо одного колеса. Каждое намерение несёт только своё значение.
    let mut screen = Screen::calibrating(look_facts());

    let pushed = screen.step(ScreenAction::WheelPushDragged {
        zone: WheelZone::Lift,
        push: push(0.5, 0.0),
    });
    let ringed = screen.step(ScreenAction::WheelRingDragged {
        zone: WheelZone::Lift,
        ring: knob(0.4),
    });

    assert_eq!(
        pushed.effects,
        [Effect::Look(LookIntent::SetWheelPush {
            zone: WheelZone::Lift,
            push: push(0.5, 0.0)
        })]
    );
    assert_eq!(
        ringed.effects,
        [Effect::Look(LookIntent::SetWheelRing {
            zone: WheelZone::Lift,
            ring: knob(0.4)
        })]
    );
}

// ---------------------------------------------------------------- кольцо

#[test]
fn dragging_the_ring_sends_the_ring_intent_and_the_same_value_is_a_no_change() {
    let wheel = ColorWheel {
        push: WheelPush::NEUTRAL,
        ring: knob(0.3),
    };
    let mut screen = Screen::calibrating(facts_with_wheel(WheelZone::Gain, wheel));

    let moved = screen.step(ScreenAction::WheelRingDragged {
        zone: WheelZone::Gain,
        ring: knob(-0.2),
    });
    let same = screen.step(ScreenAction::WheelRingDragged {
        zone: WheelZone::Gain,
        ring: knob(0.3),
    });

    assert_eq!(moved.outcome, ActionOutcome::Applied);
    assert_eq!(
        moved.effects,
        [Effect::Look(LookIntent::SetWheelRing {
            zone: WheelZone::Gain,
            ring: knob(-0.2)
        })]
    );
    assert_eq!(same.outcome, ActionOutcome::NoChange(NoChangeReason::ValueUnchanged));
    assert!(same.effects.is_empty());
}

#[test]
fn wheel_drags_are_rejected_outside_calibration_and_without_facts() {
    let mut photo = Screen::with_look(look_facts());
    let push_in_photo = photo.step(ScreenAction::WheelPushDragged {
        zone: WheelZone::Lift,
        push: push(0.1, 0.0),
    });
    let ring_in_photo = photo.step(ScreenAction::WheelRingDragged {
        zone: WheelZone::Lift,
        ring: knob(0.1),
    });
    assert_eq!(
        push_in_photo.outcome,
        ActionOutcome::Rejected(Rejection::NotInCalibrationMode)
    );
    assert_eq!(
        ring_in_photo.outcome,
        ActionOutcome::Rejected(Rejection::NotInCalibrationMode)
    );
    assert!(push_in_photo.effects.is_empty() && ring_in_photo.effects.is_empty());

    // Раздел есть, а фактов о виде ещё нет.
    let mut bare = Screen::ready();
    bare.step(ScreenAction::ModeSelected(crate::modes::CaptureMode::Calibrate));
    let without_facts = bare.step(ScreenAction::WheelPushDragged {
        zone: WheelZone::Lift,
        push: push(0.1, 0.0),
    });
    assert_eq!(
        without_facts.outcome,
        ActionOutcome::Rejected(Rejection::LookFactsMissing)
    );
    assert!(without_facts.effects.is_empty());
}

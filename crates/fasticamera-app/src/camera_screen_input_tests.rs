//! Тесты канала входа экрана: порядок, переполнение со счётчиком, разрыв после потерянного касания,
//! отказ при уничтоженном получателе. Проверяется то, что увидит читатель, а не вызовы.

use camera_screen::ScreenAction;
use shell_contract::clock::MonotonicNanos;
use shell_contract::orientation::DeviceOrientation;
use shell_contract::touch::{PointerId, SurfacePoint, TouchPhase, TouchSample};

use super::*;

fn sample(phase: TouchPhase, time_ns: i64) -> ScreenTouch {
    ScreenTouch::Sample(TouchSample {
        pointer: PointerId::new(0),
        phase,
        position: SurfacePoint { x_px: 10.0, y_px: 20.0 },
        time: MonotonicNanos::from_nanos(time_ns),
    })
}

fn drain(receiver: &mut ScreenInputReceiver) -> Vec<ScreenInput> {
    std::iter::from_fn(|| receiver.try_next()).collect()
}

#[test]
fn messages_arrive_in_send_order_and_are_counted() {
    let (mut sender, mut receiver) = screen_input_channel(8);
    let orientation = ScreenAction::DeviceOrientationChanged(DeviceOrientation::LandscapeLeft);
    assert_eq!(sender.send_touch(sample(TouchPhase::Began, 1)), InputDelivery::Queued);
    assert_eq!(sender.send_action(orientation.clone()), InputDelivery::Queued);
    assert_eq!(sender.send_touch(sample(TouchPhase::Ended, 2)), InputDelivery::Queued);

    assert_eq!(
        drain(&mut receiver),
        vec![
            ScreenInput::Touch(sample(TouchPhase::Began, 1)),
            ScreenInput::Action(orientation),
            ScreenInput::Touch(sample(TouchPhase::Ended, 2)),
        ]
    );
    let stats = receiver.stats();
    assert_eq!((stats.queued_touches, stats.queued_actions, stats.received), (2, 1, 3));
    assert_eq!(stats.overflowed_touches + stats.overflowed_actions, 0);
}

#[test]
fn an_overflowed_touch_is_counted_and_followed_by_a_gap_before_any_later_sample() {
    let (mut sender, mut receiver) = screen_input_channel(2);
    assert_eq!(sender.send_touch(sample(TouchPhase::Began, 1)), InputDelivery::Queued);
    assert_eq!(sender.send_touch(sample(TouchPhase::Moved, 2)), InputDelivery::Queued);
    // Очередь полна: образец потерян, отправитель должен экрану разрыв.
    assert_eq!(
        sender.send_touch(sample(TouchPhase::Moved, 3)),
        InputDelivery::Overflowed
    );
    assert!(sender.owes_gap());
    // Читатель освободил одно место: разрыв встаёт в него, но образец за ним уже не влезает и тоже теряется.
    assert_eq!(
        receiver.try_next(),
        Some(ScreenInput::Touch(sample(TouchPhase::Began, 1)))
    );
    assert_eq!(
        sender.send_touch(sample(TouchPhase::Ended, 4)),
        InputDelivery::Overflowed
    );
    assert!(
        sender.owes_gap(),
        "образец после разрыва потерян: сброс жестов нужен снова"
    );
    assert_eq!(
        drain(&mut receiver),
        vec![
            ScreenInput::Touch(sample(TouchPhase::Moved, 2)),
            ScreenInput::Touch(ScreenTouch::Gap),
        ]
    );
    // Место есть: сначала долг (разрыв), потом новый образец. Разрыв никогда не отстаёт от дыры.
    assert_eq!(sender.send_touch(sample(TouchPhase::Began, 5)), InputDelivery::Queued);
    assert!(!sender.owes_gap());
    assert_eq!(
        drain(&mut receiver),
        vec![
            ScreenInput::Touch(ScreenTouch::Gap),
            ScreenInput::Touch(sample(TouchPhase::Began, 5)),
        ]
    );

    let stats = sender.stats();
    assert_eq!(stats.overflowed_touches, 2, "потеряны Moved(3) и Ended(4)");
    assert_eq!(stats.gaps_queued, 2);
}

#[test]
fn a_gap_that_does_not_fit_stays_owed_until_there_is_room() {
    let (mut sender, mut receiver) = screen_input_channel(2);
    assert_eq!(sender.send_touch(sample(TouchPhase::Began, 1)), InputDelivery::Queued);
    assert_eq!(sender.send_touch(sample(TouchPhase::Moved, 1)), InputDelivery::Queued);
    assert_eq!(sender.send_touch(ScreenTouch::Gap), InputDelivery::Overflowed);
    assert!(sender.owes_gap(), "потерян сам разрыв — долг остаётся");
    assert_eq!(
        sender.send_touch(sample(TouchPhase::Began, 2)),
        InputDelivery::Overflowed
    );
    assert_eq!(drain(&mut receiver).len(), 2);
    assert_eq!(sender.send_touch(sample(TouchPhase::Began, 3)), InputDelivery::Queued);
    assert_eq!(
        drain(&mut receiver),
        vec![
            ScreenInput::Touch(ScreenTouch::Gap),
            ScreenInput::Touch(sample(TouchPhase::Began, 3)),
        ],
        "разрыв стоит перед первым образцом после дыры"
    );
}

#[test]
fn overflowed_actions_are_counted_without_owing_a_gap() {
    let (sender, mut receiver) = screen_input_channel(1);
    let first = ScreenAction::DeviceOrientationChanged(DeviceOrientation::Portrait);
    let second = ScreenAction::DeviceOrientationChanged(DeviceOrientation::LandscapeRight);
    assert_eq!(sender.send_action(first.clone()), InputDelivery::Queued);
    assert_eq!(sender.send_action(second.clone()), InputDelivery::Overflowed);
    assert!(!sender.owes_gap(), "потеря действия жесты не ломает");
    assert_eq!(drain(&mut receiver), vec![ScreenInput::Action(first)]);
    assert_eq!(sender.stats().overflowed_actions, 1);
}

#[test]
fn injector_clone_feeds_the_same_queue_and_counters() {
    let (sender, mut receiver) = screen_input_channel(4);
    let injector = sender.injector();
    let action = ScreenAction::DeviceOrientationChanged(DeviceOrientation::PortraitUpsideDown);
    assert_eq!(injector.inject(action.clone()), InputDelivery::Queued);
    assert_eq!(drain(&mut receiver), vec![ScreenInput::Action(action)]);
    assert_eq!(sender.stats().queued_actions, 1);
}

#[test]
fn sending_to_a_dropped_receiver_reports_receiver_gone_and_counts_it() {
    let (mut sender, receiver) = screen_input_channel(4);
    drop(receiver);
    assert_eq!(
        sender.send_touch(sample(TouchPhase::Began, 1)),
        InputDelivery::ReceiverGone
    );
    assert_eq!(
        sender.send_action(ScreenAction::DeviceOrientationChanged(DeviceOrientation::Portrait)),
        InputDelivery::ReceiverGone
    );
    assert_eq!(sender.stats().receiver_gone, 2);
    assert!(!sender.owes_gap(), "получателя нет — разрыв доставлять некому");
}

#[test]
fn an_empty_queue_yields_nothing() {
    let (_sender, mut receiver) = screen_input_channel(2);
    assert_eq!(receiver.try_next(), None);
    assert_eq!(receiver.stats().received, 0);
}

#[test]
fn volume_key_phases_map_to_pressed_repeated_and_released_for_both_keys() {
    use shell_contract::hardware_key::{HardwareKey, HardwareKeyEvent, KeyPhase as ShellPhase, KeyRepeat};
    let at = MonotonicNanos::from_nanos(42);
    let event = |key, phase, repeat| HardwareKeyEvent {
        key,
        phase,
        repeat,
        time: at,
    };
    let cases = [
        (
            event(HardwareKey::VolumeUp, ShellPhase::Pressed, KeyRepeat::Initial),
            ScreenKeyPhase::Pressed,
        ),
        (
            event(HardwareKey::VolumeDown, ShellPhase::Pressed, KeyRepeat::Initial),
            ScreenKeyPhase::Pressed,
        ),
        (
            event(
                HardwareKey::VolumeUp,
                ShellPhase::Pressed,
                KeyRepeat::AutoRepeat { count: 3 },
            ),
            ScreenKeyPhase::Repeated,
        ),
        (
            event(HardwareKey::VolumeDown, ShellPhase::Released, KeyRepeat::Initial),
            ScreenKeyPhase::Released,
        ),
    ];
    for (input, expected) in cases {
        assert_eq!(
            volume_key_action(input),
            ScreenAction::VolumeKey { phase: expected, at },
            "{input:?}"
        );
    }
}

#[test]
fn an_owed_gap_is_flushed_by_a_tick_without_any_new_touch() {
    let (mut sender, mut receiver) = screen_input_channel(2);
    assert_eq!(sender.send_touch(sample(TouchPhase::Began, 1)), InputDelivery::Queued);
    assert_eq!(sender.send_touch(sample(TouchPhase::Moved, 2)), InputDelivery::Queued);
    // `Ended` потерян: очередь полна, остался долг разрыва.
    assert_eq!(
        sender.send_touch(sample(TouchPhase::Ended, 3)),
        InputDelivery::Overflowed
    );
    assert!(sender.owes_gap());
    // Читатель ещё не освободил место: тик долг погасить не может, но и не теряет его.
    assert_eq!(sender.flush_owed_gap(), GapFlush::QueueFull);
    assert!(sender.owes_gap());

    // Читатель забрал сообщения; новых касаний нет — разрыв обязан дойти по тику.
    assert_eq!(drain(&mut receiver).len(), 2);
    assert_eq!(sender.flush_owed_gap(), GapFlush::Queued);
    assert!(!sender.owes_gap());
    assert_eq!(drain(&mut receiver), vec![ScreenInput::Touch(ScreenTouch::Gap)]);
    assert_eq!(receiver.stats().gaps_queued, 1);
    // Долг погашен: повтор ничего не шлёт.
    assert_eq!(sender.flush_owed_gap(), GapFlush::NotOwed);
    assert!(drain(&mut receiver).is_empty());
}

#[test]
fn flushing_without_a_debt_changes_no_counters() {
    let (mut sender, receiver) = screen_input_channel(2);
    let before = receiver.stats();
    assert_eq!(sender.flush_owed_gap(), GapFlush::NotOwed);
    assert_eq!(receiver.stats(), before);
}

#[test]
fn flushing_a_gap_to_a_destroyed_receiver_reports_it_and_keeps_the_debt() {
    let (mut sender, mut receiver) = screen_input_channel(1);
    assert_eq!(sender.send_touch(sample(TouchPhase::Began, 1)), InputDelivery::Queued);
    assert_eq!(
        sender.send_touch(sample(TouchPhase::Ended, 2)),
        InputDelivery::Overflowed
    );
    drain(&mut receiver);
    drop(receiver);
    assert_eq!(sender.flush_owed_gap(), GapFlush::ReceiverGone);
}

#[test]
fn receiver_state_follows_the_receiver_lifetime_without_sending() {
    let (sender, receiver) = screen_input_channel(2);
    let injector = sender.injector();
    assert_eq!(sender.receiver_state(), ReceiverState::Alive);
    assert_eq!(injector.receiver_state(), ReceiverState::Alive);
    drop(receiver);
    assert_eq!(sender.receiver_state(), ReceiverState::Gone);
    assert_eq!(injector.receiver_state(), ReceiverState::Gone);
    // Проверка состояния ничего не отправляла и не считала.
    assert_eq!(sender.stats(), ScreenInputStats::default());
}

#[test]
fn system_gesture_zones_travel_as_their_own_message_and_count_with_actions() {
    use shell_contract::system_gesture::{RawEdgeInsetsPx, SystemGestureZones};
    let zones = SystemGestureZones::try_from_insets(RawEdgeInsetsPx {
        left: 0,
        top: 156,
        right: 0,
        bottom: 132,
    })
    .expect("зоны");
    let (sender, mut receiver) = screen_input_channel(4);
    assert_eq!(sender.send_system_gesture_zones(zones), InputDelivery::Queued);
    assert_eq!(drain(&mut receiver), vec![ScreenInput::SystemGestureZones(zones)]);
    let stats = sender.stats();
    assert_eq!((stats.queued_actions, stats.queued_touches), (1, 0));
}

#[test]
fn system_gesture_zones_overflow_is_counted_without_owing_a_gap_and_gone_receiver_is_reported() {
    use shell_contract::system_gesture::SystemGestureZones;
    let (sender, receiver) = screen_input_channel(1);
    assert_eq!(
        sender.send_system_gesture_zones(SystemGestureZones::NONE),
        InputDelivery::Queued
    );
    assert_eq!(
        sender.send_system_gesture_zones(SystemGestureZones::NONE),
        InputDelivery::Overflowed
    );
    assert!(!sender.owes_gap(), "потеря зон жесты не ломает");
    assert_eq!(sender.stats().overflowed_actions, 1);
    drop(receiver);
    assert_eq!(
        sender.send_system_gesture_zones(SystemGestureZones::NONE),
        InputDelivery::ReceiverGone
    );
    assert_eq!(sender.stats().receiver_gone, 1);
}

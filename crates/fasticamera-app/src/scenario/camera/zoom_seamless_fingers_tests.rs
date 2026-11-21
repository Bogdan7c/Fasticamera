//! Пальцы бесшовных шагов: что именно уходит в экран (число пальцев, фазы, направление, метки времени).

use super::*;

const STRIP: ScreenPoint = ScreenPoint::new(0.5, 0.75);

fn plan() -> SeamlessPlan {
    SeamlessPlan::default()
}

fn at(nanos: i64) -> MonotonicNanos {
    MonotonicNanos::from_nanos(nanos)
}

#[test]
fn a_wheel_is_one_finger_on_the_strip_and_a_pinch_is_two_fingers_around_the_center() {
    let mut wheel = Fingers::new(SeamlessGesture::Wheel, &plan(), STRIP);
    let began = wheel.press(at(1_000));
    assert_eq!(began.len(), 1);
    assert_eq!(began[0].phase, TouchPhase::Began);
    assert_eq!(began[0].point, STRIP);

    let mut pinch = Fingers::new(SeamlessGesture::Pinch, &plan(), STRIP);
    let began = pinch.press(at(1_000));
    assert_eq!(
        began.iter().map(|touch| touch.pointer).collect::<Vec<_>>(),
        [PointerId::new(0), PointerId::new(1)]
    );
    assert!(began[0].point.x < plan().pinch_center.x && began[1].point.x > plan().pinch_center.x);
}

#[test]
fn nudging_moves_the_wheel_left_to_zoom_in_and_right_to_zoom_out() {
    let mut fingers = Fingers::new(SeamlessGesture::Wheel, &plan(), STRIP);
    fingers.press(at(1));
    let zoom_in = fingers.nudge(ZoomDirection::In, &plan(), at(2));
    assert!(zoom_in[0].point.x < STRIP.x);
    let back = fingers.nudge(ZoomDirection::Out, &plan(), at(3));
    let again = fingers.nudge(ZoomDirection::Out, &plan(), at(4));
    assert!(back[0].point.x > zoom_in[0].point.x && again[0].point.x > back[0].point.x);
    assert_eq!(again[0].phase, TouchPhase::Moved);
}

#[test]
fn nudging_spreads_the_pinch_to_zoom_in_and_closes_it_to_zoom_out() {
    let mut fingers = Fingers::new(SeamlessGesture::Pinch, &plan(), STRIP);
    fingers.press(at(1));
    let spread = |touches: &[TouchInjection]| touches[1].point.x - touches[0].point.x;
    let wider = fingers.nudge(ZoomDirection::In, &plan(), at(2));
    let narrower = fingers.nudge(ZoomDirection::Out, &plan(), at(3));
    let narrowest = fingers.nudge(ZoomDirection::Out, &plan(), at(4));
    assert!(spread(&wider) > 2.0 * plan().pinch_start_half_width);
    assert!(spread(&narrowest) < spread(&narrower));
}

#[test]
fn sample_times_strictly_increase_even_when_the_render_clock_stalls() {
    let mut fingers = Fingers::new(SeamlessGesture::Wheel, &plan(), STRIP);
    let first = fingers.press(at(5_000))[0].time;
    let second = fingers.hold(at(5_000))[0].time;
    let third = fingers.nudge(ZoomDirection::In, &plan(), at(4_000))[0].time;
    assert!(first < second && second < third, "{first:?} {second:?} {third:?}");
}

#[test]
fn the_wheel_stops_at_the_screen_edge_and_reports_the_limit() {
    let mut fingers = Fingers::new(SeamlessGesture::Wheel, &plan(), STRIP);
    fingers.press(at(1));
    assert!(!fingers.is_at_limit(ZoomDirection::Out));
    for index in 0..1_000 {
        fingers.nudge(ZoomDirection::Out, &plan(), at(2 + index));
    }
    assert!(fingers.is_at_limit(ZoomDirection::Out));
    let lifted = fingers.lift(at(5_000));
    assert_eq!(lifted[0].phase, TouchPhase::Ended);
    assert!(lifted[0].point.x <= 1.0);
}

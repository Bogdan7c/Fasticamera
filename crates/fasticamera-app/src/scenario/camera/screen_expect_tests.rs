//! Правила ожиданий шага `screen`: когда шаг достигнут, когда перерос, какой текст у нарушения.

use camera_contract::geometry::SensorOrientation;
use camera_contract::three_a::{AeAfLock, MeteringRegion, PointOfInterest, PointRequestId, SensorPoint};

use super::super::screen_sensor_point::FramePoint;
use super::*;
use crate::camera_screen_source::{
    ModeKind, PanelKind, Presence, ScreenStateSnapshot, ShutterActivity, ShutterPlacement,
};

fn delta() -> ProbeDelta {
    ProbeDelta {
        taps: 0,
        long_presses: 0,
        pans: 0,
        button_presses: 0,
        camera_intents: 0,
        camera_intents_lost: 0,
        focus_triggers: 0,
        focus_trigger_failures: 0,
        results_with_unknown_origin: 0,
        capture_requests: 0,
        lock: AeAfLock::Unlocked,
        point_of_interest: PointOfInterest::WholeFrame,
        sensor_orientation: Some(SensorOrientation::Degrees90),
        exposure_bias_ev: 0.0,
        thumbnail_has_last: false,
        screen: ScreenStateSnapshot {
            panel: PanelKind::Closed,
            lock_plaque: Presence::Hidden,
            focus_indicator: Presence::Hidden,
            exposure_steps: 0,
            mode: ModeKind::Photo,
            shutter_placement: ShutterPlacement::Visible,
            shutter_activity: ShutterActivity::Idle,
            zoom: crate::camera_screen_source::ZoomView {
                ratio_milli: 1_000,
                facing: camera_contract::ids::LensFacing::Back,
                wheel_open: false,
                gliding: false,
            },
            safe_area: shell_contract::safe_area::SafeAreaInsets::NONE,
            white: crate::camera_screen_white_view::WhiteView::starting(),
        },
        white: crate::scenario::camera::screen_white_check::WhiteDelta::quiet(),
    }
}

fn tap_that_reaches_the_hal() -> Expectation {
    Expectation::new()
        .count(Metric::Taps, Count::Exactly(1))
        .count(Metric::CameraIntents, Count::Exactly(1))
        .count(Metric::FocusTriggers, Count::Exactly(1))
}

#[test]
fn a_tap_is_reached_only_when_the_trigger_got_to_the_hal() {
    let expectation = tap_that_reaches_the_hal();
    // Экран распознал тап и отправил намерение, но триггер до HAL не дошёл: шаг не достигнут.
    let lost = ProbeDelta {
        taps: 1,
        camera_intents: 1,
        ..delta()
    };
    assert!(!expectation.is_reached(&lost));
    let violations = expectation.violations(&lost);
    assert_eq!(violations, ["триггеры до HAL: ожидали ровно 1, увидели 0"]);
    let delivered = ProbeDelta {
        focus_triggers: 1,
        ..lost
    };
    assert!(expectation.is_reached(&delivered));
    assert!(expectation.violations(&delivered).is_empty());
}

#[test]
fn an_extra_camera_intent_overshoots_an_exact_count() {
    let expectation = tap_that_reaches_the_hal();
    let double = ProbeDelta {
        taps: 1,
        camera_intents: 2,
        focus_triggers: 1,
        ..delta()
    };
    assert!(expectation.is_overshot(&double));
    assert_eq!(
        expectation.violations(&double),
        ["намерения камере: ожидали ровно 1, увидели 2"]
    );
    let exact = ProbeDelta {
        camera_intents: 1,
        ..double
    };
    assert!(!expectation.is_overshot(&exact));
}

#[test]
fn exactly_zero_catches_a_tap_that_should_only_have_closed_the_panel() {
    let probe_while_panel_open = Expectation::new()
        .count(Metric::Taps, Count::Exactly(1))
        .count(Metric::CameraIntents, Count::Exactly(0));
    let closed_only = ProbeDelta { taps: 1, ..delta() };
    assert!(probe_while_panel_open.is_reached(&closed_only));
    assert!(probe_while_panel_open.violations(&closed_only).is_empty());
    // Панель была уже закрыта: тап навёл фокус — намерение ушло, и это нарушение.
    let focused = ProbeDelta {
        taps: 1,
        camera_intents: 1,
        ..delta()
    };
    assert!(probe_while_panel_open.is_overshot(&focused));
    assert!(!probe_while_panel_open.violations(&focused).is_empty());
}

#[test]
fn at_least_accepts_a_drag_that_sent_several_values() {
    let drag = Expectation::new()
        .count(Metric::Pans, Count::Exactly(1))
        .count(Metric::CameraIntents, Count::AtLeast(1))
        .state(StateCheck::ExposureBiasIsSet);
    let few = ProbeDelta {
        pans: 1,
        camera_intents: 4,
        exposure_bias_ev: 1.0,
        ..delta()
    };
    assert!(drag.is_reached(&few) && !drag.is_overshot(&few));
    assert!(drag.violations(&few).is_empty());
    // Значения ушли, но движок остался на нуле: нарушение называет состояние.
    let ignored = ProbeDelta {
        exposure_bias_ev: 0.0,
        ..few
    };
    assert!(!drag.is_reached(&ignored));
    assert_eq!(drag.violations(&ignored), ["ожидали EV не равен 0, увидели 0"]);
}

#[test]
fn state_checks_name_the_value_that_was_seen() {
    let locked = Expectation::new().state(StateCheck::Lock(AeAfLock::Locked));
    assert!(!locked.is_reached(&delta()));
    assert_eq!(
        locked.violations(&delta()),
        ["ожидали блокировка Locked, увидели Unlocked"]
    );
    let reset = Expectation::new().state(StateCheck::ExposureBiasIsZero);
    let moved = ProbeDelta {
        exposure_bias_ev: 1.0,
        ..delta()
    };
    assert_eq!(reset.violations(&moved), ["ожидали EV = 0, увидели +1.00"]);
    let thumbnail = Expectation::new().state(StateCheck::ThumbnailShowsLast);
    assert!(!thumbnail.is_reached(&delta()));
    assert!(thumbnail.is_reached(&ProbeDelta {
        thumbnail_has_last: true,
        ..delta()
    }));
}

fn point_at(x: f32, y: f32) -> PointOfInterest {
    PointOfInterest::Point {
        region: MeteringRegion::new(SensorPoint::new(x, y).expect("точка"), 0.1).expect("регион"),
        request: PointRequestId(1),
    }
}

/// Шаг тапа в четверть кадра по ширине и три четверти по высоте: сенсор 90° ждёт точку (0,75; 0,75).
fn quarter_tap_expectation() -> Expectation {
    tap_that_reaches_the_hal().state(StateCheck::SensorPoint {
        frame: FramePoint { u: 0.25, v: 0.75 },
        tolerance: 0.05,
    })
}

#[test]
fn the_sensor_point_decides_the_step_not_just_the_counters() {
    let expectation = quarter_tap_expectation();
    let reached_counters = ProbeDelta {
        taps: 1,
        camera_intents: 1,
        focus_triggers: 1,
        ..delta()
    };
    // Счётчики идеальны, но точки интереса у движка нет: шаг не достигнут.
    assert!(!expectation.is_reached(&reached_counters));
    assert_eq!(expectation.violations(&reached_counters).len(), 1);

    let right = ProbeDelta {
        point_of_interest: point_at(0.75, 0.75),
        ..reached_counters
    };
    assert!(expectation.is_reached(&right));
    assert!(expectation.violations(&right).is_empty());

    // Перепутанные оси (x ↔ y) и зеркало по горизонтали при зелёных счётчиках дают красный шаг с текстом про точку.
    for (name, wrong) in [("оси", point_at(0.75, 0.25)), ("зеркало", point_at(0.25, 0.75))] {
        let swapped = ProbeDelta {
            point_of_interest: wrong,
            ..reached_counters
        };
        assert!(!expectation.is_reached(&swapped), "{name}");
        let violations = expectation.violations(&swapped);
        assert_eq!(violations.len(), 1, "{name}: {violations:?}");
        assert!(
            violations[0].contains("three_a.point") && violations[0].contains("(0.75; 0.75)"),
            "{name}: текст про точку с ожидаемым значением: {violations:?}"
        );
    }
}

#[test]
fn a_point_within_the_tolerance_passes_and_unknown_orientation_never_passes_silently() {
    let expectation = quarter_tap_expectation();
    let near = ProbeDelta {
        taps: 1,
        camera_intents: 1,
        focus_triggers: 1,
        point_of_interest: point_at(0.78, 0.72),
        ..delta()
    };
    assert!(expectation.is_reached(&near), "в допуске 0,05");
    let far = ProbeDelta {
        point_of_interest: point_at(0.82, 0.75),
        ..near
    };
    assert!(!expectation.is_reached(&far), "за допуском");

    let blind = ProbeDelta {
        sensor_orientation: None,
        ..near
    };
    assert!(
        !expectation.is_reached(&blind),
        "без ориентации сенсора судить нечем — не зелёный"
    );
    assert!(expectation.violations(&blind)[0].contains("ориентация сенсора неизвестна"));
}

//! Тесты времени, датчиков и размера surface: затухание индикатора, ориентация, уровень, раскладка.

use shell_contract::gravity::GravityVector;
use shell_contract::orientation::DeviceOrientation;
use shell_contract::surface::SurfaceSize;

use crate::action::ScreenAction;
use crate::effect::Effect;
use crate::focus::FocusIndicator;
use crate::level::LevelReading;
use crate::outcome::{ActionOutcome, Rejection};
use crate::settings::AspectRatio;
use crate::test_support::{OUR_SURFACE, Screen, at_ms};

#[test]
fn focus_indicator_fades_after_the_configured_time_and_not_before() {
    let mut screen = Screen::ready();
    screen.tap_center(at_ms(1_000));

    screen.step(ScreenAction::Tick(at_ms(3_999)));
    assert!(matches!(screen.state.focus().indicator(), FocusIndicator::Shown { .. }));

    screen.step(ScreenAction::Tick(at_ms(4_000)));
    assert_eq!(screen.state.focus().indicator(), FocusIndicator::Hidden);
}

#[test]
fn fade_time_comes_from_the_config() {
    let mut config = crate::ScreenConfig::default();
    config.focus.indicator_visible_ms = 500;
    let mut screen = Screen::unprepared(config);
    screen.step(ScreenAction::SurfaceResized(OUR_SURFACE));
    screen.step(ScreenAction::CameraFactsChanged(crate::test_support::live_facts()));
    screen.tap_center(at_ms(0));
    screen.step(ScreenAction::Tick(at_ms(500)));
    assert_eq!(screen.state.focus().indicator(), FocusIndicator::Hidden);
}

#[test]
fn indicator_does_not_fade_while_the_lock_is_pending_or_active() {
    let mut screen = Screen::ready();
    let point = screen.current_frame_center();
    screen.step(ScreenAction::ViewfinderLongPressed { point, at: at_ms(0) });
    screen.step(ScreenAction::Tick(at_ms(1_000)));
    assert!(
        matches!(screen.state.focus().indicator(), FocusIndicator::Shown { .. }),
        "ждём подтверждения"
    );
}

#[test]
fn orientation_turns_icons_but_keeps_the_layout() {
    let mut screen = Screen::ready();
    let layout_before = screen.state.layout().copied();
    let framing_before = screen.state.framing_intent();

    for (orientation, degrees) in [
        (DeviceOrientation::LandscapeLeft, 90.0),
        (DeviceOrientation::LandscapeRight, -90.0),
        (DeviceOrientation::PortraitUpsideDown, 180.0),
        (DeviceOrientation::Portrait, 0.0),
    ] {
        let transition = screen.step(ScreenAction::DeviceOrientationChanged(orientation));
        assert!(
            transition.effects.is_empty(),
            "поворот значков не тревожит ни камеру, ни рендер"
        );
        assert_eq!(screen.state.icon_rotation_clockwise_degrees(), degrees);
        assert_eq!(screen.state.layout().copied(), layout_before);
        assert_eq!(screen.state.framing_intent(), framing_before);
    }
}

#[test]
fn gravity_updates_the_level_reading_and_flat_phone_hides_it() {
    let mut screen = Screen::ready();
    assert_eq!(screen.state.level(), LevelReading::Unknown);

    let tilt = 12.0_f32.to_radians();
    screen.step(ScreenAction::GravityChanged(GravityVector {
        x: -9.81 * tilt.sin(),
        y: 9.81 * tilt.cos(),
        z: 0.0,
    }));
    let LevelReading::Tilted {
        roll_clockwise_degrees,
        deviation_degrees,
    } = screen.state.level()
    else {
        panic!("телефон стоит, уровень должен быть");
    };
    assert!((roll_clockwise_degrees - 12.0).abs() < 1e-3 && (deviation_degrees - 12.0).abs() < 1e-3);

    screen.step(ScreenAction::GravityChanged(GravityVector {
        x: 0.0,
        y: 0.0,
        z: 9.81,
    }));
    assert_eq!(screen.state.level(), LevelReading::Flat);
}

#[test]
fn surface_resize_recomputes_the_frame_position_and_tells_the_renderer() {
    let mut screen = Screen::unprepared(crate::ScreenConfig::default());
    let first = screen.step(ScreenAction::SurfaceResized(OUR_SURFACE));
    let layout = screen.state.layout().copied().expect("раскладка");
    assert_eq!(
        first.effects,
        [Effect::Viewfinder(crate::test_support::plain_framing(
            &layout,
            AspectRatio::FourByThree
        ))]
    );

    // Экран стал выше (другая высота surface) — кадр прижат к низу, значит, поехал вниз.
    let taller = SurfaceSize {
        width_px: 1080,
        height_px: 2500,
    };
    let second = screen.step(ScreenAction::SurfaceResized(taller));
    let new_layout = screen.state.layout().copied().expect("раскладка");
    let [Effect::Viewfinder(intent)] = second.effects.as_slice() else {
        panic!("ожидалась команда рендеру, получено {:?}", second.effects);
    };
    assert_eq!(
        *intent,
        crate::test_support::plain_framing(&new_layout, AspectRatio::FourByThree)
    );
    assert!(
        new_layout.frame_rect(AspectRatio::FourByThree).min_y() > layout.frame_rect(AspectRatio::FourByThree).min_y()
    );
}

#[test]
fn surface_resize_re_uses_the_current_aspect() {
    let mut screen = Screen::ready();
    screen.step(ScreenAction::ControlsMenuOpenRequested);
    screen.step(ScreenAction::ControlsItemSelected(crate::tools::ToolId::Aspect));
    screen.step(ScreenAction::SubmenuOptionSelected(crate::tools::ToolOption::Aspect(
        AspectRatio::Square,
    )));
    let resized = screen.step(ScreenAction::SurfaceResized(SurfaceSize {
        width_px: 1080,
        height_px: 2400,
    }));
    let [Effect::Viewfinder(intent)] = resized.effects.as_slice() else {
        panic!("нужна команда рендеру")
    };
    assert_eq!(intent.zoom.aspect, camera_contract::viewfinder_crop::CropAspect::SQUARE);
}

#[test]
fn invalid_surface_drops_the_stale_layout_so_taps_are_refused_until_a_valid_size_arrives() {
    let mut screen = Screen::ready();
    let tap = screen.current_frame_center();
    let broken = screen.step(ScreenAction::SurfaceResized(SurfaceSize {
        width_px: 0,
        height_px: 0,
    }));
    assert_eq!(broken.outcome, ActionOutcome::Rejected(Rejection::InvalidSurface));
    assert!(broken.effects.is_empty());
    assert!(
        screen.state.layout().is_none(),
        "старая раскладка описывает чужую поверхность"
    );
    assert_eq!(screen.state.surface_size(), None);
    assert_eq!(screen.state.rejections().get(Rejection::InvalidSurface), 1);

    // Тап по месту старого кадра: маски больше нет, отказ явный, фокус не ставится.
    let refused = screen.step(ScreenAction::ViewfinderTapped {
        point: tap,
        at: at_ms(10),
    });
    assert_eq!(refused.outcome, ActionOutcome::Rejected(Rejection::InvalidSurface));
    assert!(refused.effects.is_empty());
    assert_eq!(screen.state.focus().indicator(), FocusIndicator::Hidden);

    // Валидный размер возвращает раскладку и тапы.
    let restored = screen.step(ScreenAction::SurfaceResized(OUR_SURFACE));
    assert_eq!(restored.outcome, ActionOutcome::Applied);
    assert!(screen.state.layout().is_some());
    let accepted = screen.tap_center(at_ms(20));
    assert_eq!(accepted.outcome, ActionOutcome::Applied);
}

#[test]
fn tick_that_changes_nothing_is_no_change_but_still_advances_the_clock() {
    let mut screen = Screen::ready();
    let idle_tick = screen.step(ScreenAction::Tick(at_ms(100)));
    assert_eq!(
        idle_tick.outcome,
        ActionOutcome::NoChange(crate::NoChangeReason::ValueUnchanged)
    );
    assert!(idle_tick.effects.is_empty());
    // Время продвинулось: индикатор, созданный позже по факту камеры, получает свежую метку.
    screen.step(ScreenAction::CameraFactsChanged(crate::test_support::facts_with_point(
        camera_contract::three_a::SensorPoint::new(0.5, 0.5).expect("центр"),
        1,
    )));
    let FocusIndicator::Shown { since, .. } = screen.state.focus().indicator() else {
        panic!("квадрат камеры должен появиться");
    };
    assert_eq!(since, at_ms(100));
}

#[test]
fn tick_that_fades_the_indicator_is_applied() {
    let mut screen = Screen::ready();
    screen.tap_center(at_ms(0));
    let fade = screen.step(ScreenAction::Tick(at_ms(3_000)));
    assert_eq!(fade.outcome, ActionOutcome::Applied);
}

#[test]
fn a_rejected_action_still_advances_the_clock_but_changes_nothing_else() {
    let mut screen = Screen::ready();
    screen.step(ScreenAction::ControlsMenuOpenRequested);
    let before = screen.state.clone();
    let refused = screen.step(ScreenAction::ShutterTapped { at: at_ms(5_000) });
    assert_eq!(refused.outcome, ActionOutcome::Rejected(Rejection::ShutterHiddenByMenu));
    // Всё, кроме счётчика отказов и времени, прежнее.
    let mut expected = before;
    expected.rejections.record(Rejection::ShutterHiddenByMenu);
    expected.last_time = at_ms(5_000);
    assert_eq!(screen.state, expected);

    // Следствие: квадрат камеры, появившийся позже, несёт время отказанного действия, а не 0.
    screen.step(ScreenAction::ControlsMenuCloseRequested);
    screen.step(ScreenAction::CameraFactsChanged(crate::test_support::facts_with_point(
        camera_contract::three_a::SensorPoint::new(0.5, 0.5).expect("центр"),
        1,
    )));
    let FocusIndicator::Shown { since, .. } = screen.state.focus().indicator() else {
        panic!("квадрат камеры должен появиться");
    };
    assert_eq!(since, at_ms(5_000));
}

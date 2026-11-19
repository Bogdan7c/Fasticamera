#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Reduce Motion — одно правило: `Spring::for_motion`. Им пользуются аниматор свойств и движок
//! прокрутки; здесь проверяется само правило (по физике пружины, а не по тому же коду) и то, что
//! движок прокрутки реально едет без перелёта и умеет менять режим на лету.

use ui_core::geometry::Vector;
use ui_core::scroll::{AxisGeometry, ScrollAxis, ScrollConfig, ScrollTransition};
use ui_core::spring::{RestCriterion, Spring, SpringAnimation, SpringSpec, SpringStart};
use ui_core::time::{MonotonicNanos, MotionPreference};
use ui_core::velocity::VelocityEstimate;

fn ms(millis: i64) -> MonotonicNanos {
    MonotonicNanos::from_nanos(1_000_000_000 + millis * 1_000_000)
}

/// Наибольшее значение пружины, едущей от 0 к 1 с нулевой скоростью (перелёт, если > 1).
fn peak_of_unit_step(spring: Spring) -> f64 {
    let animation = SpringAnimation::start(SpringStart {
        spring,
        from: 0.0,
        to: 1.0,
        initial_velocity: 0.0,
        start_time: ms(0),
        rest: RestCriterion::default(),
    })
    .unwrap();
    (0..=3_000)
        .map(|step| animation.sample(ms(step)).value)
        .fold(f64::MIN, f64::max)
}

#[test]
fn reduced_motion_removes_overshoot_but_keeps_duration() {
    for bouncy in [
        Spring::snappy(),
        Spring::bouncy(),
        Spring::from_duration_bounce(0.85, 0.4).unwrap(),
    ] {
        let before = bouncy.perceptual().unwrap();
        let reduced = bouncy.for_motion(MotionPreference::Reduced);
        let after = reduced.perceptual().unwrap();
        assert!(before.bounce > 0.0);
        assert!(after.bounce.abs() < 1e-12, "bounce = {}", after.bounce);
        assert!(
            (after.duration_s - before.duration_s).abs() < 1e-12,
            "длительность та же"
        );
        // Контроль независимо от параметров: траектория действительно не перелетает, а исходная — да.
        assert!(peak_of_unit_step(bouncy) > 1.001);
        assert!(peak_of_unit_step(reduced) <= 1.0 + 1e-9);
    }
}

#[test]
fn reduced_motion_leaves_non_bouncing_springs_and_full_motion_untouched() {
    let camera_snap = Spring::from_duration_bounce(0.85, -0.13).unwrap();
    let smooth = Spring::smooth();
    for spring in [camera_snap, smooth, Spring::immediate(), Spring::bouncy()] {
        assert_eq!(
            spring.for_motion(MotionPreference::Full),
            spring,
            "Full ничего не меняет"
        );
    }
    for spring in [camera_snap, smooth, Spring::immediate()] {
        assert_eq!(
            spring.for_motion(MotionPreference::Reduced),
            spring,
            "без перелёта — без изменений"
        );
    }
}

fn scroll_config_with_bouncy_snap() -> ScrollConfig {
    ScrollConfig {
        snap_spring: SpringSpec::DurationBounce {
            duration_s: 0.5,
            bounce: 0.3,
        },
        ..ScrollConfig::default()
    }
}

fn geometry() -> AxisGeometry {
    AxisGeometry {
        min_offset: 0.0,
        max_offset: 1000.0,
        viewport_extent: 960.0,
    }
}

fn peak_offset_of_scroll_to(axis: &mut ScrollAxis) -> f64 {
    axis.scroll_to(100.0, ScrollTransition::Animated, ms(0)).unwrap();
    (0..=3_000)
        .map(|step| axis.sample(ms(step)).offset)
        .fold(f64::MIN, f64::max)
}

#[test]
fn scroll_axis_follows_the_motion_preference() {
    let config = scroll_config_with_bouncy_snap();
    let mut full = ScrollAxis::with_motion(&config, MotionPreference::Full, geometry(), 0.0).unwrap();
    let mut reduced = ScrollAxis::with_motion(&config, MotionPreference::Reduced, geometry(), 0.0).unwrap();
    assert_eq!(reduced.motion_preference(), MotionPreference::Reduced);
    assert!(
        peak_offset_of_scroll_to(&mut full) > 101.0,
        "контроль: полная пружина перелетает"
    );
    assert!(peak_offset_of_scroll_to(&mut reduced) <= 100.0 + 1e-6);
}

#[test]
fn motion_preference_switches_on_a_live_axis_without_moving_it() {
    let config = scroll_config_with_bouncy_snap();
    let mut axis = ScrollAxis::new(&config, geometry(), 40.0).unwrap();
    let before = axis.sample(ms(0));
    axis.set_motion_preference(MotionPreference::Reduced);
    assert_eq!(axis.motion_preference(), MotionPreference::Reduced);
    assert_eq!(axis.sample(ms(0)), before, "смена режима не двигает контент");
    assert!(
        peak_offset_of_scroll_to(&mut axis) <= 100.0 + 1e-6,
        "новое движение уже без перелёта"
    );
    // И обратно: следующее движение снова с перелётом.
    axis.set_motion_preference(MotionPreference::Full);
    axis.scroll_to(0.0, ScrollTransition::Immediate, ms(5_000)).unwrap();
    axis.scroll_to(100.0, ScrollTransition::Animated, ms(5_000)).unwrap();
    let peak = (0..=3_000)
        .map(|step| axis.sample(ms(5_000 + step)).offset)
        .fold(f64::MIN, f64::max);
    assert!(peak > 101.0);
}

#[test]
fn vector_velocity_estimate_has_the_same_or_zero_rule_as_scalar() {
    assert_eq!(
        VelocityEstimate::Measured(Vector::new(120.0, -30.0)).or_zero(),
        Vector::new(120.0, -30.0)
    );
    assert_eq!(
        VelocityEstimate::Measured(Vector::new(f64::NAN, 5.0)).or_zero(),
        Vector::new(0.0, 5.0),
        "нечисловая компонента даёт 0, остальные сохраняются"
    );
    assert_eq!(
        VelocityEstimate::Measured(Vector::new(1.0, f64::INFINITY)).or_zero(),
        Vector::new(1.0, 0.0)
    );
    assert_eq!(VelocityEstimate::<Vector>::Stopped.or_zero(), Vector::ZERO);
    assert_eq!(VelocityEstimate::<Vector>::NotEnoughSamples.or_zero(), Vector::ZERO);
}

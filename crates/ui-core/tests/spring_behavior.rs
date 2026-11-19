#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Поведенческие тесты пружин. Эталоны независимы от проверяемого кода: контрольные числа
//! документации Apple, численное интегрирование ОДУ (RK4) и отдельная формула критического случая.

use ui_core::spring::{
    AdditiveSpringAnimation, AdditiveSpringStart, RestCriterion, Spring, SpringAnimation, SpringError, SpringPhase,
    SpringSpec, SpringStart, SpringState, distance_fraction_per_second, velocity_from_distance_fraction,
};
use ui_core::time::MonotonicNanos;

fn at_seconds(seconds: f64) -> MonotonicNanos {
    MonotonicNanos::from_nanos((seconds * 1e9).round() as i64)
}

/// Численное интегрирование `m·x'' = −k·x − c·x'` методом Рунге — Кутты 4-го порядка.
fn rk4(mass: f64, stiffness: f64, damping: f64, x0: f64, v0: f64, until_s: f64) -> (f64, f64) {
    let step = 1e-5;
    let steps = (until_s / step).round() as usize;
    let accel = |x: f64, v: f64| (-stiffness * x - damping * v) / mass;
    let (mut x, mut v) = (x0, v0);
    for _ in 0..steps {
        let (k1x, k1v) = (v, accel(x, v));
        let (k2x, k2v) = (v + 0.5 * step * k1v, accel(x + 0.5 * step * k1x, v + 0.5 * step * k1v));
        let (k3x, k3v) = (v + 0.5 * step * k2v, accel(x + 0.5 * step * k2x, v + 0.5 * step * k2v));
        let (k4x, k4v) = (v + step * k3v, accel(x + step * k3x, v + step * k3v));
        x += step / 6.0 * (k1x + 2.0 * k2x + 2.0 * k3x + k4x);
        v += step / 6.0 * (k1v + 2.0 * k2v + 2.0 * k3v + k4v);
    }
    (x, v)
}

fn physical(spring: &Spring) -> (f64, f64, f64) {
    let p = spring.physical().expect("физическая пружина");
    (p.mass, p.stiffness, p.damping)
}

fn start_animation(spring: Spring, from: f64, to: f64, velocity: f64) -> SpringAnimation {
    SpringAnimation::start(SpringStart {
        spring,
        from,
        to,
        initial_velocity: velocity,
        start_time: at_seconds(1.0),
        rest: RestCriterion::default(),
    })
    .expect("анимация стартует")
}

#[test]
fn apple_documented_checkpoints_convert_both_ways() {
    let bouncy = Spring::from_duration_bounce(0.5, 0.3).unwrap();
    let (mass, stiffness, damping) = physical(&bouncy);
    assert_eq!(mass, 1.0);
    assert!((stiffness - 157.9).abs() < 0.05, "k = {stiffness}");
    assert!((damping - 17.6).abs() < 0.05, "c = {damping}");

    let reverse = Spring::from_physical(1.0, 100.0, 10.0).unwrap().perceptual().unwrap();
    assert!(
        (reverse.duration_s - 0.63).abs() < 0.005,
        "duration = {}",
        reverse.duration_s
    );
    assert!((reverse.bounce - 0.5).abs() < 1e-12, "bounce = {}", reverse.bounce);
}

#[test]
fn preset_table_matches_documentation() {
    // Таблица motion.md 1.5: (пружина, k, c).
    let table = [
        (Spring::smooth(), 157.91, 25.13),
        (Spring::snappy(), 157.91, 21.36),
        (Spring::bouncy(), 157.91, 17.59),
    ];
    for (spring, k, c) in table {
        let (_, stiffness, damping) = physical(&spring);
        assert!((stiffness - k).abs() < 0.01, "k {stiffness} != {k}");
        assert!((damping - c).abs() < 0.01, "c {damping} != {c}");
    }
    // Animation.default: response 0,55, ζ = 1 -> критическое затухание.
    let default = Spring::animation_default();
    assert!((default.damping_ratio().unwrap() - 1.0).abs() < 1e-12);
    assert!((default.perceptual().unwrap().duration_s - 0.55).abs() < 1e-12);
    let interactive = Spring::interactive();
    assert!((interactive.damping_ratio().unwrap() - 0.86).abs() < 1e-12);
    assert!((interactive.perceptual().unwrap().duration_s - 0.15).abs() < 1e-12);
}

#[test]
fn duration_bounce_round_trips_including_overdamped() {
    for bounce in [-0.9, -0.5, -0.13, 0.0, 0.15, 0.3, 0.8, 1.0] {
        let back = Spring::from_duration_bounce(0.7, bounce).unwrap().perceptual().unwrap();
        assert!((back.duration_s - 0.7).abs() < 1e-12);
        assert!(
            (back.bounce - bounce).abs() < 1e-12,
            "bounce {bounce} -> {}",
            back.bounce
        );
    }
    let from_response = Spring::from_response_damping_fraction(0.55, 0.8).unwrap();
    assert!((from_response.damping_ratio().unwrap() - 0.8).abs() < 1e-12);
}

#[test]
fn invalid_parameters_are_errors_not_silent_fixes() {
    assert_eq!(
        Spring::from_duration_bounce(0.0, 0.0),
        Err(SpringError::InvalidDuration(0.0))
    );
    assert!(Spring::from_duration_bounce(f64::NAN, 0.0).is_err());
    assert_eq!(
        Spring::from_duration_bounce(0.5, -1.0),
        Err(SpringError::BounceOutOfRange(-1.0))
    );
    assert_eq!(
        Spring::from_duration_bounce(0.5, 1.01),
        Err(SpringError::BounceOutOfRange(1.01))
    );
    assert!(Spring::from_duration_bounce(0.5, f64::NAN).is_err());
    assert!(Spring::from_response_damping_fraction(-0.1, 1.0).is_err());
    assert!(Spring::from_response_damping_fraction(0.5, -0.1).is_err());
    assert!(Spring::from_physical(0.0, 100.0, 1.0).is_err());
    assert!(Spring::from_physical(1.0, 0.0, 1.0).is_err());
    assert!(Spring::from_physical(1.0, 100.0, -1.0).is_err());
    assert!(Spring::from_physical(1.0, f64::INFINITY, 1.0).is_err());
    assert!(Spring::smooth_with_duration(-1.0).is_err());
    // Граница bounce = 1 допустима: затухания нет.
    assert_eq!(
        Spring::from_duration_bounce(0.5, 1.0)
            .unwrap()
            .physical()
            .unwrap()
            .damping,
        0.0
    );
}

#[test]
fn response_zero_is_immediate_spring_without_physics() {
    let immediate = Spring::from_response_damping_fraction(0.0, 1.0).unwrap();
    assert!(immediate.is_immediate());
    assert_eq!(immediate, Spring::immediate());
    assert!(immediate.physical().is_none() && immediate.perceptual().is_none());
    let state = immediate.evolve(
        SpringState {
            displacement: 5.0,
            velocity: 9.0,
        },
        0.0,
    );
    assert_eq!((state.displacement, state.velocity), (0.0, 0.0));
}

#[test]
fn analytic_solution_matches_numerical_integration_in_all_regimes() {
    // (mass, stiffness, damping): колебания, критическое, перезатухание (bounce ≈ −0,13 камеры),
    // сильное перезатухание, ζ около 1 с обеих сторон, почти без затухания, масса ≠ 1.
    let omega: f64 = 10.0;
    let cases = [
        ("bouncy", physical(&Spring::bouncy())),
        ("critical", (1.0, omega * omega, 2.0 * omega)),
        (
            "overdamped camera",
            physical(&Spring::from_duration_bounce(0.85, -0.13).unwrap()),
        ),
        ("overdamped strong", (1.0, 100.0, 60.0)),
        ("zeta 1-1e-6", (1.0, 100.0, 2.0 * omega * (1.0 - 1e-6))),
        ("zeta 1+1e-6", (1.0, 100.0, 2.0 * omega * (1.0 + 1e-6))),
        ("zeta 1-1e-4", (1.0, 100.0, 2.0 * omega * (1.0 - 1e-4))),
        (
            "light damping",
            physical(&Spring::from_duration_bounce(0.5, 0.95).unwrap()),
        ),
        ("mass 2.5", (2.5, 300.0, 12.0)),
    ];
    for (name, (mass, stiffness, damping)) in cases {
        let spring = Spring::from_physical(mass, stiffness, damping).unwrap();
        for (x0, v0) in [(100.0, 0.0), (-40.0, 900.0), (0.0, -500.0)] {
            for elapsed in [0.05, 0.4, 1.3] {
                let expected = rk4(mass, stiffness, damping, x0, v0, elapsed);
                let got = spring.evolve(
                    SpringState {
                        displacement: x0,
                        velocity: v0,
                    },
                    elapsed,
                );
                assert!(
                    (got.displacement - expected.0).abs() < 1e-6,
                    "{name} x t={elapsed}: {} vs {}",
                    got.displacement,
                    expected.0
                );
                assert!(
                    (got.velocity - expected.1).abs() < 1e-5,
                    "{name} v t={elapsed}: {} vs {}",
                    got.velocity,
                    expected.1
                );
            }
        }
    }
}

#[test]
fn solution_is_continuous_across_critical_damping() {
    // Независимый эталон критического случая: x = e^(−ωt)·(x0 + (v0 + ω·x0)·t).
    let omega: f64 = 10.0;
    let (x0, v0) = (100.0, -300.0);
    for delta in [1e-9, 1e-7, 1e-5, 1e-4] {
        for sign in [-1.0, 1.0] {
            let spring = Spring::from_physical(1.0, omega * omega, 2.0 * omega * (1.0 + sign * delta)).unwrap();
            for elapsed in [0.01, 0.1, 0.5, 2.0, 5.0] {
                let critical_x = (-omega * elapsed).exp() * (x0 + (v0 + omega * x0) * elapsed);
                let got = spring.evolve(
                    SpringState {
                        displacement: x0,
                        velocity: v0,
                    },
                    elapsed,
                );
                // Отличие от критического — порядка delta; формула с делением на √|1−ζ²| дала бы шум.
                let allowed = 1e-6 + 200.0 * delta;
                assert!(
                    (got.displacement - critical_x).abs() < allowed,
                    "delta {delta} t {elapsed}: {} vs {critical_x}",
                    got.displacement
                );
            }
        }
    }
    // Плотная развёртка по ζ через единицу: соседние значения отличаются плавно (нет скачка).
    let mut previous: Option<f64> = None;
    for step in -50..=50 {
        let ratio = 1.0 + f64::from(step) * 2e-6;
        let spring = Spring::from_physical(1.0, 100.0, 20.0 * ratio).unwrap();
        let value = spring
            .evolve(
                SpringState {
                    displacement: 100.0,
                    velocity: 0.0,
                },
                0.7,
            )
            .displacement;
        if let Some(prev) = previous {
            assert!((value - prev).abs() < 1e-3, "скачок при ζ = {ratio}: {prev} -> {value}");
        }
        previous = Some(value);
    }
}

#[test]
fn undamped_spring_never_settles_and_keeps_amplitude() {
    let spring = Spring::from_duration_bounce(0.5, 1.0).unwrap();
    let animation = start_animation(spring, 0.0, 100.0, 0.0);
    // Через 100 периодов амплитуда та же: -100 -> cos; на целом числе периодов значение ≈ 0.
    let after_full_periods = animation.sample(at_seconds(1.0 + 50.0));
    assert_eq!(after_full_periods.phase, SpringPhase::Moving);
    assert!(after_full_periods.value.abs() < 1e-6, "{}", after_full_periods.value);
    let half_period = animation.sample(at_seconds(1.0 + 0.25));
    assert!(
        (half_period.value - 200.0).abs() < 1e-6,
        "противоположный край: {}",
        half_period.value
    );
}

#[test]
fn animation_reaches_exact_target_and_reports_rest() {
    let animation = start_animation(Spring::smooth(), 0.0, 100.0, 0.0);
    assert_eq!(animation.sample(at_seconds(1.0)).value, 0.0);
    let mid = animation.sample(at_seconds(1.2));
    assert_eq!(mid.phase, SpringPhase::Moving);
    assert!(mid.value > 0.0 && mid.value < 100.0 && mid.velocity > 0.0);
    let end = animation.sample(at_seconds(10.0));
    assert_eq!((end.value, end.velocity, end.phase), (100.0, 0.0, SpringPhase::AtRest));
}

#[test]
fn time_before_start_is_start_state_not_extrapolation() {
    let animation = start_animation(Spring::bouncy(), 10.0, 50.0, 200.0);
    let at_start = animation.sample(at_seconds(1.0));
    let before = animation.sample(at_seconds(0.3));
    assert_eq!(before.value.to_bits(), at_start.value.to_bits());
    assert_eq!(before.velocity.to_bits(), at_start.velocity.to_bits());
    assert_eq!(before.value, 10.0);
}

#[test]
fn samples_at_60_and_120_hz_are_bitwise_equal_at_common_moments() {
    // Момент кадра = round(кадр · 1e9 / частота). Для 120 Гц чётные кадры совпадают с кадрами 60 Гц
    // до наносекунды. Оба «прогона» одинаково перенацеливаются в общий кадр 10.
    let frame_time = |index: i64, hertz: i64| {
        MonotonicNanos::from_nanos(1_000_000_000 + (index * 1_000_000_000 + hertz / 2) / hertz)
    };
    let run = |hertz: i64, retarget_frame: i64| {
        let mut animation = start_animation(Spring::snappy(), 0.0, 300.0, 700.0);
        let frames = 90 * hertz / 60;
        let mut samples = Vec::new();
        for index in 0..frames {
            if index == retarget_frame {
                animation.retarget(-120.0, frame_time(index, hertz)).unwrap();
            }
            samples.push(animation.sample(frame_time(index, hertz)));
        }
        samples
    };
    let (at_60, at_120) = (run(60, 10), run(120, 20));
    for (index, sample) in at_60.iter().enumerate() {
        let twin = at_120[index * 2];
        assert_eq!(sample.value.to_bits(), twin.value.to_bits(), "кадр {index}");
        assert_eq!(sample.velocity.to_bits(), twin.velocity.to_bits(), "кадр {index}");
    }
}

#[test]
fn settling_time_of_ca_spring_animation_is_in_documented_range() {
    // motion.md 1.2: k 100, c 5 -> ≈ 2,85 с; c 15 -> чуть больше 1 с. Критерий не подгонялся:
    // смотрим, что он даёт, и проверяем только грубую границу.
    let settle = |damping: f64| {
        let spring = Spring::from_physical(1.0, 100.0, damping).unwrap();
        let animation = start_animation(spring, 0.0, 1000.0, 0.0);
        let mut last_moving = 0.0;
        for millis in 0..20_000 {
            let now = at_seconds(1.0 + f64::from(millis) / 1000.0);
            if animation.sample(now).phase == SpringPhase::Moving {
                last_moving = f64::from(millis) / 1000.0;
            }
        }
        last_moving
    };
    let (slow, fast) = (settle(5.0), settle(15.0));
    assert!((2.5..3.2).contains(&slow), "c=5: {slow} с");
    assert!((0.8..1.4).contains(&fast), "c=15: {fast} с");
}

#[test]
fn retarget_mid_flight_keeps_position_and_velocity_continuous() {
    let mut animation = start_animation(Spring::bouncy(), 0.0, 100.0, 0.0);
    let moment = at_seconds(1.15);
    let before = animation.sample(moment);
    animation.retarget(-60.0, moment).unwrap();
    let after = animation.sample(moment);
    assert!(
        (after.value - before.value).abs() < 1e-9,
        "позиция: {} -> {}",
        before.value,
        after.value
    );
    assert!(
        (after.velocity - before.velocity).abs() < 1e-9,
        "скорость: {} -> {}",
        before.velocity,
        after.velocity
    );
    assert!(before.velocity > 0.0, "скорость к старой цели была ненулевой");
    assert_eq!(animation.target(), -60.0);

    // Дальше траектория = решение от новых начальных условий (независимо: RK4).
    let (mass, stiffness, damping) = physical(&Spring::bouncy());
    let (x_expected, v_expected) = rk4(mass, stiffness, damping, before.value + 60.0, before.velocity, 0.3);
    let later = animation.sample(at_seconds(1.15 + 0.3));
    assert!((later.value - (-60.0 + x_expected)).abs() < 1e-6);
    assert!((later.velocity - v_expected).abs() < 1e-5);
}

#[test]
fn retarget_with_swaps_spring_and_inherits_velocity() {
    let mut animation = start_animation(Spring::smooth(), 0.0, 100.0, 0.0);
    let moment = at_seconds(1.1);
    let before = animation.sample(moment);
    animation.retarget_with(Spring::bouncy(), 100.0, moment).unwrap();
    let after = animation.sample(moment);
    assert!((after.value - before.value).abs() < 1e-9 && (after.velocity - before.velocity).abs() < 1e-9);
    assert_eq!(animation.spring(), Spring::bouncy());
    // Более упругая пружина перелетает цель, а .smooth — нет.
    let overshoot = (0..200)
        .map(|i| animation.sample(at_seconds(1.1 + f64::from(i) * 0.005)).value)
        .fold(f64::MIN, f64::max);
    assert!(overshoot > 100.0, "перелёта нет: {overshoot}");
}

#[test]
fn retarget_to_same_point_with_velocity_is_not_instant_rest() {
    let mut animation = start_animation(Spring::smooth(), 0.0, 100.0, 0.0);
    let moment = at_seconds(1.1);
    let moving_fast = animation.sample(moment);
    assert!(moving_fast.velocity.abs() > 10.0);
    // Цель та же, расстояние от текущей точки не нулевое, но проверим и строго нулевое расстояние:
    let mut at_target = start_animation(Spring::bouncy(), 100.0, 100.0, 400.0);
    assert_eq!(
        at_target.sample(at_seconds(1.0)).phase,
        SpringPhase::Moving,
        "ненулевая скорость в цели — не покой"
    );
    at_target.retarget(100.0, at_seconds(1.0)).unwrap();
    assert_eq!(at_target.sample(at_seconds(1.0)).phase, SpringPhase::Moving);
    let overshoot = at_target.sample(at_seconds(1.05)).value;
    assert!(overshoot > 100.0, "скорость раскрутила пружину: {overshoot}");
    assert_eq!(at_target.sample(at_seconds(20.0)).phase, SpringPhase::AtRest);
    animation.retarget(100.0, moment).unwrap();
    assert_eq!(animation.sample(moment).phase, SpringPhase::Moving);
}

#[test]
fn retarget_after_rest_starts_from_target_with_zero_velocity() {
    let mut animation = start_animation(Spring::smooth(), 0.0, 100.0, 0.0);
    animation.retarget(40.0, at_seconds(30.0)).unwrap();
    let at_retarget = animation.sample(at_seconds(30.0));
    assert_eq!((at_retarget.value, at_retarget.velocity), (100.0, 0.0));
    assert_eq!(animation.sample(at_seconds(60.0)).value, 40.0);
}

#[test]
fn immediate_spring_follows_target_one_to_one() {
    let mut animation = start_animation(Spring::immediate(), 0.0, 25.0, 999.0);
    let sample = animation.sample(at_seconds(1.0));
    assert_eq!(
        (sample.value, sample.velocity, sample.phase),
        (25.0, 0.0, SpringPhase::AtRest)
    );
    animation.retarget(-7.0, at_seconds(1.0)).unwrap();
    assert_eq!(animation.sample(at_seconds(1.0)).value, -7.0);
}

#[test]
fn bad_animation_inputs_and_rest_criterion_are_rejected() {
    let base = SpringStart {
        spring: Spring::smooth(),
        from: 0.0,
        to: 1.0,
        initial_velocity: 0.0,
        start_time: at_seconds(0.0),
        rest: RestCriterion::default(),
    };
    assert!(matches!(
        SpringAnimation::start(SpringStart { from: f64::NAN, ..base }),
        Err(SpringError::NonFiniteInput { field: "from", .. })
    ));
    assert!(
        SpringAnimation::start(SpringStart {
            to: f64::INFINITY,
            ..base
        })
        .is_err()
    );
    assert!(
        SpringAnimation::start(SpringStart {
            initial_velocity: f64::NAN,
            ..base
        })
        .is_err()
    );
    let zero_floor = RestCriterion {
        absolute_position_floor: 0.0,
        ..RestCriterion::default()
    };
    assert!(
        SpringAnimation::start(SpringStart {
            rest: zero_floor,
            ..base
        })
        .is_err()
    );
    let negative_velocity = RestCriterion {
        velocity_tolerance_factor: -1.0,
        ..RestCriterion::default()
    };
    assert!(negative_velocity.validate().is_err());
    let mut animation = SpringAnimation::start(base).unwrap();
    assert!(animation.retarget(f64::NAN, at_seconds(0.0)).is_err());
    assert_eq!(animation.target(), 1.0, "ошибка не меняет состояние");
}

#[test]
fn rest_criterion_scales_with_distance_and_respects_floor() {
    let rest = RestCriterion::default();
    let far = rest.tolerance_for_distance(10_000.0, 10.0);
    assert!((far.position - 10.0).abs() < 1e-9);
    assert!(
        (far.velocity - 100.0).abs() < 1e-9,
        "скорость привязана к положению через ω0"
    );
    assert_eq!(
        rest.tolerance_for_distance(0.0, 10.0).position,
        rest.absolute_position_floor
    );
    // Позиция в допуске, но скорость велика — не покой.
    assert!(!far.contains(1.0, 1000.0));
    assert!(far.contains(1.0, 50.0));
}

#[test]
fn distance_fraction_velocity_helpers_follow_uikit_rule() {
    assert_eq!(velocity_from_distance_fraction(2.0, 10.0, 60.0), 100.0);
    assert_eq!(distance_fraction_per_second(100.0, 10.0, 60.0), 2.0);
    assert_eq!(
        distance_fraction_per_second(100.0, 5.0, 5.0),
        0.0,
        "нулевое расстояние -> 0"
    );
}

#[test]
fn spring_spec_parses_from_json_and_builds() {
    let spec: SpringSpec =
        serde_json::from_str(r#"{"kind":"duration_bounce","duration_s":0.85,"bounce":-0.13}"#).unwrap();
    let spring = spec.build().unwrap();
    assert!((spring.perceptual().unwrap().bounce + 0.13).abs() < 1e-12);
    assert!(serde_json::from_str::<SpringSpec>(r#"{"kind":"immediate","extra":1}"#).is_err());
    let bad: SpringSpec = serde_json::from_str(r#"{"kind":"duration_bounce","duration_s":0.0,"bounce":0.0}"#).unwrap();
    assert!(bad.build().is_err());
}

fn additive(spring: Spring, from: f64, to: f64, limit: usize) -> AdditiveSpringAnimation {
    AdditiveSpringAnimation::start(AdditiveSpringStart {
        spring,
        from,
        to,
        initial_velocity: 0.0,
        start_time: at_seconds(1.0),
        rest: RestCriterion::default(),
        max_live_contributions: limit,
    })
    .unwrap()
}

#[test]
fn additive_retarget_is_continuous_and_matches_superposition() {
    let spring = Spring::bouncy();
    let mut animation = additive(spring, 0.0, 100.0, 8);
    let moment = at_seconds(1.2);
    let before = animation.sample(moment);
    animation.set_target(50.0, moment).unwrap();
    let after = animation.sample(moment);
    assert!((after.value - before.value).abs() < 1e-9 && (after.velocity - before.velocity).abs() < 1e-9);
    assert_eq!(animation.contribution_count(), 2);

    // Независимый эталон: линейность -> два интегрирования RK4, значения складываются.
    let (mass, stiffness, damping) = physical(&spring);
    let later = 0.25;
    let first = rk4(mass, stiffness, damping, -100.0, 0.0, 0.2 + later);
    let second = rk4(mass, stiffness, damping, 100.0 - 50.0, 0.0, later);
    let got = animation.sample(at_seconds(1.2 + later));
    assert!(
        (got.value - (50.0 + first.0 + second.0)).abs() < 1e-5,
        "{} vs {}",
        got.value,
        50.0 + first.0 + second.0
    );
    assert!((got.velocity - (first.1 + second.1)).abs() < 1e-4);
}

#[test]
fn additive_equals_replace_for_one_spring_and_differs_for_mixed_springs() {
    // Решение линейно: при одной пружине сумма вкладов в точности равна «замене».
    let mut summed = additive(Spring::smooth(), 0.0, 100.0, 8);
    summed.set_target(200.0, at_seconds(1.05)).unwrap();
    let mut replaced = start_animation(Spring::smooth(), 0.0, 100.0, 0.0);
    replaced.retarget(200.0, at_seconds(1.05)).unwrap();
    let moment = at_seconds(1.15);
    let (a, b) = (summed.sample(moment), replaced.sample(moment));
    assert!((a.value - b.value).abs() < 1e-9 && (a.velocity - b.velocity).abs() < 1e-9);

    // Со сменой пружины политики расходятся: у «сложения» старый остаток доживает на старой пружине.
    let mut summed = additive(Spring::smooth(), 0.0, 100.0, 8);
    summed
        .set_target_with(Spring::bouncy(), 200.0, at_seconds(1.05))
        .unwrap();
    let mut replaced = start_animation(Spring::smooth(), 0.0, 100.0, 0.0);
    replaced
        .retarget_with(Spring::bouncy(), 200.0, at_seconds(1.05))
        .unwrap();
    let (a, b) = (summed.sample(moment), replaced.sample(moment));
    assert!((a.value - b.value).abs() > 0.1, "{} vs {}", a.value, b.value);
}

#[test]
fn additive_caps_live_contributions_and_settles_exactly_on_last_target() {
    let mut animation = additive(Spring::bouncy(), 0.0, 10.0, 3);
    for index in 1..=12 {
        let moment = at_seconds(1.0 + f64::from(index) * 0.02);
        animation.set_target(f64::from(index) * 17.0 % 90.0, moment).unwrap();
        assert!(animation.contribution_count() <= 3);
    }
    let last_target = animation.target();
    let end = animation.sample(at_seconds(40.0));
    assert_eq!(
        (end.value, end.velocity, end.phase),
        (last_target, 0.0, SpringPhase::AtRest)
    );
}

#[test]
fn additive_merge_of_same_spring_is_exact() {
    // Предел 1 заставляет сливать вклады на каждом шаге; для одной пружины это точно (линейность).
    let steps = [(1.1, 80.0), (1.2, 20.0), (1.3, 60.0)];
    let mut capped = additive(Spring::bouncy(), 0.0, 100.0, 1);
    let mut free = additive(Spring::bouncy(), 0.0, 100.0, 64);
    for (time, target) in steps {
        capped.set_target(target, at_seconds(time)).unwrap();
        free.set_target(target, at_seconds(time)).unwrap();
    }
    for time in [1.35, 1.5] {
        let (a, b) = (capped.sample(at_seconds(time)), free.sample(at_seconds(time)));
        assert!((a.value - b.value).abs() < 1e-9 && (a.velocity - b.velocity).abs() < 1e-8);
    }
}

#[test]
fn additive_rejects_bad_input_and_immediate_clears_contributions() {
    let bad_limit = AdditiveSpringStart {
        spring: Spring::smooth(),
        from: 0.0,
        to: 1.0,
        initial_velocity: 0.0,
        start_time: at_seconds(0.0),
        rest: RestCriterion::default(),
        max_live_contributions: 0,
    };
    assert_eq!(
        AdditiveSpringAnimation::start(bad_limit).unwrap_err(),
        SpringError::InvalidContributionLimit
    );
    let mut animation = additive(Spring::bouncy(), 0.0, 100.0, 8);
    assert!(animation.set_target(f64::NAN, at_seconds(1.1)).is_err());
    assert_eq!(animation.target(), 100.0);
    animation
        .set_target_with(Spring::immediate(), 7.0, at_seconds(1.1))
        .unwrap();
    let sample = animation.sample(at_seconds(1.1));
    assert_eq!(
        (sample.value, sample.phase, animation.contribution_count()),
        (7.0, SpringPhase::AtRest, 0)
    );
    // Тот же target без изменения не добавляет вклад.
    animation.set_target(7.0, at_seconds(1.2)).unwrap();
    assert_eq!(animation.contribution_count(), 0);
}

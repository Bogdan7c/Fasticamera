//! Тесты оценки скорости. Эталон — аналитическая скорость синтетической траектории
//! (производная заданной функции), а не вторая копия подгонки.

use super::*;
use crate::geometry::{Point, Vector};
use crate::time::MonotonicNanos;

/// Момент в миллисекундах.
fn at_ms(millis: f64) -> MonotonicNanos {
    MonotonicNanos::from_nanos((millis * 1e6).round() as i64)
}

fn tracker() -> VelocityTracker {
    VelocityTracker::new(VelocityConfig::default()).expect("конфиг по умолчанию верен")
}

/// Заполнить трекер траекторией `position(t_sec)` с шагом `step_ms` от 0 до `end_ms` включительно.
fn feed(tracker: &mut VelocityTracker, step_ms: f64, end_ms: f64, position: impl Fn(f64) -> Point) {
    let mut time_ms = 0.0;
    while time_ms <= end_ms + 1e-9 {
        tracker.add_sample(at_ms(time_ms), position(time_ms / 1000.0));
        time_ms += step_ms;
    }
}

fn expect_vector(estimate: VelocityEstimate<Vector>) -> Vector {
    match estimate {
        VelocityEstimate::Measured(velocity) => velocity,
        other => panic!("ожидали измеренную скорость, получили {other:?}"),
    }
}

#[test]
fn uniform_motion_gives_true_velocity_at_60_and_120_hz() {
    // x = 100 + 800 t, y = 50 − 300 t  =>  v = (800, −300) pt/с.
    for step_ms in [1000.0 / 60.0, 1000.0 / 120.0] {
        let mut tracker = tracker();
        feed(&mut tracker, step_ms, 300.0, |t| {
            Point::new(100.0 + 800.0 * t, 50.0 - 300.0 * t)
        });
        let end = at_ms(300.0 - (300.0 % step_ms));
        let velocity = expect_vector(tracker.estimate_at(end));
        assert!((velocity.dx - 800.0).abs() < 0.5, "dx = {}", velocity.dx);
        assert!((velocity.dy + 300.0).abs() < 0.5, "dy = {}", velocity.dy);
    }
}

#[test]
fn uniform_acceleration_gives_velocity_at_last_point_not_average() {
    // x = 0,5·a·t², a = 2000 pt/с²; в t = 0,3 с скорость a·t = 600, средняя за 100 мс ≈ 500.
    let mut tracker = tracker();
    feed(&mut tracker, 1000.0 / 120.0, 300.0, |t| {
        Point::new(0.5 * 2000.0 * t * t, 0.0)
    });
    let last_ms = (300.0_f64 / (1000.0 / 120.0)).floor() * (1000.0 / 120.0);
    let expected = 2000.0 * last_ms / 1000.0;
    let velocity = expect_vector(tracker.estimate_at(at_ms(last_ms)));
    assert!(
        (velocity.dx - expected).abs() < 1.0,
        "{} против {expected}",
        velocity.dx
    );
    assert!(velocity.dy.abs() < 1e-6);
}

#[test]
fn deceleration_before_lift_is_seen_through_acceleration_not_timer() {
    // Палец тормозит: x = 1000 t − 4000 t², v(t) = 1000 − 8000 t. К t = 0,12 с скорость упала
    // до 40 pt/с, касания при этом идут (таймер «стоп» не срабатывает). Средняя скорость за
    // окно 20…120 мс ≈ 500 pt/с, наивная разность двух последних точек — 80 pt/с.
    let mut tracker = tracker();
    for index in 0..=12 {
        let t = f64::from(index) * 0.01;
        tracker.add_sample(at_ms(t * 1000.0), Point::new(1000.0 * t - 4000.0 * t * t, 0.0));
    }
    let velocity = expect_vector(tracker.estimate_at(at_ms(120.0)));
    assert!(
        (velocity.dx - 40.0).abs() < 0.5,
        "{} против аналитических 40",
        velocity.dx
    );
}

#[test]
fn stop_after_pause_longer_than_threshold_gives_zero() {
    let mut tracker = tracker();
    feed(&mut tracker, 8.0, 80.0, |t| Point::new(1000.0 * t, 0.0));
    assert!(expect_vector(tracker.estimate_at(at_ms(80.0))).dx > 900.0);
    // Палец застыл: касаний нет. Запрос через 41 мс после последней точки => стоп.
    assert_eq!(tracker.estimate_at(at_ms(80.0 + 41.0)), VelocityEstimate::Stopped);
    assert_eq!(tracker.velocity_at(at_ms(80.0 + 41.0)), Vector::ZERO);
    // Ровно на пороге — ещё не стоп.
    assert!(matches!(
        tracker.estimate_at(at_ms(80.0 + 40.0)),
        VelocityEstimate::Measured(_)
    ));
}

#[test]
fn old_fast_samples_do_not_leak_into_motion_after_a_pause() {
    let mut tracker = tracker();
    feed(&mut tracker, 8.0, 80.0, |t| Point::new(2000.0 * t, 0.0));
    // Пауза 200 мс, затем медленное движение 100 pt/с.
    let outcome = tracker.add_sample(at_ms(280.0), Point::new(160.0, 0.0));
    assert_eq!(outcome, SampleOutcome::AddedAfterPause);
    assert_eq!(tracker.sample_count(), 1, "история стёрта");
    tracker.add_sample(at_ms(290.0), Point::new(161.0, 0.0));
    tracker.add_sample(at_ms(300.0), Point::new(162.0, 0.0));
    let velocity = expect_vector(tracker.estimate_at(at_ms(300.0)));
    assert!(
        (velocity.dx - 100.0).abs() < 1.0,
        "старые 2000 pt/с просочились: {}",
        velocity.dx
    );
}

#[test]
fn last_point_jerk_is_damped_versus_naive_two_point_difference() {
    // 120 Гц, равномерно 1000 pt/с, последняя точка сдвинута на лишние 3 pt (дребезг датчика).
    let step_ms = 1000.0 / 120.0;
    let mut clean = tracker();
    let mut jerked = tracker();
    for index in 0..=12 {
        let time_ms = f64::from(index) * step_ms;
        let x = 1000.0 * time_ms / 1000.0;
        clean.add_sample(at_ms(time_ms), Point::new(x, 0.0));
        let shifted = if index == 12 { x + 3.0 } else { x };
        jerked.add_sample(at_ms(time_ms), Point::new(shifted, 0.0));
    }
    let end = at_ms(12.0 * step_ms);
    let clean_velocity = expect_vector(clean.estimate_at(end)).dx;
    let jerked_velocity = expect_vector(jerked.estimate_at(end)).dx;
    // Эталон: наивная разность двух последних точек ошиблась бы на 3 / Δt = 360 pt/с.
    let naive_error = 3.0 / (step_ms / 1000.0);
    let lsq_error = (jerked_velocity - clean_velocity).abs();
    assert!((naive_error - 360.0).abs() < 1e-6);
    // Расчёт (независимый, numpy.polyfit): ошибка подгонки ≈ 59 pt/с против 360 у наивной разности, т. е. в ~6 раз;
    // порог «в 4 раза» оставляет запас на округление и ловит поломку подгонки.
    assert!(
        lsq_error < naive_error / 4.0,
        "подгонка должна гасить рывок: ошибка {lsq_error}, наивная {naive_error}"
    );
}

#[test]
fn degenerate_inputs_give_no_estimate_and_zero_without_nan() {
    let mut tracker = tracker();
    assert_eq!(tracker.estimate_at(at_ms(0.0)), VelocityEstimate::NotEnoughSamples);
    tracker.add_sample(at_ms(10.0), Point::new(5.0, 5.0));
    assert_eq!(tracker.estimate_at(at_ms(10.0)), VelocityEstimate::NotEnoughSamples);
    assert_eq!(tracker.velocity_at(at_ms(10.0)), Vector::ZERO);

    // Все точки с одной меткой времени: заменяют друг друга, остаётся одна.
    let mut same_time = VelocityTracker::new(VelocityConfig::default()).expect("конфиг");
    for index in 0..5 {
        let outcome = same_time.add_sample(at_ms(10.0), Point::new(f64::from(index), 0.0));
        if index > 0 {
            assert_eq!(outcome, SampleOutcome::ReplacedSameTime);
        }
    }
    assert_eq!(same_time.sample_count(), 1);
    assert_eq!(same_time.velocity_at(at_ms(10.0)), Vector::ZERO);
}

#[test]
fn invalid_samples_are_rejected_without_touching_history() {
    let mut tracker = tracker();
    tracker.add_sample(at_ms(10.0), Point::new(0.0, 0.0));
    tracker.add_sample(at_ms(20.0), Point::new(10.0, 0.0));
    assert_eq!(
        tracker.add_sample(at_ms(30.0), Point::new(f64::NAN, 0.0)),
        SampleOutcome::RejectedNonFinite
    );
    assert_eq!(
        tracker.add_sample(at_ms(30.0), Point::new(0.0, f64::INFINITY)),
        SampleOutcome::RejectedNonFinite
    );
    assert_eq!(
        tracker.add_sample(at_ms(15.0), Point::new(1.0, 0.0)),
        SampleOutcome::RejectedOutOfOrder
    );
    assert_eq!(tracker.sample_count(), 2);
    let velocity = expect_vector(tracker.estimate_at(at_ms(20.0)));
    assert!((velocity.dx - 1000.0).abs() < 1e-6, "{}", velocity.dx);
}

#[test]
fn history_is_limited_by_horizon_and_size() {
    let config = VelocityConfig {
        history_size: 5,
        ..VelocityConfig::default()
    };
    let mut small = VelocityTracker::new(config).expect("конфиг");
    feed(&mut small, 5.0, 60.0, |t| Point::new(t, 0.0));
    assert_eq!(small.sample_count(), 5, "кольцо истории");
    let mut wide = tracker();
    feed(&mut wide, 10.0, 500.0, |t| Point::new(t, 0.0));
    assert_eq!(wide.sample_count(), 11, "в горизонт 100 мс при шаге 10 мс: 11 точек");
}

#[test]
fn degree_one_ignores_curvature_while_degree_two_follows_it() {
    // x = 1000 t²: в t = 0,1 истинная скорость 200, прямая по участку 0…0,1 даст среднюю ~100.
    let linear_config = VelocityConfig {
        polynomial_degree: 1,
        ..VelocityConfig::default()
    };
    let mut linear = VelocityTracker::new(linear_config).expect("конфиг");
    let mut quadratic = tracker();
    for index in 0..=10 {
        let time_ms = f64::from(index) * 10.0;
        let x = 1000.0 * (time_ms / 1000.0).powi(2);
        linear.add_sample(at_ms(time_ms), Point::new(x, 0.0));
        quadratic.add_sample(at_ms(time_ms), Point::new(x, 0.0));
    }
    let linear_velocity = expect_vector(linear.estimate_at(at_ms(100.0))).dx;
    let quadratic_velocity = expect_vector(quadratic.estimate_at(at_ms(100.0))).dx;
    assert!((quadratic_velocity - 200.0).abs() < 0.5, "{quadratic_velocity}");
    assert!((linear_velocity - 100.0).abs() < 1.0, "{linear_velocity}");
}

#[test]
fn config_validation_rejects_bad_values() {
    let base = VelocityConfig::default();
    assert!(base.validate().is_ok());
    assert_eq!(
        VelocityConfig {
            polynomial_degree: 0,
            ..base.clone()
        }
        .validate(),
        Err(VelocityConfigError::InvalidDegree(0))
    );
    assert_eq!(
        VelocityConfig {
            polynomial_degree: 4,
            ..base.clone()
        }
        .validate(),
        Err(VelocityConfigError::InvalidDegree(4))
    );
    assert_eq!(
        VelocityConfig {
            horizon_ms: 0,
            ..base.clone()
        }
        .validate(),
        Err(VelocityConfigError::ZeroHorizon)
    );
    assert_eq!(
        VelocityConfig {
            history_size: 1,
            ..base.clone()
        }
        .validate(),
        Err(VelocityConfigError::HistoryTooSmall(1))
    );
    assert_eq!(
        VelocityConfig {
            assume_stopped_after_ms: 0,
            ..base
        }
        .validate(),
        Err(VelocityConfigError::ZeroStoppedThreshold)
    );
    assert!(
        VelocityTracker::new(VelocityConfig {
            horizon_ms: 0,
            ..VelocityConfig::default()
        })
        .is_err()
    );
}

#[test]
fn config_parses_from_json_and_rejects_unknown_fields() {
    let parsed: VelocityConfig = serde_json::from_str(r#"{"polynomial_degree":1}"#).expect("разбор");
    assert_eq!(parsed.polynomial_degree, 1);
    assert_eq!(parsed.horizon_ms, 100);
    assert!(serde_json::from_str::<VelocityConfig>(r#"{"horizon":1}"#).is_err());
}

#[test]
fn estimate_or_zero_never_returns_nan() {
    assert_eq!(VelocityEstimate::<f64>::NotEnoughSamples.or_zero(), 0.0);
    assert_eq!(VelocityEstimate::<f64>::Stopped.or_zero(), 0.0);
    assert_eq!(VelocityEstimate::Measured(f64::NAN).or_zero(), 0.0);
    assert_eq!(VelocityEstimate::Measured(3.5).or_zero(), 3.5);
}

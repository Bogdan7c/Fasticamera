#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Поведенческие тесты прокрутки. Эталоны независимы: контрольные числа `motion.md`, формула
//! «резинки» из спецификации, численное интегрирование скорости, проверки непрерывности по
//! плотной выборке траектории.

use ui_core::scroll::{
    AxisGeometry, DecelerationModel, DecelerationRate, ReleaseOutcome, RubberBand, ScrollAxis, ScrollConfig,
    ScrollError, ScrollPhase, ScrollSample, ScrollTransition, SnapSelection,
};
use ui_core::spring::SpringSpec;
use ui_core::time::MonotonicNanos;

fn ms(millis: i64) -> MonotonicNanos {
    MonotonicNanos::from_nanos(1_000_000_000 + millis * 1_000_000)
}

fn geometry(min: f64, max: f64) -> AxisGeometry {
    AxisGeometry {
        min_offset: min,
        max_offset: max,
        viewport_extent: 960.0,
    }
}

fn axis(min: f64, max: f64, initial: f64) -> ScrollAxis {
    ScrollAxis::new(&ScrollConfig::default(), geometry(min, max), initial).unwrap()
}

fn axis_with(config: ScrollConfig, min: f64, max: f64, initial: f64) -> ScrollAxis {
    ScrollAxis::new(&config, geometry(min, max), initial).unwrap()
}

/// Независимая формула «резинки» из спецификации.
fn rubber_formula(overshoot: f64, dimension: f64, coefficient: f64) -> f64 {
    (1.0 - 1.0 / (overshoot * coefficient / dimension + 1.0)) * dimension
}

/// Выборка с шагом 1 мс от `from_ms` на `duration_ms`.
fn trajectory(axis: &ScrollAxis, from_ms: i64, duration_ms: i64) -> Vec<ScrollSample> {
    (0..=duration_ms).map(|step| axis.sample(ms(from_ms + step))).collect()
}

/// Перетащить на `delta` и отпустить со скоростью `velocity`; возвращает результат релиза.
fn drag_and_release(axis: &mut ScrollAxis, delta: f64, velocity: f64, at_ms: i64) -> ReleaseOutcome {
    axis.begin_drag(ms(at_ms));
    axis.drag_to(delta).unwrap();
    axis.release(velocity, ms(at_ms + 10)).unwrap()
}

#[test]
fn rubber_band_matches_documented_checkpoints() {
    let band = RubberBand::new(0.55, 960.0).unwrap();
    for (overshoot, expected) in [(5.0, 2.74), (100.0, 52.02), (960.0, 340.65)] {
        let got = band.stretch(overshoot);
        assert!((got - expected).abs() < 0.01, "x={overshoot}: {got} vs {expected}");
        assert!((got - rubber_formula(overshoot, 960.0, 0.55)).abs() < 1e-12);
        assert_eq!(band.stretch(-overshoot), -got, "нечётность");
    }
    assert!(band.stretch(1e12) < 960.0, "асимптота d недостижима");
    assert_eq!(band.stretch(0.0), 0.0);
}

#[test]
fn rubber_band_inverse_and_slope_are_consistent() {
    let band = RubberBand::new(0.55, 960.0).unwrap();
    for overshoot in [0.0, 1.0, 37.5, 400.0, 5000.0, -250.0] {
        let round_trip = band.unstretch(band.stretch(overshoot));
        assert!(
            (round_trip - overshoot).abs() <= 1e-6 * overshoot.abs().max(1.0),
            "{overshoot} -> {round_trip}"
        );
    }
    // Наклон = конечная разность (независимо от аналитической производной).
    for overshoot in [0.0, 50.0, 700.0] {
        let step = 1e-4;
        let numeric = (band.stretch(overshoot + step) - band.stretch(overshoot - step)) / (2.0 * step);
        assert!((band.slope(overshoot) - numeric).abs() < 1e-6, "{overshoot}");
    }
    assert!(band.unstretch(10_000.0).is_finite(), "смещение >= d не даёт inf");
}

#[test]
fn invalid_rubber_and_deceleration_parameters_are_errors() {
    assert!(matches!(
        RubberBand::new(0.0, 960.0),
        Err(ScrollError::InvalidRubberCoefficient(_))
    ));
    assert!(matches!(
        RubberBand::new(0.55, 0.0),
        Err(ScrollError::InvalidViewportExtent(_))
    ));
    assert!(RubberBand::new(f64::NAN, 960.0).is_err());
    for rate in [0.0, 1.0, 1.5, -0.1, f64::NAN] {
        assert!(DecelerationRate::new(rate).is_err(), "{rate}");
    }
    assert_eq!(DecelerationRate::new(0.998).unwrap(), DecelerationRate::normal());
    assert_eq!(DecelerationRate::fast().per_millisecond(), 0.99);
    assert!(DecelerationModel::new(DecelerationRate::normal(), 0.0).is_err());
}

#[test]
fn projection_matches_wwdc_formula_within_documented_difference() {
    // Непрерывная модель v0/λ против дискретной формулы WWDC: ~0,1 % (normal) и ~0,5 % (fast).
    let velocity = 8_000.0;
    for (rate, expected_gap) in [(DecelerationRate::normal(), 0.001), (DecelerationRate::fast(), 0.005)] {
        let model = DecelerationModel::new(rate, 0.5).unwrap();
        let ours = model.stopping_distance(velocity);
        let wwdc = rate.wwdc_discrete_projection(velocity);
        let gap = (ours - wwdc) / ours;
        assert!(
            (gap - expected_gap).abs() < 0.0006,
            "gap {gap} для d = {}",
            rate.per_millisecond()
        );
        assert!(ours > wwdc, "интеграл больше дискретной суммы");
    }
    // Знак и порог.
    let model = DecelerationModel::new(DecelerationRate::normal(), 0.5).unwrap();
    assert_eq!(model.stopping_distance(-velocity), -model.stopping_distance(velocity));
    assert_eq!(model.stopping_distance(0.4), 0.0);
}

#[test]
fn fling_lands_exactly_where_the_projection_said() {
    let mut scroll = axis(-100_000.0, 100_000.0, 0.0);
    let velocity = 3_000.0;
    let outcome = drag_and_release(&mut scroll, 0.0, velocity, 0);
    let ReleaseOutcome::Coasting { projected_offset } = outcome else {
        panic!("{outcome:?}")
    };

    // Независимый эталон: численно интегрируем v(t) = v0·d^(1000·t) шагом 0,1 мс (метод трапеций)
    // до порога 0,5 pt/с.
    let (mut position, mut speed, step) = (0.0_f64, velocity, 1e-4);
    let per_step = 0.998_f64.powf(1000.0 * step);
    while speed > 0.5 {
        let next = speed * per_step;
        position += 0.5 * (speed + next) * step;
        speed = next;
    }
    assert!(
        (projected_offset - position).abs() < 0.05,
        "{projected_offset} vs {position}"
    );

    let end = scroll.sample(ms(60_000));
    assert_eq!(
        (end.offset, end.velocity, end.phase),
        (projected_offset, 0.0, ScrollPhase::Resting)
    );
    let mid = scroll.sample(ms(400));
    assert_eq!(mid.phase, ScrollPhase::Decelerating);
    assert!(mid.velocity > 0.0 && mid.velocity < velocity && mid.offset > 0.0 && mid.offset < projected_offset);
}

#[test]
fn slow_release_without_snap_points_just_rests() {
    let mut scroll = axis(0.0, 1000.0, 0.0);
    let outcome = drag_and_release(&mut scroll, 40.0, 0.1, 0);
    assert_eq!(outcome, ReleaseOutcome::Settled { offset: 40.0 });
    let sample = scroll.sample(ms(5_000));
    assert_eq!((sample.offset, sample.phase), (40.0, ScrollPhase::Resting));
}

#[test]
fn drag_inside_bounds_is_one_to_one_and_outside_uses_rubber_band() {
    let mut scroll = axis(0.0, 1000.0, 500.0);
    scroll.begin_drag(ms(0));
    scroll.drag_to(120.0).unwrap();
    let inside = scroll.sample(ms(1));
    assert_eq!((inside.offset, inside.phase), (620.0, ScrollPhase::Dragging));
    scroll.drag_to(500.0 + 100.0).unwrap(); // 100 pt за верхним краем 1000
    assert!((scroll.sample(ms(2)).offset - (1000.0 + 52.02)).abs() < 0.01);
    scroll.drag_to(-500.0 - 100.0).unwrap(); // 100 pt за нижним краем
    assert!((scroll.sample(ms(3)).offset - (0.0 - 52.02)).abs() < 0.01);
}

#[test]
fn fling_past_the_edge_returns_by_spring_without_jumps() {
    let mut scroll = axis(0.0, 1000.0, 900.0);
    let outcome = drag_and_release(&mut scroll, 0.0, 3_000.0, 0);
    assert_eq!(outcome, ReleaseOutcome::CoastsPastEdge { edge_offset: 1000.0 });

    let samples = trajectory(&scroll, 10, 4_000);
    for pair in samples.windows(2) {
        // 3000 pt/с за 1 мс = 3 pt; скачок позиции или сброс скорости (≈ 2800 -> 0) был бы заметен.
        assert!(
            (pair[1].offset - pair[0].offset).abs() < 3.2,
            "скачок позиции: {pair:?}"
        );
        assert!(
            (pair[1].velocity - pair[0].velocity).abs() < 120.0,
            "скачок скорости: {pair:?}"
        );
    }
    let phases: Vec<ScrollPhase> = samples.iter().map(|s| s.phase).fold(Vec::new(), |mut acc, phase| {
        if acc.last() != Some(&phase) {
            acc.push(phase);
        }
        acc
    });
    assert_eq!(
        phases,
        [
            ScrollPhase::Decelerating,
            ScrollPhase::ReturningFromEdge,
            ScrollPhase::Resting
        ]
    );
    let peak = samples.iter().map(|s| s.offset).fold(f64::MIN, f64::max);
    assert!(peak > 1000.0, "за край уезжает и возвращается");
    let last = samples[samples.len() - 1];
    assert_eq!((last.offset, last.velocity), (1000.0, 0.0), "приехал ровно на край");
}

#[test]
fn release_from_overscroll_springs_back_starting_where_the_finger_left() {
    let mut scroll = axis(0.0, 1000.0, 0.0);
    scroll.begin_drag(ms(0));
    scroll.drag_to(-300.0).unwrap();
    let before = scroll.sample(ms(5)).offset;
    assert!(before < -100.0);
    let outcome = scroll.release(0.0, ms(5)).unwrap();
    assert_eq!(
        outcome,
        ReleaseOutcome::ReturnsFromOverscroll {
            offset: 0.0,
            snap_index: None
        }
    );
    assert_eq!(scroll.sample(ms(5)).offset, before, "отпускание не двигает контент");
    let samples = trajectory(&scroll, 5, 3_000);
    assert!(samples.windows(2).all(|p| (p[1].offset - p[0].offset).abs() < 3.0));
    assert!(
        samples.iter().all(|s| s.offset <= 1e-9),
        "из-за края без бросков вглубь"
    );
    assert_eq!(samples[samples.len() - 1].offset, 0.0);
}

#[test]
fn grabbing_content_during_edge_return_is_seamless() {
    let mut scroll = axis(0.0, 1000.0, 0.0);
    scroll.begin_drag(ms(0));
    scroll.drag_to(-400.0).unwrap();
    scroll.release(-500.0, ms(0)).unwrap(); // палец ещё тянул вниз при отпускании
    let grab_time = ms(120);
    let in_flight = scroll.sample(grab_time);
    assert_eq!(in_flight.phase, ScrollPhase::ReturningFromEdge);
    assert!(in_flight.offset < -1.0);

    scroll.begin_drag(grab_time);
    scroll.drag_to(0.0).unwrap();
    let held = scroll.sample(grab_time);
    assert!(
        (held.offset - in_flight.offset).abs() < 1e-9,
        "скачок при захвате: {} -> {}",
        in_flight.offset,
        held.offset
    );
    assert_eq!(held.phase, ScrollPhase::Dragging);

    // Дальше контент идёт за пальцем по независимой формуле: виртуальный x восстановлен обратной «резинкой».
    let overshoot_now = -in_flight.offset;
    let fraction = overshoot_now / 960.0;
    let virtual_overshoot = 960.0 / 0.55 * fraction / (1.0 - fraction);
    scroll.drag_to(-30.0).unwrap();
    let expected = -rubber_formula(virtual_overshoot + 30.0, 960.0, 0.55);
    assert!((scroll.sample(grab_time).offset - expected).abs() < 1e-6);
}

fn snap_config(selection: SnapSelection) -> ScrollConfig {
    ScrollConfig {
        snap_selection: selection,
        ..ScrollConfig::default()
    }
}

fn five_points(selection: SnapSelection, start: f64) -> ScrollAxis {
    let mut scroll = axis_with(snap_config(selection), 0.0, 400.0, start);
    scroll
        .set_snap_points(&[300.0, 0.0, 100.0, 400.0, 200.0, 100.0], ms(0))
        .unwrap();
    scroll
}

/// Скорость, при которой проекция (расстояние до остановки) равна `distance`.
fn velocity_for_distance(distance: f64) -> f64 {
    let model = DecelerationModel::new(DecelerationRate::normal(), 0.5).unwrap();
    distance * model.rate().decay_per_second() + model.stop_velocity()
}

#[test]
fn snap_points_are_sorted_and_deduplicated() {
    let scroll = five_points(SnapSelection::NearestToProjection, 0.0);
    assert_eq!(scroll.snap_points(), &[0.0, 100.0, 200.0, 300.0, 400.0]);
}

#[test]
fn fling_with_snap_points_picks_the_point_nearest_to_the_projection() {
    let mut scroll = five_points(SnapSelection::NearestToProjection, 0.0);
    // Проекция 260 -> ближайшая 300 (а не 200).
    let outcome = drag_and_release(&mut scroll, 0.0, velocity_for_distance(260.0), 0);
    assert_eq!(
        outcome,
        ReleaseOutcome::SnapsToPoint {
            index: 3,
            offset: 300.0
        }
    );
    let release_sample = scroll.sample(ms(10));
    assert!(
        (release_sample.velocity - velocity_for_distance(260.0)).abs() < 1e-6,
        "скорость пальца унаследована пружиной"
    );
    let mid = scroll.sample(ms(150));
    assert_eq!(mid.phase, ScrollPhase::SnappingToPoint);
    let end = scroll.sample(ms(10_000));
    assert_eq!(
        (end.offset, end.velocity, end.phase),
        (300.0, 0.0, ScrollPhase::Resting)
    );

    // Проекция 240 -> 200. Бросок вглубь за последнюю точку -> последняя точка (400).
    let mut scroll = five_points(SnapSelection::NearestToProjection, 0.0);
    assert_eq!(
        drag_and_release(&mut scroll, 0.0, velocity_for_distance(240.0), 0),
        ReleaseOutcome::SnapsToPoint {
            index: 2,
            offset: 200.0
        }
    );
    let mut scroll = five_points(SnapSelection::NearestToProjection, 0.0);
    assert_eq!(
        drag_and_release(&mut scroll, 0.0, velocity_for_distance(5_000.0), 0),
        ReleaseOutcome::SnapsToPoint {
            index: 4,
            offset: 400.0
        }
    );
}

#[test]
fn slow_release_between_points_snaps_to_the_nearest_one() {
    let mut scroll = five_points(SnapSelection::NearestToProjection, 0.0);
    let outcome = drag_and_release(&mut scroll, 130.0, 0.0, 0);
    assert_eq!(
        outcome,
        ReleaseOutcome::SnapsToPoint {
            index: 1,
            offset: 100.0
        }
    );
}

#[test]
fn adjacent_only_policy_moves_at_most_one_step_however_hard_the_flick() {
    // Старт на точке 200 (индекс 2). Долгий бросок в обе стороны — только 100 или 300.
    let hard = velocity_for_distance(5_000.0);
    let mut scroll = five_points(SnapSelection::AdjacentOnly, 200.0);
    assert_eq!(
        drag_and_release(&mut scroll, 0.0, hard, 0),
        ReleaseOutcome::SnapsToPoint {
            index: 3,
            offset: 300.0
        }
    );
    assert_eq!(scroll.sample(ms(10_000)).offset, 300.0);
    let mut scroll = five_points(SnapSelection::AdjacentOnly, 200.0);
    assert_eq!(
        drag_and_release(&mut scroll, 0.0, -hard, 0),
        ReleaseOutcome::SnapsToPoint {
            index: 1,
            offset: 100.0
        }
    );
    // Для сравнения: «ближайшая к проекции» летит до края.
    let mut scroll = five_points(SnapSelection::NearestToProjection, 200.0);
    assert_eq!(
        drag_and_release(&mut scroll, 0.0, hard, 0),
        ReleaseOutcome::SnapsToPoint {
            index: 4,
            offset: 400.0
        }
    );
    // Слабое движение остаётся на своей точке.
    let mut scroll = five_points(SnapSelection::AdjacentOnly, 200.0);
    assert_eq!(
        drag_and_release(&mut scroll, 20.0, 0.0, 0),
        ReleaseOutcome::SnapsToPoint {
            index: 2,
            offset: 200.0
        }
    );
}

#[test]
fn system_cancel_goes_to_nearest_valid_position_without_a_throw() {
    // С точками: к ближайшей точке, скорость пальца не участвует.
    let mut scroll = five_points(SnapSelection::NearestToProjection, 0.0);
    scroll.begin_drag(ms(0));
    scroll.drag_to(130.0).unwrap();
    assert_eq!(
        scroll.cancel(ms(5)).unwrap(),
        ReleaseOutcome::SnapsToPoint {
            index: 1,
            offset: 100.0
        }
    );
    assert_eq!(scroll.sample(ms(10_000)).offset, 100.0);
    assert_eq!(
        scroll.drag_to(1.0),
        Err(ScrollError::NotDragging),
        "после отмены перетаскивание закончено"
    );

    // Без точек, внутри границ: остаёмся там, где были.
    let mut scroll = axis(0.0, 1000.0, 100.0);
    scroll.begin_drag(ms(0));
    scroll.drag_to(55.0).unwrap();
    assert_eq!(scroll.cancel(ms(5)).unwrap(), ReleaseOutcome::Settled { offset: 155.0 });
    let rest = scroll.sample(ms(6));
    assert_eq!((rest.offset, rest.phase), (155.0, ScrollPhase::Resting));

    // За краем: возврат к краю.
    let mut scroll = axis(0.0, 1000.0, 0.0);
    scroll.begin_drag(ms(0));
    scroll.drag_to(-200.0).unwrap();
    assert_eq!(
        scroll.cancel(ms(5)).unwrap(),
        ReleaseOutcome::ReturnsFromOverscroll {
            offset: 0.0,
            snap_index: None
        }
    );
    assert_eq!(scroll.sample(ms(5)).phase, ScrollPhase::ReturningFromEdge);
    assert_eq!(scroll.sample(ms(10_000)).offset, 0.0);
}

#[test]
fn calls_outside_a_drag_are_errors_and_change_nothing() {
    let mut scroll = axis(0.0, 1000.0, 250.0);
    assert_eq!(scroll.drag_to(10.0), Err(ScrollError::NotDragging));
    assert_eq!(scroll.release(100.0, ms(0)), Err(ScrollError::NotDragging));
    assert_eq!(scroll.cancel(ms(0)), Err(ScrollError::NotDragging));
    let sample = scroll.sample(ms(1));
    assert_eq!((sample.offset, sample.phase), (250.0, ScrollPhase::Resting));
    scroll.begin_drag(ms(0));
    assert!(matches!(
        scroll.drag_to(f64::NAN),
        Err(ScrollError::NonFiniteInput { .. })
    ));
    assert!(matches!(
        scroll.release(f64::INFINITY, ms(1)),
        Err(ScrollError::NonFiniteInput { .. })
    ));
    assert_eq!(
        scroll.sample(ms(2)).phase,
        ScrollPhase::Dragging,
        "ошибочный release не прервал перетаскивание"
    );
}

#[test]
fn programmatic_scroll_animates_to_clamped_target_or_jumps() {
    let mut scroll = axis(0.0, 1000.0, 0.0);
    scroll.scroll_to(300.0, ScrollTransition::Animated, ms(0)).unwrap();
    let mid = scroll.sample(ms(150));
    assert_eq!(mid.phase, ScrollPhase::ScrollingToTarget);
    assert!(mid.offset > 0.0 && mid.offset < 300.0);
    assert_eq!(scroll.sample(ms(10_000)).offset, 300.0);

    scroll
        .scroll_to(5_000.0, ScrollTransition::Animated, ms(20_000))
        .unwrap();
    assert_eq!(scroll.sample(ms(40_000)).offset, 1000.0, "цель обрезана границей");
    scroll.scroll_to(-7.0, ScrollTransition::Immediate, ms(40_000)).unwrap();
    let jumped = scroll.sample(ms(40_000));
    assert_eq!((jumped.offset, jumped.phase), (0.0, ScrollPhase::Resting));
    assert!(scroll.scroll_to(f64::NAN, ScrollTransition::Animated, ms(0)).is_err());
}

#[test]
fn programmatic_scroll_takes_over_a_fling_keeping_velocity() {
    let mut scroll = axis(-100_000.0, 100_000.0, 0.0);
    drag_and_release(&mut scroll, 0.0, 2_000.0, 0);
    let before = scroll.sample(ms(300));
    scroll
        .scroll_to(before.offset + 50.0, ScrollTransition::Animated, ms(300))
        .unwrap();
    let after = scroll.sample(ms(300));
    assert!((after.offset - before.offset).abs() < 1e-9 && (after.velocity - before.velocity).abs() < 1e-9);
}

#[test]
fn geometry_change_moves_resting_content_back_inside() {
    let mut scroll = axis(0.0, 1000.0, 800.0);
    scroll.set_geometry(geometry(0.0, 500.0), ms(0)).unwrap();
    assert_eq!(scroll.sample(ms(0)).offset, 800.0, "скачка при смене границ нет");
    assert_eq!(scroll.sample(ms(100)).phase, ScrollPhase::ReturningFromEdge);
    assert_eq!(scroll.sample(ms(10_000)).offset, 500.0);
    assert_eq!(scroll.geometry().max_offset, 500.0);
    // Некорректная геометрия отвергается и не меняет состояние.
    assert!(matches!(
        scroll.set_geometry(geometry(10.0, 5.0), ms(0)),
        Err(ScrollError::InvalidBounds { .. })
    ));
    assert_eq!(scroll.geometry().max_offset, 500.0);
}

#[test]
fn geometry_change_during_fling_replans_edge_hit_with_current_velocity() {
    let mut scroll = axis(0.0, 10_000.0, 0.0);
    drag_and_release(&mut scroll, 0.0, 3_000.0, 0);
    let moment = ms(200);
    let before = scroll.sample(moment);
    scroll
        .set_geometry(geometry(0.0, before.offset + 40.0), moment)
        .unwrap();
    let after = scroll.sample(moment);
    assert!((after.offset - before.offset).abs() < 1e-9 && (after.velocity - before.velocity).abs() < 1e-9);
    let end = scroll.sample(ms(10_000));
    assert_eq!(
        end.offset,
        before.offset + 40.0,
        "бросок упёрся в новый край и вернулся на него"
    );
    assert!(
        trajectory(&scroll, 200, 2_000)
            .iter()
            .any(|s| s.offset > before.offset + 40.0 + 1.0)
    );
}

#[test]
fn drag_survives_geometry_change() {
    let mut scroll = axis(0.0, 1000.0, 900.0);
    scroll.begin_drag(ms(0));
    scroll.drag_to(50.0).unwrap();
    scroll.set_geometry(geometry(0.0, 2000.0), ms(1)).unwrap();
    let sample = scroll.sample(ms(2));
    assert_eq!((sample.offset, sample.phase), (950.0, ScrollPhase::Dragging));
}

#[test]
fn snap_points_change_retargets_flight_to_nearest_new_point() {
    let mut scroll = five_points(SnapSelection::NearestToProjection, 0.0);
    drag_and_release(&mut scroll, 0.0, velocity_for_distance(210.0), 0); // летит к 200
    scroll.set_snap_points(&[0.0, 150.0, 400.0], ms(60)).unwrap();
    assert_eq!(scroll.sample(ms(10_000)).offset, 150.0);
    assert!(matches!(
        scroll.set_snap_points(&[1.0, f64::NAN], ms(0)),
        Err(ScrollError::InvalidSnapPoint(_))
    ));
    assert_eq!(scroll.snap_points(), &[0.0, 150.0, 400.0]);
}

#[test]
fn identical_event_traces_give_bitwise_identical_trajectories() {
    let run = || {
        let mut scroll = five_points(SnapSelection::AdjacentOnly, 100.0);
        scroll.begin_drag(ms(0));
        for step in 1..=20 {
            scroll.drag_to(f64::from(step) * 7.3).unwrap();
        }
        scroll.release(900.0, ms(300)).unwrap();
        scroll.begin_drag(ms(420));
        scroll.drag_to(-33.0).unwrap();
        scroll.release(-4_000.0, ms(500)).unwrap();
        trajectory(&scroll, 500, 3_000)
            .iter()
            .flat_map(|s| [s.offset.to_bits(), s.velocity.to_bits()])
            .collect::<Vec<_>>()
    };
    assert_eq!(run(), run());
}

#[test]
fn sampling_rate_does_not_change_the_trajectory() {
    let mut scroll = five_points(SnapSelection::NearestToProjection, 0.0);
    drag_and_release(&mut scroll, 0.0, velocity_for_distance(260.0), 0);
    // sample чистая: выборка в «лишних» моментах не влияет на значения в общих (60 Гц ⊂ 120 Гц).
    let direct = scroll.sample(ms(200));
    for extra in [37, 91, 150, 199] {
        let _ = scroll.sample(ms(extra));
    }
    let again = scroll.sample(ms(200));
    assert_eq!(direct.offset.to_bits(), again.offset.to_bits());
    assert_eq!(direct.velocity.to_bits(), again.velocity.to_bits());
}

#[test]
fn config_validation_and_serde() {
    ScrollConfig::default().validate().unwrap();
    let bad_stop = ScrollConfig {
        stop_velocity: 0.0,
        ..ScrollConfig::default()
    };
    assert!(matches!(bad_stop.validate(), Err(ScrollError::InvalidStopVelocity(_))));
    let bad_rubber = ScrollConfig {
        rubber_band_coefficient: -1.0,
        ..ScrollConfig::default()
    };
    assert!(matches!(
        bad_rubber.validate(),
        Err(ScrollError::InvalidRubberCoefficient(_))
    ));
    let bad_spring = ScrollConfig {
        snap_spring: SpringSpec::DurationBounce {
            duration_s: -1.0,
            bounce: 0.0,
        },
        ..ScrollConfig::default()
    };
    assert!(matches!(bad_spring.validate(), Err(ScrollError::Spring(_))));
    assert!(ScrollAxis::new(&bad_spring, geometry(0.0, 1.0), 0.0).is_err());

    let json = serde_json::to_string(&ScrollConfig::default()).unwrap();
    let back: ScrollConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(back, ScrollConfig::default());
    let partial: ScrollConfig =
        serde_json::from_str(r#"{"deceleration_rate":0.99,"snap_selection":"adjacent_only"}"#).unwrap();
    assert_eq!(partial.deceleration_rate, DecelerationRate::fast());
    assert_eq!(partial.snap_selection, SnapSelection::AdjacentOnly);
    assert!(serde_json::from_str::<ScrollConfig>(r#"{"deceleration_rate":1.0}"#).is_err());
    assert!(serde_json::from_str::<ScrollConfig>(r#"{"unknown":1}"#).is_err());
}

#[test]
fn axis_construction_rejects_bad_geometry_and_initial_offset() {
    let config = ScrollConfig::default();
    assert!(matches!(
        ScrollAxis::new(&config, geometry(5.0, 1.0), 2.0),
        Err(ScrollError::InvalidBounds { .. })
    ));
    assert!(matches!(
        ScrollAxis::new(&config, geometry(0.0, 10.0), 11.0),
        Err(ScrollError::InitialOffsetOutOfBounds { .. })
    ));
    let flat = AxisGeometry {
        min_offset: 0.0,
        max_offset: 10.0,
        viewport_extent: 0.0,
    };
    assert!(matches!(
        ScrollAxis::new(&config, flat, 0.0),
        Err(ScrollError::InvalidViewportExtent(_))
    ));
}

#[test]
fn degenerate_bounds_min_equals_max_work_like_a_wall() {
    let mut scroll = axis(100.0, 100.0, 100.0);
    let outcome = drag_and_release(&mut scroll, 0.0, 2_000.0, 0);
    assert_eq!(outcome, ReleaseOutcome::CoastsPastEdge { edge_offset: 100.0 });
    assert_eq!(scroll.sample(ms(10_000)).offset, 100.0);
}

#[test]
fn release_beyond_the_edge_hands_content_velocity_not_finger_velocity() {
    // Палец отпущен на 200 pt за краем со скоростью 1000 pt/с наружу. Контент за краем едет медленнее
    // пальца в производную «резинки» (эталон — конечная разность формулы из спецификации).
    let mut scroll = axis(0.0, 1000.0, 0.0);
    scroll.begin_drag(ms(0));
    scroll.drag_to(-200.0).unwrap();
    scroll.release(-1000.0, ms(1)).unwrap();
    let step = 1e-3;
    let slope = (rubber_formula(200.0 + step, 960.0, 0.55) - rubber_formula(200.0 - step, 960.0, 0.55)) / (2.0 * step);
    let velocity = scroll.sample(ms(1)).velocity;
    assert!(
        (velocity - (-1000.0 * slope)).abs() < 0.01,
        "{velocity} vs {}",
        -1000.0 * slope
    );
    assert!(slope < 0.5, "за краем контент заметно медленнее пальца");
}

#[test]
fn projection_used_for_snapping_equals_the_real_landing_point() {
    // Одна модель для проекции и траектории: stopping_distance == куда реально приехал бросок.
    for rate in [DecelerationRate::normal(), DecelerationRate::fast()] {
        for velocity in [900.0, -2_500.0, 12_000.0] {
            let config = ScrollConfig {
                deceleration_rate: rate,
                ..ScrollConfig::default()
            };
            let model = DecelerationModel::new(rate, config.stop_velocity).unwrap();
            let mut scroll = axis_with(config, -1e6, 1e6, 0.0);
            drag_and_release(&mut scroll, 0.0, velocity, 0);
            let landed = scroll.sample(ms(100_000)).offset;
            assert!(
                (landed - model.stopping_distance(velocity)).abs() < 1e-9,
                "{velocity}: {landed}"
            );
        }
    }
}

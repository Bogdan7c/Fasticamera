#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Аниматор свойств: прерывание, перелёт, Reduce Motion, мёртвые элементы. Эталон — независимое
//! численное интегрирование уравнения пружины (RK4), а не формулы из `spring`.

mod common;

use common::at_ms;
use ui_core::animator::{
    AnimatedProperty, AnimationRequest, AnimationStart, AnimatorConfig, AnimatorError, PropertyAnimator, PropertyKey,
    SnapOutcome, StartVelocity,
};
use ui_core::geometry::{Insets, Size};
use ui_core::id::NodeId;
use ui_core::spring::Spring;
use ui_core::time::{MonotonicNanos, MotionPreference};
use ui_core::tree::{TreeError, UiTree};

fn tree_with_node() -> (UiTree, NodeId) {
    let mut tree = UiTree::new(Size::new(440.0, 900.0), Insets::ZERO).unwrap();
    let root = tree.root();
    let node = tree.insert_child(root, "node").unwrap();
    (tree, node)
}

fn key(node: NodeId, property: AnimatedProperty) -> PropertyKey {
    PropertyKey { node, property }
}

fn request(
    key: PropertyKey,
    target: f64,
    spring: Spring,
    velocity: StartVelocity,
    now: MonotonicNanos,
) -> AnimationRequest {
    AnimationRequest {
        key,
        target,
        spring,
        velocity,
        now,
    }
}

/// Эталон: RK4 для `x'' = -(k/m)·x - (c/m)·x'` по смещению от цели. Возвращает (смещение, скорость).
fn rk4(spring: &Spring, mut displacement: f64, mut velocity: f64, duration_s: f64) -> (f64, f64) {
    let physical = spring.physical().unwrap();
    let accel = |x: f64, v: f64| (-physical.stiffness * x - physical.damping * v) / physical.mass;
    let steps = (duration_s / 1e-5).round() as usize;
    let dt = duration_s / steps as f64;
    for _ in 0..steps {
        let (k1x, k1v) = (velocity, accel(displacement, velocity));
        let (k2x, k2v) = (
            velocity + 0.5 * dt * k1v,
            accel(displacement + 0.5 * dt * k1x, velocity + 0.5 * dt * k1v),
        );
        let (k3x, k3v) = (
            velocity + 0.5 * dt * k2v,
            accel(displacement + 0.5 * dt * k2x, velocity + 0.5 * dt * k2v),
        );
        let (k4x, k4v) = (velocity + dt * k3v, accel(displacement + dt * k3x, velocity + dt * k3v));
        displacement += dt / 6.0 * (k1x + 2.0 * k2x + 2.0 * k3x + k4x);
        velocity += dt / 6.0 * (k1v + 2.0 * k2v + 2.0 * k3v + k4v);
    }
    (displacement, velocity)
}

fn new_animator(motion: MotionPreference) -> PropertyAnimator {
    PropertyAnimator::new(AnimatorConfig::default(), motion).unwrap()
}

/// Прерывание посреди движения: положение и скорость в момент прерывания совпадают с RK4, а
/// дальнейший путь — траектория RK4 от этих положения и скорости к НОВОЙ цели.
#[test]
fn retarget_midway_keeps_position_and_velocity() {
    let (mut tree, node) = tree_with_node();
    let scale_x = key(node, AnimatedProperty::ScaleX);
    let spring = Spring::snappy();
    let mut animator = new_animator(MotionPreference::Full);

    let started = animator
        .animate(&tree, request(scale_x, 2.0, spring, StartVelocity::Zero, at_ms(0)))
        .unwrap();
    assert_eq!(started, AnimationStart::Started);

    // Эталон первого отрезка: 150 мс от смещения -1 (1,0 → 2,0), скорость 0.
    let (x1, v1) = rk4(&spring, -1.0, 0.0, 0.150);
    let value_at_interrupt = 2.0 + x1;

    let retargeted = animator
        .animate(&tree, request(scale_x, 0.5, spring, StartVelocity::Inherit, at_ms(150)))
        .unwrap();
    assert_eq!(retargeted, AnimationStart::Retargeted);

    // Сразу после прерывания: значение и скорость не скакнули.
    let just_after = animator.sample(scale_x, at_ms(150)).unwrap();
    assert!(
        (just_after.value - value_at_interrupt).abs() < 1e-6,
        "позиция скакнула: {just_after:?}"
    );
    assert!(
        (just_after.velocity - v1).abs() < 1e-5,
        "скорость скакнула: {just_after:?} против {v1}"
    );
    assert!(v1 > 0.5, "к моменту прерывания пружина должна ехать заметно, v1 = {v1}");

    // Второй отрезок: ещё 120 мс к цели 0,5 из унаследованных положения и скорости.
    let report = animator.advance(at_ms(270), &mut tree);
    assert_eq!(report.applied, 1);
    let (x2, _) = rk4(&spring, value_at_interrupt - 0.5, v1, 0.120);
    assert!((tree.scale(node).unwrap().x - (0.5 + x2)).abs() < 1e-5);
    assert_eq!(tree.scale(node).unwrap().y, 1.0, "соседняя ось не тронута");
}

/// Явная скорость (палец) заменяет унаследованную, но положение сохраняется.
#[test]
fn explicit_velocity_replaces_inherited_but_keeps_position() {
    let (tree, node) = tree_with_node();
    let rotation = key(node, AnimatedProperty::Rotation);
    let spring = Spring::smooth();
    let mut animator = new_animator(MotionPreference::Full);
    animator
        .animate(&tree, request(rotation, 1.0, spring, StartVelocity::Zero, at_ms(0)))
        .unwrap();
    let before = animator.sample(rotation, at_ms(100)).unwrap();

    animator
        .animate(
            &tree,
            request(rotation, -1.0, spring, StartVelocity::Explicit(-7.5), at_ms(100)),
        )
        .unwrap();
    let after = animator.sample(rotation, at_ms(100)).unwrap();
    assert!(
        (after.value - before.value).abs() < 1e-12,
        "положение должно сохраниться"
    );
    assert!(
        (after.velocity - (-7.5)).abs() < 1e-9,
        "скорость должна стать явной: {after:?}"
    );

    // Для нового свойства Inherit — это нулевая скорость.
    let scale_y = key(node, AnimatedProperty::ScaleY);
    animator
        .animate(&tree, request(scale_y, 2.0, spring, StartVelocity::Inherit, at_ms(100)))
        .unwrap();
    assert_eq!(animator.sample(scale_y, at_ms(100)).unwrap().velocity, 0.0);
}

/// Перелёт упругой пружины за 1 для прозрачности: в дерево пишется обрезанное значение, дерево
/// ошибок не видит, а сама пружина продолжает физически перелетать (эталон RK4).
#[test]
fn opacity_overshoot_is_clamped_on_apply_but_spring_stays_physical() {
    let (mut tree, node) = tree_with_node();
    let opacity = key(node, AnimatedProperty::Opacity);
    tree.set_opacity(node, 0.0).unwrap();
    let bouncy = Spring::from_duration_bounce(0.5, 0.5).unwrap();
    let mut animator = new_animator(MotionPreference::Full);
    animator
        .animate(&tree, request(opacity, 1.0, bouncy, StartVelocity::Zero, at_ms(0)))
        .unwrap();

    let mut clamped_total = 0;
    let mut rejected_total = 0;
    let mut applied_values = Vec::new();
    for frame in 1..=200 {
        let report = animator.advance(at_ms(frame * 8), &mut tree);
        clamped_total += report.clamped_to_range;
        rejected_total += report.rejected.len();
        applied_values.push(tree.opacity(node).unwrap());
    }
    assert_eq!(rejected_total, 0, "дерево не должно отвергать обрезанные значения");
    assert!(clamped_total > 0, "bounce 0,5 обязан перелететь за 1");
    assert!(applied_values.iter().all(|value| (0.0..=1.0).contains(value)));
    assert!(animator.is_empty(), "после 1,6 с пружина успокоена и снята");
    assert_eq!(
        tree.opacity(node).unwrap().to_bits(),
        1.0_f64.to_bits(),
        "финал — точно цель"
    );

    // Физическая пружина (те же условия заново): перелетает, сверяем с RK4.
    tree.set_opacity(node, 0.0).unwrap();
    let mut probe = new_animator(MotionPreference::Full);
    probe
        .animate(&tree, request(opacity, 1.0, bouncy, StartVelocity::Zero, at_ms(0)))
        .unwrap();
    let overshoot_time_ms = 280;
    let physical = probe.sample(opacity, at_ms(overshoot_time_ms)).unwrap();
    let (displacement, velocity) = rk4(&bouncy, -1.0, 0.0, overshoot_time_ms as f64 / 1000.0);
    assert!(physical.value > 1.0, "без обрезки пружина выше 1: {physical:?}");
    assert!((physical.value - (1.0 + displacement)).abs() < 1e-5);
    assert!(
        (physical.velocity - velocity).abs() < 1e-4,
        "скорость пружины не тронута обрезкой"
    );
    // Применение в этот момент даёт ровно 1,0, а не ошибку дерева.
    let report = probe.advance(at_ms(overshoot_time_ms), &mut tree);
    assert_eq!(report.clamped_to_range, 1);
    assert_eq!(report.rejected.len(), 0);
    assert_eq!(tree.opacity(node).unwrap(), 1.0);
}

/// Наибольшее значение масштаба по кадрам при заданном предпочтении движения.
fn max_scale_over_run(motion: MotionPreference, velocity: StartVelocity) -> (f64, usize) {
    let (mut tree, node) = tree_with_node();
    let scale_x = key(node, AnimatedProperty::ScaleX);
    let bouncy = Spring::from_duration_bounce(0.5, 0.5).unwrap();
    let mut animator = new_animator(motion);
    animator
        .animate(&tree, request(scale_x, 2.0, bouncy, velocity, at_ms(0)))
        .unwrap();
    let mut max_value = f64::MIN;
    let mut held = 0;
    for frame in 1..=250 {
        let report = animator.advance(at_ms(frame * 8), &mut tree);
        held += report.clamped_by_reduced_motion;
        max_value = max_value.max(tree.scale(node).unwrap().x);
    }
    (max_value, held)
}

/// Reduce Motion: bounce убран, перелёта нет. Контроль: в обычном режиме тот же запуск перелетает.
#[test]
fn reduced_motion_never_overshoots_target() {
    let (full_max, _) = max_scale_over_run(MotionPreference::Full, StartVelocity::Zero);
    assert!(
        full_max > 2.05,
        "контроль: обычная bouncy-пружина перелетает, max = {full_max}"
    );
    let (reduced_max, held) = max_scale_over_run(MotionPreference::Reduced, StartVelocity::Zero);
    assert!(reduced_max <= 2.0, "Reduced перелетел: {reduced_max}");
    assert_eq!(
        held, 0,
        "без начальной скорости удерживать нечего: перелёта нет у самой пружины"
    );
}

/// Reduce Motion: даже большая начальная скорость (бросок пальцем) не выносит за цель.
#[test]
fn reduced_motion_holds_value_against_fast_release() {
    let (reduced_max, held) = max_scale_over_run(MotionPreference::Reduced, StartVelocity::Explicit(40.0));
    assert!(reduced_max <= 2.0, "Reduced перелетел из-за скорости: {reduced_max}");
    assert!(held > 0, "бросок обязан упираться в границу цели");
    let (full_max, _) = max_scale_over_run(MotionPreference::Full, StartVelocity::Explicit(40.0));
    assert!(full_max > 2.0, "контроль: без Reduced бросок перелетает");
}

/// Reduced сохраняет длительность: у заменённой пружины то же `duration`, `bounce` = 0.
#[test]
fn reduced_motion_keeps_spring_duration() {
    let (tree, node) = tree_with_node();
    let opacity = key(node, AnimatedProperty::Opacity);
    let mut reduced = new_animator(MotionPreference::Reduced);
    let bouncy = Spring::from_duration_bounce(0.7, 0.4).unwrap();
    reduced
        .animate(&tree, request(opacity, 0.0, bouncy, StartVelocity::Zero, at_ms(0)))
        .unwrap();
    // Критически затухающая пружина (ζ = 1) той же жёсткости: x(t) = x0·(1 + ωt)·e^(−ωt).
    let omega = 2.0 * std::f64::consts::PI / 0.7;
    let t = 0.3;
    let expected = 1.0 * (1.0 + omega * t) * (-omega * t).exp();
    let sample = reduced.sample(opacity, at_ms(300)).unwrap();
    assert!((sample.value - expected).abs() < 1e-9, "{sample:?} против {expected}");
}

/// Мёртвый элемент: анимация снимается со счётчиком, новый элемент в том же месте не страдает.
#[test]
fn dead_node_animation_is_dropped_without_touching_slot_reuser() {
    let (mut tree, victim) = tree_with_node();
    let root = tree.root();
    let mut animator = new_animator(MotionPreference::Full);
    let victim_opacity = key(victim, AnimatedProperty::Opacity);
    animator
        .animate(
            &tree,
            request(victim_opacity, 0.0, Spring::smooth(), StartVelocity::Zero, at_ms(0)),
        )
        .unwrap();

    tree.remove_subtree(victim).unwrap();
    let heir = tree.insert_child(root, "heir").unwrap();
    assert_ne!(heir, victim, "место переиспользовано, но поколение другое");

    let report = animator.advance(at_ms(100), &mut tree);
    assert_eq!(report.dropped_dead_nodes, 1);
    assert_eq!(report.applied, 0);
    assert!(animator.is_empty());
    assert_eq!(
        tree.opacity(heir).unwrap(),
        1.0,
        "анимация мёртвого id не применилась к наследнику"
    );

    // Старт анимации на мёртвом идентификаторе — отказ, ничего не заведено.
    let error = animator
        .animate(
            &tree,
            request(victim_opacity, 0.0, Spring::smooth(), StartVelocity::Zero, at_ms(100)),
        )
        .unwrap_err();
    assert_eq!(error, AnimatorError::Tree(TreeError::UnknownNode(victim)));
    assert!(animator.is_empty());
}

/// `forget` снимает только анимации указанных элементов и сообщает сколько.
#[test]
fn forget_removes_only_listed_nodes() {
    let (mut tree, first) = tree_with_node();
    let root = tree.root();
    let second = tree.insert_child(root, "second").unwrap();
    let mut animator = new_animator(MotionPreference::Full);
    for node in [first, second] {
        for property in [AnimatedProperty::Opacity, AnimatedProperty::Rotation] {
            let target = if property == AnimatedProperty::Opacity {
                0.5
            } else {
                1.0
            };
            animator
                .animate(
                    &tree,
                    request(
                        key(node, property),
                        target,
                        Spring::smooth(),
                        StartVelocity::Zero,
                        at_ms(0),
                    ),
                )
                .unwrap();
        }
    }
    assert_eq!(animator.len(), 4);
    assert_eq!(animator.forget(&[first]), 2);
    assert_eq!(animator.forget(&[first]), 0, "повторное снятие ничего не находит");
    assert!(animator.is_animating(key(second, AnimatedProperty::Opacity)));
    let report = animator.advance(at_ms(50), &mut tree);
    assert_eq!(report.applied, 2);
    assert_eq!(tree.opacity(first).unwrap(), 1.0, "снятая анимация не двигает элемент");
    assert!(tree.opacity(second).unwrap() < 1.0);
}

/// Неверные цели и значения — ошибки, ничего не меняющие.
#[test]
fn invalid_targets_are_errors_that_change_nothing() {
    let (mut tree, node) = tree_with_node();
    let opacity = key(node, AnimatedProperty::Opacity);
    let mut animator = new_animator(MotionPreference::Full);
    for bad_target in [1.5, -0.1, f64::NAN] {
        let error = animator
            .animate(
                &tree,
                request(opacity, bad_target, Spring::smooth(), StartVelocity::Zero, at_ms(0)),
            )
            .unwrap_err();
        assert!(matches!(
            error,
            AnimatorError::TargetOutOfRange {
                property: AnimatedProperty::Opacity,
                ..
            }
        ));
    }
    let zero_scale = key(node, AnimatedProperty::ScaleX);
    assert!(matches!(
        animator.animate(
            &tree,
            request(zero_scale, 0.0, Spring::smooth(), StartVelocity::Zero, at_ms(0))
        ),
        Err(AnimatorError::TargetOutOfRange { .. })
    ));
    assert!(matches!(
        animator.animate(
            &tree,
            request(
                opacity,
                0.5,
                Spring::smooth(),
                StartVelocity::Explicit(f64::INFINITY),
                at_ms(0)
            )
        ),
        Err(AnimatorError::Spring(_))
    ));
    assert!(animator.is_empty());

    // Недопустимое значение `snap` не снимает ехавшую анимацию.
    animator
        .animate(
            &tree,
            request(opacity, 0.2, Spring::smooth(), StartVelocity::Zero, at_ms(0)),
        )
        .unwrap();
    assert!(animator.snap(&mut tree, opacity, 3.0).is_err());
    assert!(animator.is_animating(opacity));
    assert_eq!(tree.opacity(node).unwrap(), 1.0);
}

/// `snap` ставит значение сразу и снимает анимацию; следующий шаг её не перезаписывает.
#[test]
fn snap_applies_immediately_and_cancels_running_animation() {
    let (mut tree, node) = tree_with_node();
    let opacity = key(node, AnimatedProperty::Opacity);
    let mut animator = new_animator(MotionPreference::Full);
    assert_eq!(animator.snap(&mut tree, opacity, 0.4).unwrap(), SnapOutcome::Applied);
    assert_eq!(tree.opacity(node).unwrap(), 0.4);

    animator
        .animate(
            &tree,
            request(opacity, 1.0, Spring::smooth(), StartVelocity::Zero, at_ms(0)),
        )
        .unwrap();
    assert_eq!(
        animator.snap(&mut tree, opacity, 0.9).unwrap(),
        SnapOutcome::AppliedAndCancelledAnimation
    );
    let report = animator.advance(at_ms(200), &mut tree);
    assert_eq!(report.applied, 0);
    assert_eq!(tree.opacity(node).unwrap(), 0.9);
}

/// Цель совпала с текущим значением: успокоится на первом шаге, не оставив анимацию.
#[test]
fn animation_to_current_value_settles_at_once() {
    let (mut tree, node) = tree_with_node();
    let rotation = key(node, AnimatedProperty::Rotation);
    let mut animator = new_animator(MotionPreference::Full);
    animator
        .animate(
            &tree,
            request(rotation, 0.0, Spring::bouncy(), StartVelocity::Zero, at_ms(0)),
        )
        .unwrap();
    let report = animator.advance(at_ms(16), &mut tree);
    assert_eq!((report.applied, report.settled), (1, 1));
    assert!(animator.is_empty());
}

/// До момента старта (событие «из будущего») значение стоит на месте; потом едет.
#[test]
fn animation_waits_for_its_start_time() {
    let (mut tree, node) = tree_with_node();
    let scale_x = key(node, AnimatedProperty::ScaleX);
    let mut animator = new_animator(MotionPreference::Full);
    animator
        .animate(
            &tree,
            request(scale_x, 3.0, Spring::snappy(), StartVelocity::Zero, at_ms(100)),
        )
        .unwrap();
    animator.advance(at_ms(50), &mut tree);
    assert_eq!(tree.scale(node).unwrap().x, 1.0);
    animator.advance(at_ms(160), &mut tree);
    assert!(tree.scale(node).unwrap().x > 1.0);
}

/// Конфиг проверяется: нулевой минимальный масштаб — ошибка.
#[test]
fn invalid_config_is_rejected() {
    let config = AnimatorConfig {
        min_scale: 0.0,
        ..AnimatorConfig::default()
    };
    assert_eq!(config.validate(), Err(AnimatorError::InvalidMinScale(0.0)));
    assert!(PropertyAnimator::new(config, MotionPreference::Full).is_err());
}

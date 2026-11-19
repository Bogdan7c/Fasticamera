#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::field_reassign_with_default
)]
//! События шкалы: тики, крупные упоры, хаптика, магнит, программные изменения, детерминизм.

use shell_contract::haptics::HapticPattern;
use ui_kit::drive::{DragOrigin, GrowthDirection, MotionPhase, ScrollTransition};
use ui_kit::scale::{ScaleError, ScaleEvent, SetValueOutcome, SnapPolicy, SnapScale, TickVisibility, TravelDirection};
use ui_kit::tokens::{HapticSetting, UiKitTokens};
use ui_kit::value::{LinearLimits, QuantizedLinear, ValueError, ValueModel};

mod common;
use common::*;

type Event = ScaleEvent<f64>;

fn ticks(events: &[Event]) -> usize {
    events
        .iter()
        .filter(|event| matches!(event, ScaleEvent::TickCrossed { .. }))
        .count()
}
fn references(events: &[Event]) -> usize {
    events
        .iter()
        .filter(|event| matches!(event, ScaleEvent::ReferenceCrossed { .. }))
        .count()
}
fn haptics(events: &[Event]) -> Vec<HapticPattern> {
    events
        .iter()
        .filter_map(|event| match event {
            ScaleEvent::Haptic { pattern } => Some(*pattern),
            _ => None,
        })
        .collect()
}
fn value_changes(events: &[Event]) -> Vec<f64> {
    events
        .iter()
        .filter_map(|event| match event {
            ScaleEvent::ValueChanged { value } => Some(*value),
            _ => None,
        })
        .collect()
}

fn right_spec() -> ui_kit::scale::ScaleSpec {
    exposure_spec(GrowthDirection::FingerRight, DragOrigin::FromRecognition)
}

/// Медленно ведёт палец на `total_dx` с шагом `step_dx`; возвращает все события.
fn slow_drag(scale: &mut SnapScale<QuantizedLinear>, total_dx: f64, step_dx: f64) -> Vec<Event> {
    scale.drag_began(ms(0), pan_same(0.0)).unwrap();
    let frames = (total_dx.abs() / step_dx).round() as i64;
    for frame in 1..=frames {
        let dx = total_dx.signum() * step_dx * frame as f64;
        let now = ms(frame * 16);
        scale.drag_moved(now, pan_same(dx)).unwrap();
        scale.advance(now);
    }
    scale.drain_events()
}

#[test]
fn slow_pass_fires_exactly_one_event_per_passed_tick_and_one_rigid_click_at_zero() {
    let tokens = UiKitTokens::default();
    // Из -2 в +2: 12 шагов по 1/3 EV, по ходу один проход через ноль.
    let mut scale = exposure_scale(-2.0, right_spec(), &tokens);
    let events = slow_drag(&mut scale, 144.0, 1.0);
    assert_eq!(
        ticks(&events) + references(&events),
        12,
        "число событий = числу пройденных шагов"
    );
    assert_eq!(references(&events), 1, "ноль — крупный упор, ровно один");
    assert_eq!(ticks(&events), 11);
    let patterns = haptics(&events);
    assert_eq!(patterns.iter().filter(|p| **p == HapticPattern::RigidImpact).count(), 1);
    assert_eq!(
        patterns.iter().filter(|p| **p == HapticPattern::SelectionTick).count(),
        11
    );
    // Щелчок «ноля» стоит именно там, где значение становится нулевым.
    let zero_click_index = events
        .iter()
        .position(|event| matches!(event, ScaleEvent::ReferenceCrossed { .. }))
        .unwrap();
    let value_before_click = events[..zero_click_index]
        .iter()
        .rev()
        .find_map(|event| match event {
            ScaleEvent::ValueChanged { value } => Some(*value),
            _ => None,
        })
        .unwrap();
    assert!(
        (value_before_click + 1.0 / 3.0).abs() < 1e-12,
        "до щелчка было -1/3: {value_before_click}"
    );
    // Значения идут по порядку шагов (эталон: деление на 3).
    let changes = value_changes(&events);
    assert_eq!(changes.len(), 12);
    for (index, value) in changes.iter().enumerate() {
        assert!((value - (index as f64 - 5.0) / 3.0).abs() < 1e-12, "{index}: {value}");
    }
    // Палец остановился на 12-м шаге: все события ведут к 12-му положению.
    assert!((scale.value(ms(144 * 16)) - 2.0).abs() < 1e-12);
}

#[test]
fn passing_zero_downwards_also_clicks_rigid_and_reports_direction() {
    let tokens = UiKitTokens::default();
    let mut scale = exposure_scale(1.0, right_spec(), &tokens);
    let events = slow_drag(&mut scale, -72.0, 1.0);
    assert_eq!(ticks(&events) + references(&events), 6);
    assert_eq!(references(&events), 1);
    assert!(
        events
            .iter()
            .filter(|event| matches!(
                event,
                ScaleEvent::TickCrossed { .. } | ScaleEvent::ReferenceCrossed { .. }
            ))
            .all(|event| matches!(
                event,
                ScaleEvent::TickCrossed {
                    direction: TravelDirection::Decreasing
                } | ScaleEvent::ReferenceCrossed {
                    direction: TravelDirection::Decreasing
                }
            ))
    );
}

#[test]
fn a_fast_frame_reports_every_crossing_but_a_single_haptic() {
    let tokens = UiKitTokens::default();
    let mut scale = exposure_scale(0.0, right_spec(), &tokens);
    scale.drag_began(ms(0), pan_same(0.0)).unwrap();
    scale.drag_moved(ms(16), pan_same(60.0)).unwrap(); // сразу 5 шагов
    let events = scale.drain_events();
    assert_eq!(ticks(&events), 5);
    assert_eq!(
        haptics(&events),
        vec![HapticPattern::SelectionTick],
        "один паттерн на кадр"
    );
    assert_eq!(
        value_changes(&events).len(),
        1,
        "значение за кадр сообщено один раз — итоговым"
    );
    assert!((value_changes(&events)[0] - 5.0 / 3.0).abs() < 1e-12);
}

#[test]
fn a_frame_with_a_reference_crossing_plays_the_strongest_haptic() {
    let tokens = UiKitTokens::default();
    let mut scale = exposure_scale(-1.0 / 3.0, right_spec(), &tokens);
    scale.drag_began(ms(0), pan_same(0.0)).unwrap();
    scale.drag_moved(ms(16), pan_same(36.0)).unwrap(); // -1/3 → +2/3: через ноль
    let events = scale.drain_events();
    assert_eq!(haptics(&events), vec![HapticPattern::RigidImpact]);
}

#[test]
fn haptic_events_obey_the_token_table_and_hidden_ticks_do_not_change_mechanics() {
    let mut tokens = UiKitTokens::default();
    tokens.haptics.scale_reference = HapticSetting::Silent;
    tokens.haptics.scale_tick = HapticSetting::Play(HapticPattern::LightImpact);
    let mut spec = right_spec();
    spec.ticks = TickVisibility::Hidden;
    let mut scale = exposure_scale(-1.0 / 3.0, spec, &tokens);
    assert!(scale.marks().is_empty(), "риски не рисуются");
    let events = slow_drag(&mut scale, 36.0, 1.0);
    assert_eq!(
        ticks(&events) + references(&events),
        3,
        "события те же, что у видимой шкалы"
    );
    // Кадр с крупным упором: он «Silent», поэтому вибрации в этом кадре нет вовсе.
    let patterns = haptics(&events);
    assert_eq!(patterns, vec![HapticPattern::LightImpact, HapticPattern::LightImpact]);
    // Видимая шкала с теми же токенами рисок отдаёт 13 штук.
    let visible = exposure_scale(0.0, right_spec(), &tokens);
    assert_eq!(visible.marks().len(), 13);
    assert_eq!(
        visible.marks()[6].offset_points,
        0.0,
        "ноль — в нулевом смещении модели"
    );
}

#[test]
fn scale_that_does_not_move_reports_nothing() {
    let tokens = UiKitTokens::default();
    let mut scale = exposure_scale(0.0, right_spec(), &tokens);
    for frame in 0..30 {
        scale.advance(ms(frame * 16));
    }
    assert!(scale.drain_events().is_empty());
    scale.catch(ms(500));
    scale.release_without_drag(ms(600)).unwrap();
    let events = scale.drain_events();
    assert_eq!(
        events,
        vec![ScaleEvent::Settled { value: 0.0 }],
        "касание без движения: только «успокоилась»"
    );
    assert_eq!(scale.drain_events().len(), 0, "очередь очищается");
}

#[test]
fn dragging_without_began_is_an_explicit_error_and_changes_nothing() {
    let tokens = UiKitTokens::default();
    let mut scale = exposure_scale(0.0, right_spec(), &tokens);
    assert!(matches!(
        scale.drag_moved(ms(0), pan_same(5.0)),
        Err(ScaleError::Scroll(_))
    ));
    assert!(matches!(
        scale.drag_ended(ms(0), horizontal(5.0)),
        Err(ScaleError::Scroll(_))
    ));
    assert_eq!(scale.snapshot(ms(0)).offset_points, 0.0);
    assert!(scale.drain_events().is_empty());
}

#[test]
fn magnet_pulls_a_slow_stop_near_an_anchor_but_leaves_a_far_stop_alone() {
    let tokens = UiKitTokens::default();
    // Близко (10 pt < 14 pt): доезжает до якоря 1× точно.
    let mut near = magnet_scale(1.0, vec![0.5, 1.0, 2.0], 2.0, &tokens);
    near.drag_began(ms(0), pan_same(0.0)).unwrap();
    near.drag_moved(ms(16), pan_same(10.0)).unwrap();
    near.drag_ended(ms(16), horizontal(0.0)).unwrap();
    assert_eq!(near.snapshot(ms(16 + 5_000)).value, 1.0);
    assert_eq!(near.snapshot(ms(16 + 5_000)).offset_points, 0.0);
    // Далеко (30 pt): остановка там, где отпустили; значение — по независимой формуле.
    let mut far = magnet_scale(1.0, vec![0.5, 1.0, 2.0], 2.0, &tokens);
    far.drag_began(ms(0), pan_same(0.0)).unwrap();
    far.drag_moved(ms(16), pan_same(30.0)).unwrap();
    far.drag_ended(ms(16), horizontal(0.0)).unwrap();
    let rested = far.snapshot(ms(16 + 5_000));
    assert!((rested.offset_points - 30.0).abs() < 1e-9);
    assert!((rested.value - 2.0_f64.powf(30.0 / 24.0 / 12.0)).abs() < 1e-12);
    assert_eq!(rested.phase, MotionPhase::Resting);
}

#[test]
fn magnet_catches_a_coasting_flick_that_would_stop_near_an_anchor() {
    let tokens = UiKitTokens::default();
    let mut scale = magnet_scale(1.0, vec![0.5, 1.0, 1.5], 4.0, &tokens);
    scale.drag_began(ms(0), pan_same(0.0)).unwrap();
    scale.drag_moved(ms(16), pan_same(100.0)).unwrap();
    scale.drag_ended(ms(16), horizontal(150.0)).unwrap();
    for frame in 1..=400 {
        scale.advance(ms(16 + frame * 16));
    }
    let rested = scale.snapshot(ms(16 + 400 * 16));
    let anchor_offset = 24.0 * 12.0 * 1.5_f64.log2();
    assert!(
        (rested.offset_points - anchor_offset).abs() < 1e-9,
        "остановка на якоре 1,5: {}",
        rested.offset_points
    );
    assert!((rested.value - 1.5).abs() < 1e-12);
}

#[test]
fn magnet_scale_flicked_far_from_anchors_just_coasts() {
    let tokens = UiKitTokens::default();
    let mut scale = magnet_scale(1.0, vec![0.5, 1.0, 1.5], 4.0, &tokens);
    scale.drag_began(ms(0), pan_same(0.0)).unwrap();
    scale.drag_moved(ms(16), pan_same(60.0)).unwrap();
    scale.drag_ended(ms(16), horizontal(120.0)).unwrap();
    let rested = scale.snapshot(ms(16 + 20_000));
    // Эталон: проекция 60 + 0,499·120 = 119,9 pt; якорей рядом нет (1,5 — на 168).
    // Допуск 0,5 pt: бросок обрывается при скорости ниже порога остановки (0,5 pt/с), это ≈ 0,25 pt.
    let projected = 60.0 + 120.0 * 0.998 / (1.0 - 0.998) / 1000.0;
    assert!(
        (rested.offset_points - projected).abs() < 0.5,
        "остановка {}",
        rested.offset_points
    );
    assert_eq!(rested.phase, MotionPhase::Resting);
}

#[test]
fn magnets_with_a_stepped_model_are_rejected() {
    let tokens = UiKitTokens::default();
    let mut spec = right_spec();
    spec.snap = SnapPolicy::ContinuousWithMagnets;
    let result = SnapScale::new(
        exposure_model(),
        0.0,
        spec,
        &env(&tokens, ui_core::time::MotionPreference::Full),
        ms(0),
    );
    assert!(matches!(result, Err(ScaleError::MagnetsRequireContinuousModel)));
}

#[test]
fn programmatic_animated_reset_clicks_on_the_way_but_never_vibrates() {
    let tokens = UiKitTokens::default();
    let mut scale = exposure_scale(1.0, right_spec(), &tokens);
    let outcome = scale.set_value(0.0, ScrollTransition::Animated, ms(0)).unwrap();
    assert_eq!(outcome, SetValueOutcome::Applied);
    for frame in 1..=300 {
        scale.advance(ms(frame * 16));
    }
    let events = scale.drain_events();
    assert_eq!(
        ticks(&events) + references(&events),
        3,
        "по пути три шага: 1 → 2/3 → 1/3 → 0"
    );
    assert_eq!(references(&events), 1);
    assert!(haptics(&events).is_empty(), "программный сброс не вибрирует");
    assert!(events.contains(&ScaleEvent::Settled { value: 0.0 }));
    assert_eq!(scale.value(ms(300 * 16)), 0.0);
    assert_eq!(scale.snapshot(ms(300 * 16)).offset_points, 0.0);
}

#[test]
fn programmatic_immediate_set_jumps_without_crossing_events() {
    let tokens = UiKitTokens::default();
    let mut scale = exposure_scale(2.0, right_spec(), &tokens);
    scale.set_value(-1.0, ScrollTransition::Immediate, ms(0)).unwrap();
    let events = scale.drain_events();
    assert_eq!(ticks(&events) + references(&events), 0, "прыжок рубежей не считает");
    assert_eq!(value_changes(&events).len(), 1);
    assert!((scale.value(ms(0)) + 1.0).abs() < 1e-12);
    assert_eq!(scale.snapshot(ms(0)).offset_points, -36.0);
    // Дальнейшее движение пальца считается уже от нового положения.
    scale.drag_began(ms(16), pan_same(0.0)).unwrap();
    scale.drag_moved(ms(32), pan_same(12.0)).unwrap();
    assert_eq!(ticks(&scale.drain_events()), 1);
}

#[test]
fn set_value_adjusts_out_of_range_and_rejects_invalid() {
    let tokens = UiKitTokens::default();
    let mut scale = exposure_scale(0.0, right_spec(), &tokens);
    assert_eq!(
        scale.set_value(5.0, ScrollTransition::Immediate, ms(0)).unwrap(),
        SetValueOutcome::Adjusted {
            applied: scale.model().clamp_value(2.0).unwrap()
        }
    );
    assert!((scale.value(ms(0)) - 2.0).abs() < 1e-12);
    let before = scale.snapshot(ms(0));
    let error = scale
        .set_value(f64::NAN, ScrollTransition::Immediate, ms(1))
        .unwrap_err();
    assert!(matches!(error, ScaleError::Value(ValueError::NonFinite(_))));
    assert_eq!(
        scale.snapshot(ms(1)).offset_points,
        before.offset_points,
        "ошибка состояния не меняет"
    );
}

#[test]
fn narrowing_limits_clamps_the_value_at_once_and_moves_the_scale_back_with_a_spring() {
    let tokens = UiKitTokens::default();
    // Режим «Immediate»: сразу.
    let mut immediate = exposure_scale(2.0, right_spec(), &tokens);
    immediate
        .set_limits(LinearLimits { min: -1.0, max: 1.0 }, ScrollTransition::Immediate, ms(0))
        .unwrap();
    assert!((immediate.value(ms(0)) - 1.0).abs() < 1e-12);
    assert_eq!(immediate.snapshot(ms(0)).offset_points, 36.0);
    assert!(
        value_changes(&immediate.drain_events())
            .iter()
            .any(|value| (value - 1.0).abs() < 1e-12)
    );
    assert_eq!(immediate.marks().len(), 7, "шкала перестроена под ±1");

    // Режим «Animated»: значение крайнее сразу, шкала доезжает.
    let mut animated = exposure_scale(2.0, right_spec(), &tokens);
    animated
        .set_limits(LinearLimits { min: -1.0, max: 1.0 }, ScrollTransition::Animated, ms(0))
        .unwrap();
    assert!((animated.value(ms(0)) - 1.0).abs() < 1e-12, "значение — крайнее сразу");
    assert!(
        animated.snapshot(ms(0)).offset_points > 36.0 + 1.0,
        "шкала ещё не доехала"
    );
    assert_eq!(animated.snapshot(ms(0)).phase, MotionPhase::Settling);
    let rested = animated.snapshot(ms(10_000));
    assert!((rested.offset_points - 36.0).abs() < 1e-9);
    assert_eq!(rested.phase, MotionPhase::Resting);

    // Расширение пределов положение значений не двигает.
    animated
        .set_limits(
            LinearLimits { min: -2.0, max: 2.0 },
            ScrollTransition::Immediate,
            ms(11_000),
        )
        .unwrap();
    assert!((animated.snapshot(ms(11_000)).offset_points - 36.0).abs() < 1e-9);
    assert_eq!(animated.marks().len(), 13);
}

#[test]
fn invalid_limits_leave_the_scale_untouched() {
    let tokens = UiKitTokens::default();
    let mut scale = exposure_scale(1.0, right_spec(), &tokens);
    let before = scale.snapshot(ms(0));
    let result = scale.set_limits(LinearLimits { min: 0.1, max: 0.2 }, ScrollTransition::Immediate, ms(0));
    assert!(matches!(
        result,
        Err(ScaleError::Value(ValueError::InvalidLimits { .. }))
    ));
    assert_eq!(scale.snapshot(ms(0)), before);
    assert_eq!(scale.model().limits(), &LinearLimits { min: -2.0, max: 2.0 });
    assert_eq!(scale.marks().len(), 13);
}

#[test]
fn identical_input_gives_bit_identical_samples_and_events() {
    let tokens = UiKitTokens::default();
    let run = || {
        let mut scale = exposure_scale(-1.0, right_spec(), &tokens);
        scale.drag_began(ms(0), pan(6.0, 0.0)).unwrap();
        let mut trace = Vec::new();
        for frame in 1..=20 {
            let now = ms(frame * 8);
            scale.drag_moved(now, pan_same(frame as f64 * 3.7)).unwrap();
            scale.advance(now);
            trace.push(scale.snapshot(now).offset_points.to_bits());
        }
        scale.drag_ended(ms(160), horizontal(310.0)).unwrap();
        for frame in 1..=300 {
            let now = ms(160 + frame * 16);
            scale.advance(now);
            let snapshot = scale.snapshot(now);
            trace.push(snapshot.offset_points.to_bits());
            trace.push(scale.velocity_points_per_second(now).to_bits());
        }
        (trace, scale.drain_events())
    };
    let (first_trace, first_events) = run();
    let (second_trace, second_events) = run();
    assert_eq!(first_trace, second_trace);
    assert_eq!(first_events, second_events);
    // Сценарий содержательный: пройдены тики, были значения, один паттерн хаптики на кадр,
    // и шкала закончила `Settled` на последнем значении.
    assert!(ticks(&first_events) > 0, "тики пройдены: {first_events:?}");
    assert!(
        first_events
            .iter()
            .any(|event| matches!(event, ScaleEvent::ValueChanged { .. })),
        "значение менялось: {first_events:?}"
    );
    assert!(!haptics(&first_events).is_empty(), "палец вибрировал: {first_events:?}");
    let last_value = first_events.iter().rev().find_map(|event| match event {
        ScaleEvent::ValueChanged { value } => Some(*value),
        _ => None,
    });
    assert!(
        matches!(first_events.last(), Some(ScaleEvent::Settled { value }) if Some(*value) == last_value),
        "последним — Settled на последнем значении: {first_events:?}"
    );
}

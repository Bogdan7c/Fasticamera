#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! K7 «Панель-морф»: фазы, прерывание с наследованием скорости, события ровно один раз,
//! совпадение положения на 60 и 120 Гц, Reduce Motion, ошибки описания.

use ui_core::geometry::Rect;
use ui_core::time::{MonotonicNanos, MotionPreference};
use ui_kit::panel_morph::{
    PanelError, PanelEvent, PanelMorph, PanelMorphSpec, PanelPhase, TransitionOutcome, concentric_radius_pt,
};
use ui_kit::tokens::{MotionToken, UiKitTokens};

mod common;
use common::*;

/// Кнопка меню 48 × 48 внизу справа.
const SOURCE: Rect = Rect::from_xywh(358.0, 778.0, 48.0, 48.0);
/// Открытая панель 382 × 416.
const TARGET: Rect = Rect::from_xywh(29.0, 340.0, 382.0, 416.0);
const TARGET_RADIUS_PT: f64 = 40.0;

fn spec() -> PanelMorphSpec {
    PanelMorphSpec::new(SOURCE, TARGET, TARGET_RADIUS_PT)
}

fn panel(spec: PanelMorphSpec, motion: MotionPreference, tokens: &UiKitTokens) -> PanelMorph {
    PanelMorph::new(spec, &env(tokens, motion), ms(0)).unwrap()
}

/// Гонит кадры с шагом `frame_ms` до `until_ms`, собирая события.
fn run(panel: &mut PanelMorph, from_ms: i64, until_ms: i64, frame_ms: f64) -> Vec<PanelEvent> {
    let mut events = Vec::new();
    let mut t = from_ms as f64;
    while t <= until_ms as f64 + 1e-9 {
        panel.advance(MonotonicNanos::from_nanos(1_000_000_000 + (t * 1e6) as i64));
        events.extend(panel.drain_events());
        t += frame_ms;
    }
    events
}

#[test]
fn closed_panel_looks_like_its_source_button() {
    let tokens = UiKitTokens::default();
    let panel = panel(spec(), MotionPreference::Full, &tokens);
    let snapshot = panel.snapshot(ms(0));
    assert_eq!(snapshot.phase, PanelPhase::Closed);
    assert_eq!(snapshot.frame, SOURCE);
    // Источник — круг: радиус равен половине стороны.
    assert_eq!(snapshot.corner_radius_pt, 24.0);
    assert_eq!(snapshot.effect, 0.0);
    assert_eq!(snapshot.content_opacity, 0.0);
    assert_eq!(snapshot.source_opacity, 1.0);
    // Пункты ждут внизу: сдвиг равен токену.
    assert_eq!(snapshot.content_offset_pt, tokens.panel.content_slide_distance_pt);
}

#[test]
fn opening_runs_through_phases_and_reports_opened_exactly_once() {
    let tokens = UiKitTokens::default();
    let mut panel = panel(spec(), MotionPreference::Full, &tokens);
    assert_eq!(panel.open(ms(0)).unwrap(), TransitionOutcome::Started);
    assert_eq!(panel.phase(ms(50)), PanelPhase::Opening);
    let mid = panel.snapshot(ms(150));
    assert!(mid.frame.size.width > SOURCE.size.width && mid.frame.size.width < TARGET.size.width);
    assert!(mid.effect > 0.0 && mid.effect < 1.0);

    let events = run(&mut panel, 0, 3000, 16.0);
    assert_eq!(events, vec![PanelEvent::Opened]);
    // Повторные кадры после покоя событий не дают.
    assert!(run(&mut panel, 3000, 4000, 16.0).is_empty());

    let done = panel.snapshot(ms(3000));
    assert_eq!(done.phase, PanelPhase::Open);
    assert_eq!(done.frame, TARGET);
    assert_eq!(done.corner_radius_pt, TARGET_RADIUS_PT);
    assert_eq!(done.effect, 1.0);
    assert_eq!(done.content_opacity, 1.0);
    assert_eq!(done.content_offset_pt, 0.0);
    assert_eq!(done.source_opacity, 0.0);
}

#[test]
fn content_slides_in_after_the_glass_starts_to_materialise() {
    let tokens = UiKitTokens::default();
    let mut panel = panel(spec(), MotionPreference::Full, &tokens);
    panel.open(ms(0)).unwrap();
    // Раньше порога проявления содержимого его не видно, но стекло уже растёт.
    let early = panel.snapshot(ms(30));
    assert!(early.effect > 0.0);
    assert!(early.progress < tokens.panel.content_reveal_start);
    assert_eq!(early.content_opacity, 0.0);
    // Пункты едут снизу вверх: сдвиг уменьшается со временем.
    let later = panel.snapshot(ms(300));
    assert!(later.content_offset_pt < early.content_offset_pt);
    assert!(later.content_opacity > 0.0);
}

#[test]
fn same_time_gives_the_same_snapshot_at_60_and_120_hz() {
    let tokens = UiKitTokens::default();
    let mut slow = panel(spec(), MotionPreference::Full, &tokens);
    let mut fast = panel(spec(), MotionPreference::Full, &tokens);
    slow.open(ms(0)).unwrap();
    fast.open(ms(0)).unwrap();
    let slow_events = run(&mut slow, 0, 400, 1000.0 / 60.0);
    let fast_events = run(&mut fast, 0, 400, 1000.0 / 120.0);
    for at_ms in [50, 100, 250, 400] {
        assert_eq!(slow.snapshot(ms(at_ms)), fast.snapshot(ms(at_ms)), "на {at_ms} мс");
    }
    assert_eq!(slow_events, fast_events);
}

#[test]
fn closing_during_opening_keeps_the_velocity_and_never_reports_opened() {
    let tokens = UiKitTokens::default();
    let mut panel = panel(spec(), MotionPreference::Full, &tokens);
    panel.open(ms(0)).unwrap();
    let before = panel.snapshot(ms(100)).progress;
    assert_eq!(panel.close(ms(100)).unwrap(), TransitionOutcome::Reversed);
    // Скачка нет: в момент разворота положение то же.
    let at_reversal = panel.snapshot(ms(100)).progress;
    assert!((before - at_reversal).abs() < 1e-12);
    // Скорость унаследована: панель ещё немного растёт, прежде чем свернуться.
    let just_after = panel.snapshot(ms(108)).progress;
    assert!(
        just_after > at_reversal,
        "ожидали инерцию: {at_reversal} -> {just_after}"
    );
    assert_eq!(panel.phase(ms(120)), PanelPhase::Closing);

    let events = run(&mut panel, 100, 3000, 16.0);
    // Открытие прервано: `Opened` не было, закрытие завершено одним событием.
    assert_eq!(events, vec![PanelEvent::Closed]);
    assert_eq!(panel.snapshot(ms(3000)).frame, SOURCE);
}

#[test]
fn repeating_the_same_command_changes_nothing() {
    let tokens = UiKitTokens::default();
    let mut panel = panel(spec(), MotionPreference::Full, &tokens);
    // Закрытая панель на `close` не реагирует и событий не даёт.
    assert_eq!(panel.close(ms(0)).unwrap(), TransitionOutcome::AlreadyThere);
    assert!(run(&mut panel, 0, 500, 16.0).is_empty());
    panel.open(ms(500)).unwrap();
    let before = panel.snapshot(ms(600));
    assert_eq!(panel.open(ms(600)).unwrap(), TransitionOutcome::AlreadyThere);
    // Повторное открытие не перезапускает движение.
    assert_eq!(panel.snapshot(ms(600)), before);
}

#[test]
fn reduce_motion_removes_the_overshoot_of_a_bouncy_spring() {
    let tokens = UiKitTokens::default();
    let bouncy = spec().with_spring(MotionToken::Bouncy);
    let peak = |motion| {
        let mut panel = panel(bouncy, motion, &tokens);
        panel.open(ms(0)).unwrap();
        (0..2000)
            .map(|t| panel.snapshot(ms(t)).progress)
            .fold(f64::MIN, f64::max)
    };
    assert!(peak(MotionPreference::Full) > 1.01, "bouncy обязана перелетать");
    assert!(
        peak(MotionPreference::Reduced) <= 1.0 + 1e-9,
        "Reduce Motion убирает перелёт"
    );
}

#[test]
fn submenu_morph_with_smooth_spring_differs_from_the_sheet_spring() {
    let tokens = UiKitTokens::default();
    let mut sheet = panel(spec(), MotionPreference::Full, &tokens);
    let mut smooth = panel(spec().with_spring(MotionToken::Smooth), MotionPreference::Full, &tokens);
    sheet.open(ms(0)).unwrap();
    smooth.open(ms(0)).unwrap();
    assert_ne!(sheet.snapshot(ms(150)).progress, smooth.snapshot(ms(150)).progress);
}

#[test]
fn degenerate_description_is_an_error_not_a_silent_fix() {
    let tokens = UiKitTokens::default();
    let flat = Rect::from_xywh(0.0, 0.0, 0.0, 10.0);
    let result = PanelMorph::new(
        PanelMorphSpec::new(flat, TARGET, 10.0),
        &env(&tokens, MotionPreference::Full),
        ms(0),
    );
    assert!(matches!(result, Err(PanelError::InvalidFrame { which: "source", .. })));
    let result = PanelMorph::new(
        PanelMorphSpec::new(SOURCE, TARGET, -1.0),
        &env(&tokens, MotionPreference::Full),
        ms(0),
    );
    assert!(matches!(result, Err(PanelError::InvalidRadius(_))));
}

#[test]
fn nested_radius_is_parent_minus_padding_and_never_negative() {
    assert_eq!(concentric_radius_pt(40.0, 8.0), 32.0);
    assert_eq!(concentric_radius_pt(6.0, 8.0), 0.0);
}

#[test]
fn open_and_close_at_the_same_instant_never_report_a_movement() {
    let tokens = UiKitTokens::default();
    let mut panel = panel(spec(), MotionPreference::Full, &tokens);
    assert_eq!(panel.open(ms(100)).unwrap(), TransitionOutcome::Started);
    // Панель не успела сдвинуться ни на шаг: «Closed» о закрытии несуществующего открытия — ложь.
    assert_eq!(panel.close(ms(100)).unwrap(), TransitionOutcome::Reversed);
    assert!(run(&mut panel, 100, 3000, 16.0).is_empty());
    assert_eq!(panel.phase(ms(3000)), PanelPhase::Closed);
}

#[test]
fn closing_after_a_finished_but_unnoticed_opening_still_reports_both_events() {
    let tokens = UiKitTokens::default();
    let mut panel = panel(spec(), MotionPreference::Full, &tokens);
    panel.open(ms(0)).unwrap();
    // Открытие давно закончилось, кадров не было; `close` не должен затереть событие `Opened`.
    assert_eq!(panel.close(ms(5000)).unwrap(), TransitionOutcome::Started);
    assert_eq!(
        run(&mut panel, 5000, 8000, 16.0),
        vec![PanelEvent::Opened, PanelEvent::Closed]
    );
}

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Линейка с мелким шагом значения (ручки вида, решения 234 и 251): шаг значения 1/200, риска и щелчок через 20
//! шагов, 1 pt на шаг (400 pt на весь ход −200…+200). Плотность задана явно (`ScaleDensity::Explicit`), как это
//! делает экран из своего конфига.
//!
//! Проверяется то, что чувствует палец: любое целое ставится ровно; значение догоняет палец с точки касания;
//! щелчки стоят на рисках и на нуле, а не на каждой единице; быстрый взмах не сыплет щелчки каждый кадр;
//! взмах едет дальше медленного ведения; метка значения пресета рисуется и ничего не делает.

use std::num::NonZeroU32;

use ui_core::geometry::Vector;
use ui_core::gesture::PanTranslation;
use ui_core::time::MotionPreference;
use ui_draw::draw_list::DrawItem;
use ui_draw::geometry::PxSize;
use ui_draw::scene_bridge::NodePlacementPx;
use ui_draw::transform::Affine2;
use ui_kit::drive::DragOrigin;
use ui_kit::pan::PanUpdate;
use ui_kit::ruler::{GatedEvent, HapticPacing, RulerControl, RulerFeel, RulerSpec};
use ui_kit::scale::ZeroMagnet;
use ui_kit::tokens::{HapticEvent, PointsPerUnit, ScaleDensity, UiKitTokens};
use ui_kit::value::{Emphasis, LinearLimits, QuantizedLinear, ReferenceValue, TickPitch, ValueModel};
use ui_kit::view::{DimmedRange, PxPerPt, RulerMarkerStyle, RulerStopTickStyle};

mod common;
use common::{env, ms};

/// Шагов значения на 1,0.
const STEPS_PER_ONE: f64 = 200.0;
/// Шагов значения между рисками.
const TICK_EVERY: u32 = 20;
/// pt на шаг значения (явная плотность шкалы).
const POINTS_PER_STEP: f64 = 1.0;
/// Один кадр, мс.
const FRAME_MS: i64 = 16;

fn fine_model() -> QuantizedLinear {
    QuantizedLinear::new(
        1.0 / STEPS_PER_ONE,
        ReferenceValue::At(0.0),
        LinearLimits { min: -1.0, max: 1.0 },
    )
    .unwrap()
    .with_tick_pitch(TickPitch::EveryNSteps(NonZeroU32::new(TICK_EVERY).unwrap()))
}

fn fine_ruler(tokens: &UiKitTokens, feel: RulerFeel, initial: f64) -> RulerControl {
    RulerControl::new(
        RulerSpec {
            model: fine_model(),
            density: ScaleDensity::Explicit(PointsPerUnit::new(POINTS_PER_STEP).unwrap()),
            major_every_steps: NonZeroU32::new(50).unwrap(),
            feel,
        },
        initial,
        &env(tokens, MotionPreference::Full),
        ms(0),
    )
    .unwrap()
}

/// Как у ручек раздела: отсчёт от касания, щелчки с ограничителем частоты.
fn knob_feel() -> RulerFeel {
    RulerFeel {
        drag_origin: DragOrigin::FromTouchDown,
        haptic_pacing: HapticPacing::Throttled,
        zero_magnet: ZeroMagnet::Off,
    }
}

/// Палец сдвинулся на `dx` от касания; до распознавания пана палец прошёл `before_recognition`.
fn finger(dx: f64, before_recognition: f64) -> PanTranslation {
    PanTranslation {
        from_touch_down: Vector::new(dx, 0.0),
        from_recognition: Vector::new(dx - before_recognition, 0.0),
    }
}

fn finger_values(events: &[GatedEvent<f64>]) -> Vec<f64> {
    events
        .iter()
        .filter_map(|event| match event {
            GatedEvent::FingerValue(value) => Some(*value),
            GatedEvent::Haptic(_) => None,
        })
        .collect()
}

/// Прогоняет кадры до покоя и собирает события (время — метки кадров).
fn settle(ruler: &mut RulerControl, from_ms: i64) -> Vec<GatedEvent<f64>> {
    let mut events = Vec::new();
    for frame in 1..=300 {
        events.extend(ruler.advance(ms(from_ms + frame * FRAME_MS)));
    }
    events
}

/// Медленное ведение пальца (по 1 шагу за кадр), остановка и отпускание без скорости; итоговое значение.
fn drag_slowly_and_release(ruler: &mut RulerControl, finger_travel_pt: f64) -> f64 {
    let direction = finger_travel_pt.signum();
    let steps = (finger_travel_pt.abs() / POINTS_PER_STEP).round() as i64;
    ruler
        .apply_pan(
            ms(0),
            PanUpdate::Began {
                translation: finger(0.0, 0.0),
            },
        )
        .unwrap();
    for step in 1..=steps {
        let dx = direction * POINTS_PER_STEP * step as f64;
        ruler
            .apply_pan(
                ms(step * FRAME_MS),
                PanUpdate::Changed {
                    translation: finger(dx, 0.0),
                },
            )
            .unwrap();
        ruler.advance(ms(step * FRAME_MS));
    }
    let end_ms = (steps + 1) * FRAME_MS;
    ruler
        .apply_pan(ms(end_ms), PanUpdate::Ended { velocity: Vector::ZERO })
        .unwrap();
    settle(ruler, end_ms);
    ruler.value(ms(end_ms + 400 * FRAME_MS))
}

// ----- модель: шаг значения и шаг риски разведены --------------------------------------------------------------------

#[test]
fn model_snaps_to_every_step_but_marks_only_ticks_and_zero() {
    let model = fine_model();
    let marks = model.marks();
    assert_eq!(marks.len(), 401, "шаг значения 1/200: 401 положение на ход −200…+200");
    let tick_positions: Vec<i64> = marks
        .iter()
        .filter(|mark| mark.emphasis == Emphasis::Tick)
        .map(|mark| mark.position as i64)
        .collect();
    let expected: Vec<i64> = (-10..=10).filter(|n| *n != 0).map(|n| n * 20).collect();
    assert_eq!(tick_positions, expected, "риски через 20 шагов, ноль — опорная");
    let references: Vec<i64> = marks
        .iter()
        .filter(|mark| mark.emphasis == Emphasis::Reference)
        .map(|mark| mark.position as i64)
        .collect();
    assert_eq!(references, vec![0]);
    assert_eq!(
        marks.iter().filter(|mark| mark.emphasis == Emphasis::Step).count(),
        401 - 20 - 1
    );
}

#[test]
fn default_pitch_keeps_a_tick_on_every_step_for_other_scales() {
    let model = QuantizedLinear::new(0.1, ReferenceValue::At(0.0), LinearLimits { min: -1.0, max: 1.0 }).unwrap();
    assert_eq!(model.tick_pitch(), TickPitch::EveryStep);
    assert!(
        model.marks().iter().all(|mark| mark.emphasis != Emphasis::Step),
        "без шага риски шагов-без-риски нет: прежние шкалы не меняются"
    );
}

// ----- палец ставит любое целое -----------------------------------------------------------------------------------

#[test]
fn finger_sets_any_integer_exactly_in_both_directions() {
    let tokens = UiKitTokens::default();
    for units in [-200_i32, -74, -7, -1, 0, 1, 7, 30, 74, 200] {
        let mut ruler = fine_ruler(&tokens, knob_feel(), 0.0);
        // Палец влево — значение растёт: 1 pt на единицу.
        let value = drag_slowly_and_release(&mut ruler, -f64::from(units) * POINTS_PER_STEP);
        assert!(
            (value - f64::from(units) / STEPS_PER_ONE).abs() < 1e-12,
            "{units}: палец довёл линейку до {value}"
        );
    }
}

#[test]
fn the_last_value_the_finger_reports_is_the_one_under_the_finger() {
    let tokens = UiKitTokens::default();
    let mut ruler = fine_ruler(&tokens, knob_feel(), 0.0);
    ruler
        .apply_pan(
            ms(0),
            PanUpdate::Began {
                translation: finger(0.0, 0.0),
            },
        )
        .unwrap();
    // 30 pt влево = 30 шагов: значение 0,15, ровно.
    ruler
        .apply_pan(
            ms(16),
            PanUpdate::Changed {
                translation: finger(-30.0, 0.0),
            },
        )
        .unwrap();
    let events = ruler.advance(ms(32));
    let values = finger_values(&events);
    assert!(
        values.last().is_some_and(|last| (last - 0.15).abs() < 1e-12),
        "под пальцем +30 (0,15): {values:?}"
    );
}

// ----- начало движения: значение догоняет палец --------------------------------------------------------------------

#[test]
fn value_catches_up_with_the_finger_from_the_touch_point() {
    let tokens = UiKitTokens::default();
    // Палец прошёл 5 pt до распознавания пана и ещё 4 pt после: от касания это 9 pt = 9 шагов.
    let travel_from_touch = -9.0;
    let recognized_after = 5.0;
    let begun = |feel: RulerFeel| {
        let mut ruler = fine_ruler(&tokens, feel, 0.0);
        ruler
            .apply_pan(
                ms(0),
                PanUpdate::Began {
                    translation: finger(travel_from_touch, -recognized_after),
                },
            )
            .unwrap();
        ruler.position_steps(ms(0))
    };
    let caught_up = begun(knob_feel());
    assert!(
        (caught_up - 9.0).abs() < 1e-9,
        "значение сразу на 9 шагах под пальцем, а не теряет ход до распознавания: {caught_up}"
    );
    let legacy = begun(RulerFeel::default());
    assert!(
        (legacy - 4.0).abs() < 1e-9,
        "прежнее поведение (от точки распознавания) не изменилось: {legacy}"
    );
}

// ----- щелчки отдачи -----------------------------------------------------------------------------------------------

fn pattern_of(tokens: &UiKitTokens, event: HapticEvent) -> shell_contract::haptics::HapticPattern {
    tokens.haptics.pattern_for(event).expect("щелчок включён токенами")
}

fn haptics_of(events: &[GatedEvent<f64>]) -> Vec<shell_contract::haptics::HapticPattern> {
    events
        .iter()
        .filter_map(|event| match event {
            GatedEvent::Haptic(pattern) => Some(*pattern),
            GatedEvent::FingerValue(_) => None,
        })
        .collect()
}

/// Ведёт палец по одному шагу за кадр между значениями `from_steps` и `to_steps` (в шагах), собирая щелчки.
fn slide_collecting_haptics(
    ruler: &mut RulerControl,
    from_steps: i64,
    to_steps: i64,
) -> Vec<(i64, shell_contract::haptics::HapticPattern)> {
    ruler
        .apply_pan(
            ms(0),
            PanUpdate::Began {
                translation: finger(0.0, 0.0),
            },
        )
        .unwrap();
    let direction = (to_steps - from_steps).signum();
    let mut played = Vec::new();
    let mut step = from_steps;
    let mut frame = 0;
    while step != to_steps {
        step += direction;
        frame += 1;
        ruler
            .apply_pan(
                ms(frame * 100),
                PanUpdate::Changed {
                    translation: finger(-((step - from_steps) as f64) * POINTS_PER_STEP, 0.0),
                },
            )
            .unwrap();
        for pattern in haptics_of(&ruler.advance(ms(frame * 100))) {
            played.push((step, pattern));
        }
    }
    played
}

#[test]
fn clicks_stand_on_ticks_and_a_stronger_one_on_zero_not_on_every_unit() {
    let tokens = UiKitTokens::default();
    // Начальное значение +50 единиц, палец ведёт линейку вниз до −50 (по шагу за 100 мс: ограничитель не мешает).
    let mut ruler = fine_ruler(&tokens, knob_feel(), 0.25);
    let played = slide_collecting_haptics(&mut ruler, 50, -50);
    let tick = pattern_of(&tokens, HapticEvent::ScaleTick);
    let reference = pattern_of(&tokens, HapticEvent::ScaleReference);
    assert_ne!(tick, reference, "ноль щёлкает другим, более сильным паттерном");
    let on_ticks: Vec<i64> = played
        .iter()
        .filter(|(_, p)| *p == tick)
        .map(|(step, _)| *step)
        .collect();
    let on_zero: Vec<i64> = played
        .iter()
        .filter(|(_, p)| *p == reference)
        .map(|(step, _)| *step)
        .collect();
    assert_eq!(on_ticks, vec![40, 20, -20, -40], "обычные щелчки — только на рисках");
    assert_eq!(on_zero, vec![0], "один сильный щелчок — на нуле");
    assert_eq!(played.len(), 5, "100 единиц пути — 5 щелчков, а не 100: {played:?}");
}

#[test]
fn fast_swipe_does_not_click_every_frame_but_unlimited_does() {
    let tokens = UiKitTokens::default();
    // Быстрый взмах: 20 шагов (20 pt) за кадр в 16 мс = 1250 pt/с, весь ход 400 шагов — 20 кадров; каждый кадр проходит риску.
    let reference = pattern_of(&tokens, HapticEvent::ScaleReference);
    let run = |pacing: HapticPacing| {
        let mut ruler = fine_ruler(
            &tokens,
            RulerFeel {
                drag_origin: DragOrigin::FromTouchDown,
                haptic_pacing: pacing,
                zero_magnet: ZeroMagnet::Off,
            },
            -1.0,
        );
        ruler
            .apply_pan(
                ms(0),
                PanUpdate::Began {
                    translation: finger(0.0, 0.0),
                },
            )
            .unwrap();
        let mut click_times = Vec::new();
        for frame in 1..=20_i64 {
            let dx = -20.0 * frame as f64;
            ruler
                .apply_pan(
                    ms(frame * FRAME_MS),
                    PanUpdate::Changed {
                        translation: finger(dx, 0.0),
                    },
                )
                .unwrap();
            // Щелчок опорного значения ограничитель пропускает всегда: в темп обычных щелчков он не входит.
            for pattern in haptics_of(&ruler.advance(ms(frame * FRAME_MS))) {
                if pattern != reference {
                    click_times.push(frame * FRAME_MS);
                }
            }
        }
        (click_times, ruler.suppressed_haptics())
    };
    let (unlimited, unlimited_suppressed) = run(HapticPacing::Unlimited);
    assert!(
        unlimited.len() >= 17,
        "без ограничителя щелчок почти в каждом кадре: {unlimited:?}"
    );
    assert_eq!(unlimited_suppressed, 0);
    let (throttled, suppressed) = run(HapticPacing::Throttled);
    assert!(!throttled.is_empty(), "щелчки не пропали совсем");
    assert!(
        throttled.windows(2).all(|pair| pair[1] - pair[0] >= 30),
        "обычные щелчки реже, чем раз в 30 мс: {throttled:?}"
    );
    assert!(
        throttled.len() * 2 <= unlimited.len() + 2,
        "{throttled:?} против {unlimited:?}"
    );
    assert!(suppressed > 0, "подавленные щелчки посчитаны");
}

#[test]
fn zero_click_is_never_swallowed_by_the_throttle_but_a_tick_right_after_a_tick_is() {
    let tokens = UiKitTokens::default();
    let tick = pattern_of(&tokens, HapticEvent::ScaleTick);
    let reference = pattern_of(&tokens, HapticEvent::ScaleReference);
    // Старт на +42 шага; кадр 1 уходит на +30 (риска 40), кадр 2 — на −2 (риска 20 и ноль в одном кадре, через 16 мс).
    let mut ruler = fine_ruler(&tokens, knob_feel(), 0.21);
    ruler
        .apply_pan(
            ms(0),
            PanUpdate::Began {
                translation: finger(0.0, 0.0),
            },
        )
        .unwrap();
    let mut by_frame = Vec::new();
    for (frame, steps_from_start) in [(1_i64, -12_i64), (2, -44)] {
        ruler
            .apply_pan(
                ms(frame * FRAME_MS),
                PanUpdate::Changed {
                    translation: finger(-(steps_from_start as f64) * POINTS_PER_STEP, 0.0),
                },
            )
            .unwrap();
        by_frame.push(haptics_of(&ruler.advance(ms(frame * FRAME_MS))));
    }
    assert_eq!(by_frame[0], vec![tick], "первая риска щёлкает");
    assert_eq!(
        by_frame[1],
        vec![reference],
        "через 16 мс: ноль щёлкает сильным (риска 20 в том же кадре слабее и ограничителем глушится в пользу нуля)"
    );
}

// ----- инерция: быстрый взмах едет дальше ------------------------------------------------------------------------

/// Один взмах: палец отпускается со скоростью `velocity_pt_per_s` (влево — значение растёт) сразу после касания.
fn swipe(ruler: &mut RulerControl, velocity_pt_per_s: f64, at_ms: i64) -> f64 {
    ruler
        .apply_pan(
            ms(at_ms),
            PanUpdate::Began {
                translation: finger(0.0, 0.0),
            },
        )
        .unwrap();
    ruler
        .apply_pan(
            ms(at_ms + FRAME_MS),
            PanUpdate::Changed {
                translation: finger(-velocity_pt_per_s * 0.016, 0.0),
            },
        )
        .unwrap();
    ruler
        .apply_pan(
            ms(at_ms + 2 * FRAME_MS),
            PanUpdate::Ended {
                velocity: Vector::new(-velocity_pt_per_s, 0.0),
            },
        )
        .unwrap();
    settle(ruler, at_ms + 2 * FRAME_MS);
    ruler.value(ms(at_ms + 400 * FRAME_MS))
}

#[test]
fn slow_drag_stops_exactly_under_the_finger_and_a_fast_swipe_travels_much_further() {
    let tokens = UiKitTokens::default();
    // Медленное ведение на 100 pt = 100 единиц и остановка: значение ровно 0,5.
    let mut slow = fine_ruler(&tokens, knob_feel(), 0.0);
    let held = drag_slowly_and_release(&mut slow, -100.0);
    assert!((held - 0.5).abs() < 1e-12, "под пальцем +100 (0,5): {held}");
    // Взмах 1200 pt/с: движение продолжается после отпускания и уходит далеко за путь пальца (≈ 19 шагов).
    let mut fast = fine_ruler(&tokens, knob_feel(), 0.0);
    let flung = swipe(&mut fast, 1200.0, 0);
    assert!(
        flung > 0.5,
        "после взмаха линейка уехала намного дальше пути пальца (+19 шагов): {flung}"
    );
    assert!(
        ((flung * STEPS_PER_ONE).round() - flung * STEPS_PER_ONE).abs() < 1e-9,
        "и остановилась на целой единице: {flung}"
    );
}

#[test]
fn one_or_two_fast_swipes_reach_the_edge_from_the_opposite_one() {
    let tokens = UiKitTokens::default();
    for velocity in [1000.0, 1500.0] {
        let mut ruler = fine_ruler(&tokens, knob_feel(), -1.0);
        let mut swipes = 0;
        let mut value = -1.0;
        while swipes < 2 && value < 1.0 - 1e-12 {
            value = swipe(&mut ruler, velocity, swipes * 10_000);
            swipes += 1;
        }
        assert!(
            (value - 1.0).abs() < 1e-12,
            "{velocity} pt/с: от −200 до +200 за {swipes} взмаха(ов) не дошло: {value}"
        );
    }
}

// ----- метка значения пресета -------------------------------------------------------------------------------------

fn placement() -> NodePlacementPx {
    NodePlacementPx {
        size: PxSize::new(1200.0, 120.0),
        transform: Affine2::translate(600.0, 60.0),
        opacity: 1.0,
    }
}

fn marker_style(tokens: &UiKitTokens) -> RulerMarkerStyle {
    RulerMarkerStyle {
        diameter_pt: 4.0,
        gap_pt: 2.0,
        color: tokens.colors.text_primary,
    }
}

#[test]
fn preset_marker_sits_under_the_ticks_at_the_preset_value_and_changes_nothing() {
    let tokens = UiKitTokens::default();
    let ruler = fine_ruler(&tokens, knob_feel(), 0.0);
    let px = PxPerPt::new(3.0).unwrap();
    // Пресет на +30 (0,15) при текущем нуле: метка в 30 pt правее жёлтой отметки (большие значения справа).
    let item = ruler
        .marker_item(&placement(), ms(0), px, &tokens.ruler, (0.15, marker_style(&tokens)))
        .unwrap()
        .expect("метка в пределах видимого");
    let DrawItem::Shape(shape) = item else {
        panic!("метка — фигура");
    };
    let center_x = shape.transform.tx;
    let center_y = shape.transform.ty;
    assert!(
        (center_x - (600.0 + 30.0 * 3.0)).abs() < 1e-3,
        "метка стоит ровно в позиции +30, 30 pt от текущего: {center_x}"
    );
    let ticks_bottom = 60.0 + tokens.ruler.tick_length_pt as f32 * 3.0 / 2.0;
    assert!(
        center_y > ticks_bottom,
        "метка под рядом рисок: {center_y} ниже {ticks_bottom}"
    );
    assert_eq!(ruler.suppressed_echo_events(), 0);
    assert!(
        ruler.position_steps(ms(1000)).abs() < 1e-9,
        "метка линейку не двигает и не магнитит"
    );
    // Метка далеко (значение пресета +200 при текущем нуле, 200 pt) — вне зоны затухания рисок: не рисуется.
    let far = ruler
        .marker_item(&placement(), ms(0), px, &tokens.ruler, (1.0, marker_style(&tokens)))
        .unwrap();
    assert!(far.is_none(), "далёкая метка не торчит одна на пустом месте");
}

#[test]
fn preset_marker_follows_the_ruler_when_the_finger_moves_it() {
    let tokens = UiKitTokens::default();
    let mut ruler = fine_ruler(&tokens, knob_feel(), 0.0);
    let px = PxPerPt::new(3.0).unwrap();
    let before = ruler
        .marker_item(&placement(), ms(0), px, &tokens.ruler, (0.15, marker_style(&tokens)))
        .unwrap();
    ruler
        .apply_pan(
            ms(0),
            PanUpdate::Began {
                translation: finger(-30.0, 0.0),
            },
        )
        .unwrap();
    let after = ruler
        .marker_item(&placement(), ms(0), px, &tokens.ruler, (0.15, marker_style(&tokens)))
        .unwrap()
        .expect("видна");
    let DrawItem::Shape(after_shape) = after else {
        panic!("фигура");
    };
    assert!(
        (after_shape.transform.tx - 600.0).abs() < 1e-3,
        "когда линейка встала на значение пресета, метка под жёлтой отметкой: {}",
        after_shape.transform.tx
    );
    assert!(before.is_some());
}

// ----- упор и тусклый ход за ним (решения 288, 293) ---------------------------------------------------------------

/// Прозрачность фигуры-риски.
fn shape_alpha(item: &DrawItem) -> f32 {
    let DrawItem::Shape(shape) = item else {
        panic!("риска — фигура");
    };
    shape.fill.a
}

fn stop_style(tokens: &UiKitTokens) -> RulerStopTickStyle {
    RulerStopTickStyle {
        thickness_pt: 3.0,
        length_pt: 22.0,
        color: tokens.colors.text_primary,
    }
}

#[test]
fn without_dimmed_ranges_the_ruler_draws_bit_for_bit_the_same_as_before() {
    let tokens = UiKitTokens::default();
    let ruler = fine_ruler(&tokens, knob_feel(), 0.0);
    let px = PxPerPt::new(3.0).unwrap();
    let plain = ruler
        .items(&placement(), ms(0), px, &tokens.ruler, &tokens.colors)
        .unwrap();
    let with_empty_list = ruler
        .items_with_dimmed_ranges(&placement(), ms(0), px, (&tokens.ruler, &tokens.colors), &[])
        .unwrap();
    assert!(!plain.is_empty());
    assert_eq!(
        plain, with_empty_list,
        "Exposure, белое и зум не должны измениться ни на бит"
    );
}

#[test]
fn only_the_ticks_beyond_the_stop_are_dimmed_and_the_current_mark_never_is() {
    let tokens = UiKitTokens::default();
    let ruler = fine_ruler(&tokens, knob_feel(), 0.0);
    let px = PxPerPt::new(3.0).unwrap();
    let plain = ruler
        .items(&placement(), ms(0), px, &tokens.ruler, &tokens.colors)
        .unwrap();
    // Упор плюсовой стороны на +92 (0,46): всё правее тускнеет в 0,4 раза.
    let dimmed = ruler
        .items_with_dimmed_ranges(
            &placement(),
            ms(0),
            px,
            (&tokens.ruler, &tokens.colors),
            &[DimmedRange {
                from: 0.46,
                to: 1.0,
                alpha_factor: 0.4,
            }],
        )
        .unwrap();
    assert_eq!(plain.len(), dimmed.len(), "риски те же, меняется только яркость");
    let mut dimmed_count = 0;
    for (before, after) in plain.iter().zip(&dimmed) {
        let (DrawItem::Shape(before_shape), DrawItem::Shape(_)) = (before, after) else {
            panic!("риски — фигуры");
        };
        let offset_px = before_shape.transform.tx - 600.0;
        let steps = f64::from(offset_px) / 3.0;
        if steps > 92.0 + 1e-3 {
            dimmed_count += 1;
            assert!(
                (shape_alpha(after) - shape_alpha(before) * 0.4).abs() < 1e-6,
                "риска на {steps:+.0} за упором тускнеет"
            );
        } else {
            assert_eq!(
                before, after,
                "риска на {steps:+.0} до упора (и жёлтая отметка) не тронута"
            );
        }
    }
    assert!(dimmed_count >= 1, "в окне линейки есть риски за упором (+100)");
}

#[test]
fn the_stop_tick_stands_at_the_stop_longer_than_a_normal_tick_and_far_stops_are_not_drawn() {
    let tokens = UiKitTokens::default();
    let ruler = fine_ruler(&tokens, knob_feel(), 0.0);
    let px = PxPerPt::new(3.0).unwrap();
    for (value, expected_steps) in [(0.46, 92.0), (-0.46, -92.0)] {
        let item = ruler
            .stop_tick_item(&placement(), ms(0), px, &tokens.ruler, (value, stop_style(&tokens)))
            .unwrap()
            .expect("упор в видимой части");
        let DrawItem::Shape(shape) = item else {
            panic!("упор — фигура");
        };
        assert!(
            (shape.transform.tx - (600.0 + expected_steps as f32 * 3.0)).abs() < 1e-3,
            "упор {value}: на {expected_steps:+} единицах, а не {}",
            shape.transform.tx
        );
        let ui_draw::shape::ShapeGeometry::RoundedRect { size, .. } = shape.geometry else {
            panic!("упор — прямоугольник");
        };
        assert!(
            f64::from(size.height) > tokens.ruler.tick_length_pt * 3.0,
            "упор длиннее обычной риски: {size:?}"
        );
    }
    let far = ruler
        .stop_tick_item(&placement(), ms(0), px, &tokens.ruler, (1.0, stop_style(&tokens)))
        .unwrap();
    assert!(far.is_none(), "упор за пределами видимых рисок не торчит один");
    assert!(ruler.position_steps(ms(1000)).abs() < 1e-9, "упор линейку не двигает");
}

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Линейка-ручка с НЕПРЕРЫВНОЙ моделью (Кельвины по майредам, оттенок ±50): значение от пальца не округляется,
//! после отпускания шкала стоит где остановилась (ни доезда до риски, ни магнита), программная установка
//! показывает значение точно и не даёт намерений, пределы работают «резинкой», смена пределов — программная.

use std::num::NonZeroU32;

use ui_core::geometry::Vector;
use ui_core::time::MotionPreference;
use ui_draw::draw_list::DrawItem;
use ui_draw::geometry::PxSize;
use ui_draw::scene_bridge::NodePlacementPx;
use ui_draw::transform::Affine2;
use ui_kit::pan::PanUpdate;
use ui_kit::ruler::{GatedEvent, RulerControl, RulerFeel, RulerSpec};
use ui_kit::tokens::{ScaleDensity, UiKitTokens};
use ui_kit::value::{ContinuousLinear, LinearLimits, MiredKelvin, RulerAxis};
use ui_kit::view::PxPerPt;

mod common;
use common::{env, ms, pan_same};

/// Диапазон Кельвинов линейки белого.
const KELVIN_LIMITS: LinearLimits = LinearLimits {
    min: 2000.0,
    max: 10000.0,
};
/// Риска каждые 5 майред, крупная — каждая 5-я (25 майред).
const MIRED_TICK_SPACING: f64 = 5.0;
const MAJOR_EVERY: u32 = 5;
/// 2 pt на майред (токен `MiredRuler`).
const POINTS_PER_MIRED: f64 = 2.0;

fn mired_ruler(tokens: &UiKitTokens, initial_kelvin: f64) -> RulerControl<MiredKelvin> {
    let model = MiredKelvin::new(MIRED_TICK_SPACING, KELVIN_LIMITS).unwrap();
    RulerControl::new(
        RulerSpec {
            model,
            density: ScaleDensity::MiredRuler,
            major_every_steps: NonZeroU32::new(MAJOR_EVERY).unwrap(),
            feel: RulerFeel::default(),
        },
        initial_kelvin,
        &env(tokens, MotionPreference::Full),
        ms(0),
    )
    .unwrap()
}

fn tint_ruler(tokens: &UiKitTokens, initial: f64) -> RulerControl<ContinuousLinear> {
    tint_ruler_with_limits(tokens, initial, LinearLimits { min: -50.0, max: 50.0 })
}

fn tint_ruler_with_limits(tokens: &UiKitTokens, initial: f64, limits: LinearLimits) -> RulerControl<ContinuousLinear> {
    let model = ContinuousLinear::new(5.0, limits).unwrap();
    RulerControl::new(
        RulerSpec {
            model,
            density: ScaleDensity::TintRuler,
            major_every_steps: NonZeroU32::new(MAJOR_EVERY).unwrap(),
            feel: RulerFeel::default(),
        },
        initial,
        &env(tokens, MotionPreference::Full),
        ms(0),
    )
    .unwrap()
}

/// Кельвины точки, сдвинутой от `kelvin` на `delta_mired` майред в сторону РОСТА Кельвинов.
fn kelvin_after(kelvin: f64, delta_mired: f64) -> f64 {
    1_000_000.0 / (1_000_000.0 / kelvin - delta_mired)
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

fn haptic_count(events: &[GatedEvent<f64>]) -> usize {
    events
        .iter()
        .filter(|event| matches!(event, GatedEvent::Haptic(_)))
        .count()
}

fn begin(ruler: &mut RulerControl<impl RulerAxis>) {
    ruler
        .apply_pan(
            ms(0),
            PanUpdate::Began {
                translation: pan_same(0.0),
            },
        )
        .unwrap();
}

fn move_finger(ruler: &mut RulerControl<impl RulerAxis>, dx: f64, at_ms: i64) {
    ruler
        .apply_pan(
            ms(at_ms),
            PanUpdate::Changed {
                translation: pan_same(dx),
            },
        )
        .unwrap();
}

fn release(ruler: &mut RulerControl<impl RulerAxis>, velocity: Vector, at_ms: i64) {
    ruler.apply_pan(ms(at_ms), PanUpdate::Ended { velocity }).unwrap();
}

/// Прогоняет кадры (каждые 16 мс), собирает события.
fn run_frames(
    ruler: &mut RulerControl<impl RulerAxis<Value = f64>>,
    from_ms: i64,
    frames: i64,
) -> Vec<GatedEvent<f64>> {
    let mut events = Vec::new();
    for frame in 1..=frames {
        events.extend(ruler.advance(ms(from_ms + frame * 16)));
    }
    events
}

#[test]
fn one_point_of_finger_travel_changes_the_value_by_exactly_the_price_of_one_point() {
    let tokens = UiKitTokens::default();
    let mut ruler = mired_ruler(&tokens, 3140.0);
    begin(&mut ruler);
    // Палец влево на 1 pt: значение растёт на 1/2 майреда (2 pt на майред).
    move_finger(&mut ruler, -1.0, 16);
    let values = finger_values(&ruler.advance(ms(32)));
    let first = *values.first().expect("палец сдвинул шкалу — значение сообщено");
    let expected = kelvin_after(3140.0, 1.0 / POINTS_PER_MIRED);
    assert!(
        (first - expected).abs() < 1e-6,
        "первое значение {first} К, ожидалось {expected} К без округления и скачка"
    );
    assert!(
        (first - 3140.0).abs() < 6.0 && (first - 3140.0) > 1.0,
        "сдвиг ≈ 2,5 К, а не скачок к ближайшей риске/десятке: {first}"
    );
}

#[test]
fn a_slow_release_keeps_the_exact_value_the_finger_left_it_at() {
    let tokens = UiKitTokens::default();
    // 3125 К = ровно 320 майред — стоим на риске; уходим на 0,3 pt (0,15 майреда) от неё: магнит/доезд забрали бы назад.
    let mut ruler = mired_ruler(&tokens, 3125.0);
    begin(&mut ruler);
    move_finger(&mut ruler, -0.3, 16);
    release(&mut ruler, Vector::ZERO, 32);
    let events = run_frames(&mut ruler, 32, 120);
    let expected = kelvin_after(3125.0, 0.15);
    let settled = ruler.value(ms(5000));
    assert!(
        (settled - expected).abs() < 1e-6,
        "после медленного отпускания шкала стоит там же: {settled} К, ожидалось {expected} К"
    );
    assert!((settled - 3125.0).abs() > 0.5, "не притянулась к риске 3125 К");
    let reported = finger_values(&events);
    assert!(
        reported.iter().all(|value| (value - expected).abs() < 1e-6),
        "после отпускания новых значений нет: {reported:?}"
    );
    assert!(!ruler.is_finger_driven(), "после Settled шкала снова у программы");
}

#[test]
fn a_fling_coasts_as_the_finger_continuation_and_stops_off_the_ticks() {
    let tokens = UiKitTokens::default();
    let mut ruler = mired_ruler(&tokens, 3140.0);
    begin(&mut ruler);
    move_finger(&mut ruler, -20.0, 16);
    let mut events = ruler.advance(ms(32));
    let value_at_release = ruler.value(ms(32));
    // Бросок влево: контент продолжает ехать, значение растёт.
    release(&mut ruler, Vector::new(-300.0, 0.0), 32);
    events.extend(run_frames(&mut ruler, 32, 240));
    let reported = finger_values(&events);
    let final_value = ruler.value(ms(10_000));
    assert!(
        final_value > value_at_release + 5.0 && final_value < KELVIN_LIMITS.max,
        "инерция унесла значение дальше, но не в предел: {final_value}"
    );
    assert_eq!(
        reported.last().copied(),
        Some(final_value),
        "последнее намерение — итоговое значение, инерция — продолжение пальца"
    );
    assert_eq!(ruler.suppressed_echo_events(), 0, "инерция не считается эхом");
    let final_mired = 1_000_000.0 / final_value;
    let to_tick =
        (final_mired / MIRED_TICK_SPACING - (final_mired / MIRED_TICK_SPACING).round()).abs() * MIRED_TICK_SPACING;
    assert!(
        to_tick > 1e-3,
        "бросок остановился между рисками, а не на риске: {final_mired} майред"
    );
}

#[test]
fn haptic_ticks_come_per_mark_not_per_unit() {
    let tokens = UiKitTokens::default();
    let mut ruler = mired_ruler(&tokens, 3125.0);
    begin(&mut ruler);
    let mut events = Vec::new();
    // 100 pt = 50 майред = 10 рисок; палец идёт малыми шагами по 1 pt, как живой.
    for step in 1..=100_i64 {
        move_finger(&mut ruler, -(step as f64), step * 8);
        events.extend(ruler.advance(ms(step * 8)));
    }
    let haptics = haptic_count(&events);
    assert_eq!(
        haptics, 10,
        "10 пройденных рисок — 10 тиков вибрации, а не 50 (по майреду)"
    );
}

#[test]
fn program_value_is_shown_exactly_without_intents() {
    let tokens = UiKitTokens::default();
    let mut ruler = mired_ruler(&tokens, 5600.0);
    ruler.show_exact_value(3137.0, 1e-3, ms(0)).unwrap();
    let events = run_frames(&mut ruler, 0, 240);
    assert!(
        finger_values(&events).is_empty(),
        "программа не рождает намерений: {events:?}"
    );
    assert_eq!(haptic_count(&events), 0, "программное движение не вибрирует");
    let shown = ruler.value(ms(10_000));
    assert!(
        (shown - 3137.0).abs() < 1e-6,
        "показано точно 3137 К, не 3140 и не риска: {shown}"
    );
    assert!(ruler.suppressed_echo_events() > 0, "смены значений погашены как эхо");
    assert!(!ruler.is_finger_driven());
    // Повтор того же значения не двигает шкалу и не плодит эхо.
    let echo_before = ruler.suppressed_echo_events();
    ruler.show_exact_value(3137.0, 1e-3, ms(11_000)).unwrap();
    assert!(run_frames(&mut ruler, 11_000, 30).is_empty());
    assert_eq!(ruler.suppressed_echo_events(), echo_before);
}

#[test]
fn program_value_does_not_steal_the_ruler_from_the_finger() {
    let tokens = UiKitTokens::default();
    let mut ruler = mired_ruler(&tokens, 3140.0);
    begin(&mut ruler);
    let before = ruler.position_steps(ms(10));
    ruler.show_exact_value(6500.0, 1e-3, ms(10)).unwrap();
    assert!(ruler.is_finger_driven(), "палец остаётся хозяином");
    assert!(
        (ruler.position_steps(ms(1000)) - before).abs() < 1e-9,
        "программа не сдвинула шкалу из-под пальца"
    );
}

#[test]
fn program_value_outside_limits_is_clamped_and_nonsense_is_an_error() {
    let tokens = UiKitTokens::default();
    let mut ruler = mired_ruler(&tokens, 5600.0);
    ruler.show_exact_value(20_000.0, 1e-3, ms(0)).unwrap();
    run_frames(&mut ruler, 0, 240);
    assert!(
        (ruler.value(ms(10_000)) - 10_000.0).abs() < 1e-6,
        "зажато верхним пределом"
    );
    assert!(
        ruler.show_exact_value(f64::NAN, 1e-3, ms(11_000)).is_err(),
        "не число — ошибка, а не молчаливая подмена"
    );
    assert!(
        ruler.show_exact_value(-5.0, 1e-3, ms(11_000)).is_err(),
        "Кельвины <= 0 — ошибка"
    );
    assert!(
        (ruler.value(ms(12_000)) - 10_000.0).abs() < 1e-6,
        "ошибка шкалу не двигает"
    );
}

#[test]
fn dragging_past_the_limit_stretches_like_a_rubber_band_but_the_value_stays_at_the_limit() {
    let tokens = UiKitTokens::default();
    let mut ruler = mired_ruler(&tokens, 9900.0);
    begin(&mut ruler);
    // 9900 К = 101,01 майред, до 10000 К (100 майред) около 2 pt: тянем далеко за край.
    move_finger(&mut ruler, -200.0, 16);
    let values = finger_values(&ruler.advance(ms(32)));
    assert!(
        values.iter().all(|value| *value <= 10_000.0 + 1e-9),
        "значение не выходит за предел: {values:?}"
    );
    assert!(
        (ruler.value(ms(32)) - 10_000.0).abs() < 1e-6,
        "у края значение — предел"
    );
    release(&mut ruler, Vector::ZERO, 48);
    run_frames(&mut ruler, 48, 120);
    assert!(
        (ruler.value(ms(5000)) - 10_000.0).abs() < 1e-6,
        "после возврата резинки значение — предел"
    );
}

#[test]
fn retargeting_limits_moves_the_ruler_programmatically() {
    let tokens = UiKitTokens::default();
    let mut ruler = mired_ruler(&tokens, 8000.0);
    ruler
        .retarget_limits(
            LinearLimits {
                min: 2500.0,
                max: 6500.0,
            },
            ms(0),
        )
        .unwrap();
    let events = run_frames(&mut ruler, 0, 240);
    assert!(
        finger_values(&events).is_empty(),
        "смена пределов — не палец: {events:?}"
    );
    assert!(
        (ruler.value(ms(10_000)) - 6500.0).abs() < 1e-6,
        "значение зажато новым пределом"
    );
    assert_eq!(
        ruler.limits(),
        &LinearLimits {
            min: 2500.0,
            max: 6500.0
        }
    );
    // Некорректные пределы отвергаются, шкала и пределы остаются прежними.
    assert!(
        ruler
            .retarget_limits(
                LinearLimits {
                    min: 9000.0,
                    max: 100.0
                },
                ms(11_000)
            )
            .is_err()
    );
    assert_eq!(
        ruler.limits(),
        &LinearLimits {
            min: 2500.0,
            max: 6500.0
        }
    );
}

#[test]
fn tint_ruler_follows_the_finger_exactly_in_both_directions() {
    let tokens = UiKitTokens::default();
    let mut ruler = tint_ruler(&tokens, 0.0);
    begin(&mut ruler);
    // 7 pt влево при 2 pt на единицу: +3,5 (не округляется к целому и не к риске 5).
    move_finger(&mut ruler, -7.0, 16);
    let values = finger_values(&ruler.advance(ms(32)));
    assert!(
        (values.last().copied().unwrap() - 3.5).abs() < 1e-9,
        "палец влево на 7 pt — оттенок +3,5: {values:?}"
    );
    release(&mut ruler, Vector::ZERO, 48);
    run_frames(&mut ruler, 48, 120);
    assert!(
        (ruler.value(ms(5000)) - 3.5).abs() < 1e-9,
        "после отпускания стоит на +3,5"
    );
    // Программная установка — точно и без намерений.
    ruler.show_exact_value(-12.3, 1e-6, ms(6000)).unwrap();
    let events = run_frames(&mut ruler, 6000, 240);
    assert!(finger_values(&events).is_empty());
    assert!((ruler.value(ms(20_000)) + 12.3).abs() < 1e-9);
}

/// Нарисованные риски: сколько pt между ними и какие крупные.
fn tick_bars(items: &[DrawItem]) -> Vec<(f32, f32)> {
    let (_, ticks) = items.split_last().expect("последняя — жёлтая отметка");
    ticks
        .iter()
        .map(|item| {
            let DrawItem::Shape(shape) = item else {
                panic!("{item:?}")
            };
            (
                shape.transform.apply_point(ui_draw::geometry::PxPoint::ORIGIN).x,
                shape.fill.a,
            )
        })
        .collect()
}

#[test]
fn ticks_are_ten_points_apart_and_every_fifth_is_brighter() {
    let tokens = UiKitTokens::default();
    // Пределы шире видимой части (±120 pt = ±60 единиц), чтобы затухание, а не край шкалы, ограничивало риски.
    let ruler = tint_ruler_with_limits(
        &tokens,
        0.0,
        LinearLimits {
            min: -100.0,
            max: 100.0,
        },
    );
    let scale = 3.0_f32;
    let center_x = 600.0_f32;
    let placement = NodePlacementPx {
        size: PxSize::new(1200.0, 120.0),
        transform: Affine2::translate(center_x, 60.0),
        opacity: 1.0,
    };
    let items = ruler
        .items(
            &placement,
            ms(0),
            PxPerPt::new(scale).unwrap(),
            &tokens.ruler,
            &tokens.colors,
        )
        .unwrap();
    let ticks = tick_bars(&items);
    // Затухание считается в pt, как у Exposure: видны риски до ±120 pt, при 10 pt на риску это ±12 рисок (25 штук).
    assert_eq!(ticks.len(), 25, "{ticks:?}");
    let first = ticks.first().unwrap().0;
    let last = ticks.last().unwrap().0;
    assert!(
        (center_x - first - 120.0 * scale).abs() < 0.01,
        "крайняя слева на 120 pt: {first}"
    );
    assert!(
        (last - center_x - 120.0 * scale).abs() < 0.01,
        "крайняя справа на 120 pt: {last}"
    );
    assert!(
        (ticks[0].1 - tokens.ruler.minor_tick_alpha * 0.1).abs() < 1e-6,
        "крайняя — 10 % яркости, как у Exposure на ±6 шагов"
    );
    for pair in ticks.windows(2) {
        assert!(
            ((pair[1].0 - pair[0].0).abs() - 10.0 * scale).abs() < 0.01,
            "риски через 10 pt: {pair:?}"
        );
    }
    // Значение 0 стоит в центре: нулевая риска — крупная (каждая 5-я = 25 единиц, нуль кратен), соседи — мелкие.
    let center = ticks
        .iter()
        .position(|(x, _)| (x - center_x).abs() < 0.01)
        .expect("риска под отметкой");
    assert!(
        ticks[center].1 > ticks[center - 1].1 && ticks[center].1 > ticks[center + 1].1,
        "{ticks:?}"
    );
    assert!((ticks[center].1 - tokens.ruler.major_tick_alpha).abs() < 1e-6);
}

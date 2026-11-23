#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Линейка-ручка: палец ведёт значение и отдаёт его наружу; программное движение эха не даёт; значение между
//! ступенями показывается точно и не перепосылается; пределы можно менять на ходу.

use std::num::NonZeroU32;

use ui_core::geometry::Vector;
use ui_core::gesture::PanTranslation;
use ui_core::time::MotionPreference;
use ui_draw::draw_list::DrawItem;
use ui_draw::geometry::PxSize;
use ui_draw::scene_bridge::NodePlacementPx;
use ui_draw::transform::Affine2;
use ui_kit::pan::PanUpdate;
use ui_kit::ruler::{GatedEvent, RulerControl, RulerSpec};
use ui_kit::tokens::{ScaleDensity, UiKitTokens};
use ui_kit::value::{LinearLimits, QuantizedLinear, ReferenceValue};
use ui_kit::view::PxPerPt;

mod common;
use common::{env, ms};

/// Шаг ручки: 0,1 на ступень, диапазон [-1, 1], ноль — опорное значение; плотность — токен ручки (20 pt на шаг).
fn knob_ruler(tokens: &UiKitTokens, initial: f64) -> RulerControl {
    let model = QuantizedLinear::new(0.1, ReferenceValue::At(0.0), LinearLimits { min: -1.0, max: 1.0 }).unwrap();
    RulerControl::new(
        RulerSpec {
            model,
            density: ScaleDensity::KnobRuler,
            major_every_steps: NonZeroU32::new(5).unwrap(),
        },
        initial,
        &env(tokens, MotionPreference::Full),
        ms(0),
    )
    .unwrap()
}

fn finger_pan(dx: f64) -> PanTranslation {
    PanTranslation {
        from_touch_down: Vector::new(dx, 0.0),
        from_recognition: Vector::new(dx, 0.0),
    }
}

/// Прогоняет кадры, пока линейка не успокоится, и собирает все события.
fn settle(ruler: &mut RulerControl, from_ms: i64) -> Vec<GatedEvent<f64>> {
    let mut events = Vec::new();
    for frame in 1..=120 {
        events.extend(ruler.advance(ms(from_ms + frame * 16)));
    }
    events
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

#[test]
fn finger_drag_reports_the_value_the_finger_chose() {
    let tokens = UiKitTokens::default();
    let mut ruler = knob_ruler(&tokens, 0.0);
    ruler
        .apply_pan(
            ms(0),
            PanUpdate::Began {
                translation: finger_pan(0.0),
            },
        )
        .unwrap();
    // Палец влево на 60 pt = три шага по 20 pt: значение растёт до 0,3.
    ruler
        .apply_pan(
            ms(16),
            PanUpdate::Changed {
                translation: finger_pan(-60.0),
            },
        )
        .unwrap();
    let mut events = ruler.advance(ms(32));
    ruler
        .apply_pan(ms(48), PanUpdate::Ended { velocity: Vector::ZERO })
        .unwrap();
    events.extend(settle(&mut ruler, 48));
    let values = finger_values(&events);
    assert!(
        values.last().is_some_and(|last| (last - 0.3).abs() < 1e-9),
        "палец довёл ручку до 0,3: {values:?}"
    );
    assert!(
        (ruler.value(ms(3000)) - 0.3).abs() < 1e-9,
        "после отпускания ручка стоит на ступени 0,3"
    );
    assert_eq!(ruler.suppressed_echo_events(), 0, "эха при ведении пальцем нет");
}

#[test]
fn program_moves_never_produce_finger_values_and_are_counted_as_echo() {
    let tokens = UiKitTokens::default();
    let mut ruler = knob_ruler(&tokens, 0.0);
    ruler.follow_grid_value(0.5, 0.01, ms(0)).unwrap();
    let events = settle(&mut ruler, 0);
    assert!(
        finger_values(&events).is_empty(),
        "программа не ведёт палец: {events:?}"
    );
    assert!((ruler.value(ms(5000)) - 0.5).abs() < 1e-9, "линейка доехала до 0,5");
    assert!(
        ruler.suppressed_echo_events() > 0,
        "смена значений от программы погашена и посчитана"
    );
    assert!(!ruler.is_finger_driven());
}

#[test]
fn exact_value_between_steps_is_shown_as_is_and_not_reported() {
    let tokens = UiKitTokens::default();
    let mut ruler = knob_ruler(&tokens, 0.0);
    // 0,15 при шаге 0,1 — между ступенями: линейка стоит ровно на полутора шагах.
    ruler.follow_exact_value(0.15, 1e-3, ms(0)).unwrap();
    let events = settle(&mut ruler, 0);
    assert!(
        finger_values(&events).is_empty(),
        "значение от программы не уходит наружу"
    );
    let position = ruler.position_steps(ms(5000));
    assert!(
        (position - 1.5).abs() < 1e-6,
        "линейка стоит на 1,5 шага, а не на ступени: {position}"
    );
    // Повторная установка того же значения ничего не двигает: линейка не дёргается каждый кадр.
    let before = ruler.suppressed_echo_events();
    ruler.follow_exact_value(0.15, 1e-3, ms(6000)).unwrap();
    let events = settle(&mut ruler, 6000);
    assert!(events.is_empty(), "стоит на месте, событий нет: {events:?}");
    assert_eq!(ruler.suppressed_echo_events(), before);
}

#[test]
fn follow_does_nothing_while_the_finger_holds_the_ruler() {
    let tokens = UiKitTokens::default();
    let mut ruler = knob_ruler(&tokens, 0.0);
    ruler
        .apply_pan(
            ms(0),
            PanUpdate::Began {
                translation: finger_pan(0.0),
            },
        )
        .unwrap();
    ruler.follow_exact_value(0.7, 1e-3, ms(10)).unwrap();
    ruler.follow_grid_value(0.7, 0.01, ms(10)).unwrap();
    assert!(ruler.is_finger_driven(), "палец остаётся хозяином");
    assert!(
        ruler.position_steps(ms(1000)).abs() < 1e-9,
        "программа не сдвинула линейку из-под пальца"
    );
}

#[test]
fn limits_change_moves_the_ruler_and_hands_it_to_the_program() {
    let tokens = UiKitTokens::default();
    let mut ruler = knob_ruler(&tokens, 0.9);
    ruler
        .retarget_limits(LinearLimits { min: -0.5, max: 0.5 }, ms(0))
        .unwrap();
    let events = settle(&mut ruler, 0);
    assert!(finger_values(&events).is_empty());
    assert!(
        (ruler.value(ms(5000)) - 0.5).abs() < 1e-9,
        "значение зажато новым пределом"
    );
    assert_eq!(ruler.limits(), &LinearLimits { min: -0.5, max: 0.5 });
}

#[test]
fn items_draw_ticks_and_the_yellow_mark_for_the_current_value() {
    let tokens = UiKitTokens::default();
    let ruler = knob_ruler(&tokens, 0.0);
    let placement = NodePlacementPx {
        size: PxSize::new(600.0, 80.0),
        transform: Affine2::translate(300.0, 40.0),
        opacity: 1.0,
    };
    let items = ruler
        .items(
            &placement,
            ms(0),
            PxPerPt::new(3.0).unwrap(),
            &tokens.ruler,
            &tokens.colors,
        )
        .unwrap();
    // Жёлтая отметка всегда последняя; риски вокруг неё симметричны и уже ярче у крупной.
    let DrawItem::Shape(mark) = items.last().unwrap() else {
        panic!("отметка — фигура")
    };
    assert_eq!(mark.fill, tokens.colors.focus_yellow);
    assert!(items.len() > 5, "риски нарисованы: {}", items.len());
}

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Детерминизм: один и тот же поток касаний и кадров на двух свежих сценах даёт побитово
//! одинаковые события и значения свойств.

mod common;

use common::*;
use shell_contract::touch::TouchPhase;
use ui_core::animator::{AnimatedProperty, AnimationRequest, PropertyKey, StartVelocity};
use ui_core::geometry::Rect;
use ui_core::gesture::{GestureData, GestureEvent, GestureState, LongPressSpec, PanSpec, RecognizerSpec, TapSpec};
use ui_core::id::NodeId;
use ui_core::scene::UiScene;
use ui_core::spring::Spring;

/// Прогоняет фиксированный сценарий и возвращает «след»: события (через `Debug`, который
/// печатает f64 без потерь) и биты свойств после каждого кадра.
/// След прогона: события и биты свойств после каждого кадра.
#[derive(Default)]
struct Trace {
    /// События через `Debug` (печатает f64 без потерь): побитовое сравнение двух прогонов.
    event_texts: Vec<String>,
    /// Те же события как данные: по ним проверяется, что сценарий содержательный.
    events: Vec<GestureEvent<&'static str>>,
    property_bits: Vec<u64>,
}

impl Trace {
    fn record_event(&mut self, event: GestureEvent<&'static str>) {
        self.event_texts.push(format!("{event:?}"));
        self.events.push(event);
    }

    /// Сколько раз распознаватель с намерением `intent` перешёл в `state`.
    fn count(&self, intent: &str, state: GestureState) -> usize {
        self.events
            .iter()
            .filter(|event| event.intent == intent && event.state == state)
            .count()
    }

    /// Состояния распознавателя `intent` по порядку событий.
    fn states_of(&self, intent: &str) -> Vec<GestureState> {
        self.events
            .iter()
            .filter(|event| event.intent == intent)
            .map(|event| event.state)
            .collect()
    }

    fn record(&mut self, scene: &mut UiScene<&'static str>, panel: NodeId, button: NodeId) {
        for event in scene.drain_gesture_events() {
            self.record_event(event);
        }
        self.property_bits.push(scene.tree().opacity(panel).unwrap().to_bits());
        self.property_bits.push(scene.tree().scale(panel).unwrap().x.to_bits());
        self.property_bits
            .push(scene.tree().rotation_radians(button).unwrap().to_bits());
    }
}

fn run_scenario() -> Trace {
    let mut scene: UiScene<&'static str> = empty_scene();
    let root = scene.tree().root();
    let panel = add_rect(&mut scene, root, "panel", Rect::from_xywh(0.0, 300.0, 440.0, 400.0));
    let button = add_rect(&mut scene, panel, "button", Rect::from_xywh(20.0, 20.0, 90.0, 90.0));
    scene
        .add_recognizer(panel, RecognizerSpec::Pan(PanSpec::default()), "panel_pan")
        .unwrap();
    scene
        .add_recognizer(panel, RecognizerSpec::LongPress(LongPressSpec::default()), "panel_hold")
        .unwrap();
    scene
        .add_recognizer(button, RecognizerSpec::Tap(TapSpec::default()), "button_tap")
        .unwrap();

    let mut trace = Trace::default();
    let bouncy = Spring::bouncy();

    // Тап по кнопке, затем пан по панели с отпусканием и пружиной со скоростью пальца, поверх —
    // долгий тап в другом месте и анимация вращения кнопки.
    scene.handle_touch(&touch_at_pt(0, TouchPhase::Began, 60.0, 360.0, 0));
    scene.handle_touch(&touch_at_pt(0, TouchPhase::Ended, 60.0, 360.0, 50));
    scene.advance_frame(at_ms(60));
    trace.record(&mut scene, panel, button);

    scene
        .animate(AnimationRequest {
            key: PropertyKey {
                node: button,
                property: AnimatedProperty::Rotation,
            },
            target: 1.3,
            spring: bouncy,
            velocity: StartVelocity::Zero,
            now: at_ms(60),
        })
        .unwrap();

    scene.handle_touch(&touch_at_pt(1, TouchPhase::Began, 300.0, 500.0, 100));
    let mut x = 300.0;
    for step in 1..=12 {
        x += 5.5;
        scene.handle_touch(&touch_at_pt(
            1,
            TouchPhase::Moved,
            x,
            500.0 + step as f64 * 0.7,
            100 + step * 8,
        ));
        scene.advance_frame(at_ms(100 + step * 8));
        trace.record(&mut scene, panel, button);
    }
    scene.handle_touch(&touch_at_pt(1, TouchPhase::Ended, x, 508.4, 204));
    let released = scene.drain_gesture_events();
    for event in &released {
        trace.record_event(event.clone());
        if let (GestureState::Ended, GestureData::Pan { velocity, .. }) = (event.state, event.data) {
            scene
                .animate(AnimationRequest {
                    key: PropertyKey {
                        node: panel,
                        property: AnimatedProperty::ScaleX,
                    },
                    target: 1.0,
                    spring: bouncy,
                    velocity: StartVelocity::Explicit(velocity.dx / 200.0),
                    now: event.time,
                })
                .unwrap();
            scene
                .animate(AnimationRequest {
                    key: PropertyKey {
                        node: panel,
                        property: AnimatedProperty::Opacity,
                    },
                    target: 0.3,
                    spring: bouncy,
                    velocity: StartVelocity::Zero,
                    now: event.time,
                })
                .unwrap();
        }
    }
    // Долгий тап в углу панели: только кадры.
    scene.handle_touch(&touch_at_pt(2, TouchPhase::Began, 400.0, 650.0, 220));
    for frame in 0..90 {
        scene.advance_frame(at_ms(212 + frame * 8));
        trace.record(&mut scene, panel, button);
    }
    scene.handle_touch(&touch_at_pt(2, TouchPhase::Ended, 400.0, 650.0, 940));
    scene.advance_frame(at_ms(960));
    trace.record(&mut scene, panel, button);
    trace
}

#[test]
fn identical_input_gives_bit_identical_events_and_properties() {
    let first = run_scenario();
    let second = run_scenario();
    assert_eq!(first.event_texts, second.event_texts);
    assert_eq!(first.events, second.events);
    assert_eq!(first.property_bits, second.property_bits);
}

/// Сценарий содержательный (иначе равенство прогонов ничего бы не доказывало): проверяется по
/// типизированным событиям — сколько раз и в каком порядке каждый распознаватель менял состояние.
#[test]
fn scenario_is_not_empty_by_typed_events() {
    let trace = run_scenario();
    assert_eq!(trace.count("button_tap", GestureState::Ended), 1);
    assert_eq!(trace.count("panel_pan", GestureState::Ended), 1);
    assert_eq!(trace.count("panel_hold", GestureState::Began), 1);
    // Пан: начался, менялся, закончился — ровно в таком порядке и без отмен. `Failed` от первого
    // касания (тап по кнопке) в цепочку не входит: пан тогда ещё не начинался.
    let pan_states: Vec<GestureState> = trace
        .states_of("panel_pan")
        .into_iter()
        .filter(|state| *state != GestureState::Failed)
        .collect();
    assert_eq!(pan_states.first(), Some(&GestureState::Began));
    assert_eq!(pan_states.last(), Some(&GestureState::Ended));
    assert!(pan_states.contains(&GestureState::Changed));
    assert_eq!(
        pan_states.iter().filter(|state| **state == GestureState::Began).count(),
        1
    );
    assert_eq!(trace.count("panel_pan", GestureState::Cancelled), 0);
    // Тап по кнопке случился раньше начала пана: порядок событий сохранён.
    let position_of = |intent: &str, state: GestureState| {
        trace
            .events
            .iter()
            .position(|event| event.intent == intent && event.state == state)
    };
    assert!(position_of("button_tap", GestureState::Ended) < position_of("panel_pan", GestureState::Began));
    let distinct: std::collections::BTreeSet<_> = trace.property_bits.iter().collect();
    assert!(
        distinct.len() > 20,
        "свойства должны реально меняться: {}",
        distinct.len()
    );
}

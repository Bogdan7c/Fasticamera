#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! «Панель едет за пальцем»: пан ведёт визуальный сдвиг 1:1, на отпускании пружина к 0 стартует
//! со скоростью пальца, новое касание захватывает едущую панель на месте.

mod common;

use common::*;
use shell_contract::touch::TouchPhase;
use ui_core::animator::{AnimatedProperty, AnimationRequest, FreezeOutcome, PropertyKey, StartVelocity};
use ui_core::geometry::Rect;
use ui_core::gesture::{GestureData, GestureEvent, GestureState, PanDirection, PanSpec, RecognizerSpec};
use ui_core::id::NodeId;
use ui_core::scene::UiScene;
use ui_core::spring::Spring;

const PANEL_FRAME: Rect = Rect::from_xywh(20.0, 500.0, 200.0, 100.0);
const FINGER_SPEED_PT_PER_S: f64 = 300.0;
const FINGER_Y_PT: f64 = 550.0;

struct Stand {
    scene: UiScene<&'static str>,
    panel: NodeId,
    key: PropertyKey,
    now_ms: i64,
    finger_x_pt: f64,
}

fn stand() -> Stand {
    let mut scene = empty_scene();
    let root = scene.tree().root();
    let panel = add_rect(&mut scene, root, "panel", PANEL_FRAME);
    scene
        .add_recognizer(
            panel,
            RecognizerSpec::Pan(PanSpec {
                direction: PanDirection::Horizontal,
                ..PanSpec::default()
            }),
            "panel_drag",
        )
        .unwrap();
    Stand {
        scene,
        panel,
        key: PropertyKey {
            node: panel,
            property: AnimatedProperty::TranslationX,
        },
        now_ms: 0,
        finger_x_pt: 100.0,
    }
}

fn pan_event(events: Vec<GestureEvent<&'static str>>, state: GestureState) -> Option<GestureEvent<&'static str>> {
    events.into_iter().find(|event| event.state == state)
}

impl Stand {
    fn finger_down(&mut self) {
        self.scene.handle_touch(&touch_at_pt(
            0,
            TouchPhase::Began,
            self.finger_x_pt,
            FINGER_Y_PT,
            self.now_ms,
        ));
        self.scene.drain_gesture_events();
    }

    /// Один шаг пальца на 8 мс с постоянной скоростью; возвращает событие пана.
    fn finger_step(&mut self) -> Vec<GestureEvent<&'static str>> {
        self.now_ms += 8;
        self.finger_x_pt += FINGER_SPEED_PT_PER_S * 0.008;
        let x_px = (self.finger_x_pt * PIXELS_PER_POINT) as f32;
        self.scene.handle_touch(&touch_px(
            0,
            TouchPhase::Moved,
            x_px,
            (FINGER_Y_PT * PIXELS_PER_POINT) as f32,
            self.now_ms,
        ));
        self.scene.drain_gesture_events()
    }

    fn finger_up(&mut self) -> GestureEvent<&'static str> {
        self.now_ms += 8;
        let x_px = (self.finger_x_pt * PIXELS_PER_POINT) as f32;
        self.scene.handle_touch(&touch_px(
            0,
            TouchPhase::Ended,
            x_px,
            (FINGER_Y_PT * PIXELS_PER_POINT) as f32,
            self.now_ms,
        ));
        pan_event(self.scene.drain_gesture_events(), GestureState::Ended).expect("пан завершён отрывом")
    }

    fn translation_x(&self) -> f64 {
        self.scene.tree().translation(self.panel).unwrap().dx
    }
}

#[test]
fn pan_drives_translation_one_to_one_then_release_springs_back_with_finger_velocity() {
    let mut stand = stand();
    stand.finger_down();

    let mut began_x_pt = None;
    let mut last_velocity = 0.0;
    let mut followed = 0;
    for _ in 0..40 {
        let events = stand.finger_step();
        let finger_seen_pt = f64::from((stand.finger_x_pt * PIXELS_PER_POINT) as f32) / PIXELS_PER_POINT;
        for event in events {
            let GestureData::Pan {
                translation, velocity, ..
            } = event.data
            else {
                continue;
            };
            if event.state == GestureState::Began {
                began_x_pt = Some(finger_seen_pt);
            }
            let origin = began_x_pt.unwrap();
            let offset = translation.from_recognition.dx;
            assert!(
                (offset - (finger_seen_pt - origin)).abs() < 1e-6,
                "1:1 от точки распознавания"
            );
            stand
                .scene
                .set_property(stand.panel, AnimatedProperty::TranslationX, offset)
                .unwrap();
            // Сдвиг виден в дереве точно, а вёрстка (frame) не тронута.
            assert_eq!(stand.translation_x(), offset);
            assert_eq!(stand.scene.tree().frame(stand.panel).unwrap(), PANEL_FRAME);
            last_velocity = velocity.dx;
            followed += 1;
        }
    }
    assert!(followed > 20, "панель вели {followed} кадров");
    assert!((last_velocity - FINGER_SPEED_PT_PER_S).abs() < 0.05 * FINGER_SPEED_PT_PER_S);
    // Панель реально уехала: её видимая рамка сдвинута, касание по новому месту ловит панель.
    let shift = stand.translation_x();
    assert!(shift > 50.0, "сдвиг {shift}");
    let visible = stand.scene.tree().visual_bounds(stand.panel).unwrap();
    assert!((visible.min_x() - (PANEL_FRAME.min_x() + shift)).abs() < 1e-9);
    let inside_new_place = ui_core::geometry::Point::new(visible.max_x() - 5.0, 550.0);
    assert_eq!(stand.scene.tree().hit_path(inside_new_place)[0], stand.panel);

    // Отпускание: пружина к 0 со скоростью пальца, положение не скачет.
    let ended = stand.finger_up();
    let GestureData::Pan { velocity, .. } = ended.data else {
        panic!("данные пана")
    };
    let position_at_release = stand.translation_x();
    stand
        .scene
        .animate(AnimationRequest {
            key: stand.key,
            target: 0.0,
            spring: Spring::interactive(),
            velocity: StartVelocity::Explicit(velocity.dx),
            now: ended.time,
        })
        .unwrap();
    let first = stand.scene.animator().sample(stand.key, ended.time).unwrap();
    assert!(
        (first.velocity - velocity.dx).abs() < 1e-9,
        "первая выборка = скорость пальца: {first:?}"
    );
    assert!((first.value - position_at_release).abs() < 1e-9, "без скачка положения");
    assert!((velocity.dx - FINGER_SPEED_PT_PER_S).abs() < 0.05 * FINGER_SPEED_PT_PER_S);

    // Через 0,2 мс сдвиг изменился по ряду Тейлора до третьего порядка (a, рывок — из параметров пружины).
    let dt_s = 0.0002;
    stand.scene.advance_frame(ui_core::time::MonotonicNanos::from_nanos(
        ended.time.as_nanos() + 200_000,
    ));
    let physical = Spring::interactive().physical().unwrap();
    let acceleration = (-physical.stiffness * position_at_release - physical.damping * velocity.dx) / physical.mass;
    // Рывок (производная ускорения) нужен: у жёсткой пружины третий порядок ≈ 1e-5.
    let jerk = -(physical.stiffness * velocity.dx + physical.damping * acceleration) / physical.mass;
    let expected =
        position_at_release + velocity.dx * dt_s + 0.5 * acceleration * dt_s * dt_s + jerk * dt_s * dt_s * dt_s / 6.0;
    assert!(
        (stand.translation_x() - expected).abs() < 1e-6,
        "{} против {expected}",
        stand.translation_x()
    );
    assert!(
        stand.translation_x() > position_at_release,
        "сначала продолжает движение пальца"
    );

    // Доезжает до 0 точно и анимация снята; frame цел всё время.
    let mut frame_ms = stand.now_ms;
    for _ in 0..300 {
        frame_ms += 8;
        stand.scene.advance_frame(at_ms(frame_ms));
    }
    assert_eq!(stand.translation_x(), 0.0);
    assert!(stand.scene.animator().is_empty());
    assert_eq!(stand.scene.tree().frame(stand.panel).unwrap(), PANEL_FRAME);
}

/// Захват едущей панели новым касанием: на Began экран зовёт `freeze_property` — панель замирает
/// ровно там, где её видно (значение последнего кадра), и дальше не едет.
#[test]
fn touch_down_freezes_moving_panel_where_it_is_seen() {
    let mut stand = stand();
    // Панель едет к 0 из сдвига 150 pt со скоростью.
    stand
        .scene
        .set_property(stand.panel, AnimatedProperty::TranslationX, 150.0)
        .unwrap();
    stand
        .scene
        .animate(AnimationRequest {
            key: stand.key,
            target: 0.0,
            spring: Spring::smooth(),
            velocity: StartVelocity::Explicit(-400.0),
            now: at_ms(0),
        })
        .unwrap();
    stand.scene.advance_frame(at_ms(120));
    let seen_on_screen = stand.translation_x();
    assert!(
        seen_on_screen > 0.0 && seen_on_screen < 150.0,
        "панель в пути: {seen_on_screen}"
    );
    // Касание пришло между кадрами (125 мс): физическая точка пружины уже другая,
    // но палец ложится на то, что видно, — замереть надо там.
    let physical_at_touch = stand.scene.animator().sample(stand.key, at_ms(125)).unwrap().value;
    assert!(
        (physical_at_touch - seen_on_screen).abs() > 1e-3,
        "пружина за 5 мс сдвинулась"
    );

    let outcome = stand
        .scene
        .freeze_property(stand.panel, AnimatedProperty::TranslationX)
        .unwrap();
    assert_eq!(outcome, FreezeOutcome::Frozen { value: seen_on_screen });
    assert!(!stand.scene.animator().is_animating(stand.key));

    // Кадры идут, палец держит — панель стоит на месте побитово.
    for frame in 1..=20 {
        stand.scene.advance_frame(at_ms(120 + frame * 8));
    }
    assert_eq!(stand.translation_x().to_bits(), seen_on_screen.to_bits());

    // Повторный захват неподвижной панели — «не ехала», значение то же.
    let again = stand
        .scene
        .freeze_property(stand.panel, AnimatedProperty::TranslationX)
        .unwrap();
    assert_eq!(again, FreezeOutcome::NotAnimating { value: seen_on_screen });
}

/// Захват удалённого элемента — ошибка, а не «не ехало».
#[test]
fn freezing_removed_element_is_an_error() {
    let mut stand = stand();
    stand.scene.remove_subtree(stand.panel).unwrap();
    assert!(
        stand
            .scene
            .freeze_property(stand.panel, AnimatedProperty::TranslationX)
            .is_err()
    );
}

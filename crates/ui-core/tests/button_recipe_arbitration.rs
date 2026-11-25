#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Рецепт кнопки с долгим нажатием (`Press` + `Tap` + `LongPress` на ОДНОМ узле) на настоящей сцене:
//! касания в пикселях → хит-тест → арена. Кнопка стоит поверх видоискателя, у которого свои `Tap` и
//! `LongPress` (AE/AF Lock). Проверяется то, что видит экран: сколько раз кнопка сработала, сколько раз
//! началось долгое нажатие и молчит ли родитель.
//!
//! «Сработала» — это `Press:Ended` с пальцем внутри рамки кнопки: ровно так решает `ui-kit::PressTracker`
//! (событие `Tap` кнопка не слушает, оно нужно арбитражу).

mod common;

use common::*;
use shell_contract::touch::TouchPhase;
use ui_core::geometry::Rect;
use ui_core::gesture::{GestureData, GestureEvent, GestureState, RecognizerSpec};
use ui_core::scene::UiScene;

const VIEWFINDER: Rect = Rect::from_xywh(0.0, 0.0, 440.0, 700.0);
const BUTTON: Rect = Rect::from_xywh(100.0, 100.0, 60.0, 44.0);
/// Центр кнопки.
const INSIDE: (f64, f64) = (130.0, 122.0);
/// Допуск долгого нажатия по умолчанию — 10 pt: 40 pt вправо уводит палец за допуск, но ещё над видоискателем.
const FAR_OUTSIDE: (f64, f64) = (170.0, 122.0);

/// Видоискатель с `Tap` и `LongPress`; на нём кнопка. `button_has_long_press` — выбор рецепта.
fn scene(button_has_long_press: bool) -> UiScene<&'static str> {
    let mut scene = empty_scene();
    let root = scene.tree().root();
    let viewfinder = add_rect(&mut scene, root, "viewfinder", VIEWFINDER);
    scene
        .add_recognizer(viewfinder, RecognizerSpec::Tap(Default::default()), "viewfinder_tap")
        .unwrap();
    scene
        .add_recognizer(
            viewfinder,
            RecognizerSpec::LongPress(Default::default()),
            "viewfinder_long_press",
        )
        .unwrap();
    let button = add_rect(&mut scene, viewfinder, "button", BUTTON);
    scene
        .add_recognizer(button, RecognizerSpec::Press(Default::default()), "button_press")
        .unwrap();
    scene
        .add_recognizer(button, RecognizerSpec::Tap(Default::default()), "button_tap")
        .unwrap();
    if button_has_long_press {
        scene
            .add_recognizer(
                button,
                RecognizerSpec::LongPress(Default::default()),
                "button_long_press",
            )
            .unwrap();
    }
    scene
}

/// Всё, что произошло за жест, в порядке возникновения.
#[derive(Default)]
struct Gesture {
    events: Vec<GestureEvent<&'static str>>,
}

impl Gesture {
    fn touch(&mut self, scene: &mut UiScene<&'static str>, phase: TouchPhase, at: (f64, f64), millis: i64) {
        scene.handle_touch(&touch_at_pt(0, phase, at.0, at.1, millis));
        self.events.extend(scene.drain_gesture_events());
    }

    fn frame(&mut self, scene: &mut UiScene<&'static str>, millis: i64) {
        scene.advance_frame(at_ms(millis));
        self.events.extend(scene.drain_gesture_events());
    }

    fn count(&self, intent: &str, state: GestureState) -> usize {
        self.events
            .iter()
            .filter(|event| event.intent == intent && event.state == state)
            .count()
    }

    /// Сколько раз кнопка «сработала»: нажатие закончилось отпусканием внутри рамки.
    fn activations(&self) -> usize {
        self.events
            .iter()
            .filter(|event| event.intent == "button_press" && event.state == GestureState::Ended)
            .filter(|event| match event.data {
                GestureData::Press { position } => BUTTON.contains(position),
                _ => false,
            })
            .count()
    }

    /// Сколько раз началось долгое нажатие (кнопки или видоискателя).
    fn long_presses_began(&self) -> usize {
        self.count("button_long_press", GestureState::Began) + self.count("viewfinder_long_press", GestureState::Began)
    }

    /// Родитель (видоискатель) не получил ни тапа, ни долгого нажатия.
    fn viewfinder_stayed_silent(&self) -> bool {
        self.count("viewfinder_tap", GestureState::Ended) == 0
            && self.count("viewfinder_long_press", GestureState::Began) == 0
    }
}

/// Короткое касание: кнопка срабатывает ровно один раз, долгого нажатия нет, видоискатель молчит.
#[test]
fn a_short_touch_activates_the_button_once_without_a_long_press() {
    let mut scene = scene(true);
    let mut gesture = Gesture::default();
    gesture.touch(&mut scene, TouchPhase::Began, INSIDE, 0);
    gesture.touch(&mut scene, TouchPhase::Ended, INSIDE, 80);
    // Таймер долгого нажатия после отрыва ничего не поднимает.
    gesture.frame(&mut scene, 700);
    assert_eq!(gesture.activations(), 1, "кнопка сработала один раз");
    assert_eq!(
        gesture.long_presses_began(),
        0,
        "долгого нажатия нет ни у кнопки, ни у видоискателя"
    );
    assert_eq!(
        gesture.count("button_tap", GestureState::Ended),
        1,
        "тап кнопки выиграл арбитраж"
    );
    assert!(gesture.viewfinder_stayed_silent(), "видоискатель не получил фокус-тап");
}

/// Удержание 0,5 с: долгое нажатие начинается ровно один раз, `Press` отменён, отпускание кнопку не срабатывает.
#[test]
fn holding_for_half_a_second_begins_one_long_press_and_cancels_the_press() {
    let mut scene = scene(true);
    let mut gesture = Gesture::default();
    gesture.touch(&mut scene, TouchPhase::Began, INSIDE, 0);
    gesture.frame(&mut scene, 400);
    assert_eq!(gesture.long_presses_began(), 0, "до 0,5 с долгого нажатия ещё нет");
    gesture.frame(&mut scene, 520);
    // Ещё кадры при удержании не должны начинать его второй раз.
    gesture.frame(&mut scene, 700);
    gesture.touch(&mut scene, TouchPhase::Ended, INSIDE, 900);
    assert_eq!(
        gesture.count("button_long_press", GestureState::Began),
        1,
        "долгое нажатие началось один раз"
    );
    assert_eq!(
        gesture.count("button_press", GestureState::Cancelled),
        1,
        "нажатие отменено"
    );
    assert_eq!(
        gesture.activations(),
        0,
        "отпускание после долгого нажатия не активирует кнопку"
    );
    assert_eq!(gesture.count("button_tap", GestureState::Ended), 0, "тапа нет");
    assert!(
        gesture.viewfinder_stayed_silent(),
        "родитель не получил свой LongPress (AE/AF Lock)"
    );
}

/// Палец ушёл за допуск до 0,5 с: ни срабатывания, ни долгого нажатия; позже по таймеру ничего не оживает.
#[test]
fn leaving_the_tolerance_before_half_a_second_gives_neither_activation_nor_long_press() {
    let mut scene = scene(true);
    let mut gesture = Gesture::default();
    gesture.touch(&mut scene, TouchPhase::Began, INSIDE, 0);
    gesture.touch(&mut scene, TouchPhase::Moved, FAR_OUTSIDE, 200);
    gesture.frame(&mut scene, 700);
    gesture.touch(&mut scene, TouchPhase::Ended, FAR_OUTSIDE, 800);
    assert_eq!(
        gesture.activations(),
        0,
        "палец отпущен вне рамки — кнопка не срабатывает"
    );
    assert_eq!(gesture.long_presses_began(), 0, "долгое нажатие не началось");
    assert_eq!(gesture.count("button_tap", GestureState::Ended), 0, "тапа тоже нет");
    assert!(gesture.viewfinder_stayed_silent(), "видоискатель молчит");
}

/// Почему в рецепте у кнопки СВОЙ `LongPress`: без него удержание на кнопке достаётся видоискателю
/// (AE/AF Lock сработал бы под кнопкой). Тест закрепляет это свойство арены, чтобы рецепт не «упростили».
#[test]
fn without_its_own_long_press_the_button_would_let_the_viewfinder_lock_focus() {
    let mut scene = scene(false);
    let mut gesture = Gesture::default();
    gesture.touch(&mut scene, TouchPhase::Began, INSIDE, 0);
    gesture.frame(&mut scene, 520);
    gesture.touch(&mut scene, TouchPhase::Ended, INSIDE, 900);
    assert_eq!(
        gesture.count("viewfinder_long_press", GestureState::Began),
        1,
        "без LongPress на кнопке родитель получает удержание"
    );
}

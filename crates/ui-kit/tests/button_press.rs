#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! K2 «Кнопка»: механика нажатия. Подсветка при касании, срабатывание по отпусканию, снятие при
//! уходе пальца и возврат; рецепт «кнопка = Tap + Press» и его ловушка — на настоящей сцене.

use shell_contract::touch::TouchPhase;
use ui_core::geometry::{Point, Rect};
use ui_core::gesture::{GestureData, GestureEvent, GestureState, PressSpec, RecognizerSpec, TapSpec};
use ui_core::id::NodeId;
use ui_core::scene::UiScene;
use ui_kit::button::{ButtonError, ButtonState, PressOutcome, PressTracker};
use ui_kit::tokens::UiKitTokens;

mod common;
use common::*;

/// Рамка кнопки: 100 × 50 pt. Отступ зоны отслеживания по умолчанию — 8 pt (токен).
const BUTTON: Rect = Rect::from_xywh(100.0, 600.0, 100.0, 50.0);

fn tracker() -> PressTracker {
    PressTracker::new(BUTTON, &UiKitTokens::default()).unwrap()
}

fn press(position: Point) -> GestureData {
    GestureData::Press { position }
}

fn inside() -> Point {
    Point::new(150.0, 625.0)
}

#[test]
fn highlight_turns_on_at_touch_down_and_activation_comes_only_on_release_inside() {
    let mut button = tracker();
    assert_eq!(button.state(), ButtonState::Normal);
    // Касание: подсветка сразу, срабатывания нет.
    assert_eq!(
        button.handle_press(GestureState::Began, press(inside())).unwrap(),
        PressOutcome::StateChanged(ButtonState::Pressed)
    );
    assert_eq!(button.state(), ButtonState::Pressed);
    // Палец ёрзает внутри: ничего нового (не перерисовывать).
    assert_eq!(
        button
            .handle_press(GestureState::Changed, press(Point::new(160.0, 630.0)))
            .unwrap(),
        PressOutcome::Unchanged
    );
    // Отпускание внутри — срабатывание, подсветка снята.
    assert_eq!(
        button.handle_press(GestureState::Ended, press(inside())).unwrap(),
        PressOutcome::Activated
    );
    assert_eq!(button.state(), ButtonState::Normal);
}

#[test]
fn sliding_out_clears_the_highlight_and_coming_back_restores_it() {
    let mut button = tracker();
    button.handle_press(GestureState::Began, press(inside())).unwrap();
    // 30 pt ниже нижнего края (650): за зоной (650 + 8).
    let outside = Point::new(150.0, 680.0);
    assert_eq!(
        button.handle_press(GestureState::Changed, press(outside)).unwrap(),
        PressOutcome::StateChanged(ButtonState::Normal)
    );
    // Пока палец снаружи — повторов нет.
    assert_eq!(
        button
            .handle_press(GestureState::Changed, press(Point::new(150.0, 700.0)))
            .unwrap(),
        PressOutcome::Unchanged
    );
    // Вернулся — подсветка снова, и отпускание срабатывает.
    assert_eq!(
        button.handle_press(GestureState::Changed, press(inside())).unwrap(),
        PressOutcome::StateChanged(ButtonState::Pressed)
    );
    assert_eq!(
        button.handle_press(GestureState::Ended, press(inside())).unwrap(),
        PressOutcome::Activated
    );
}

#[test]
fn releasing_outside_never_activates_even_if_the_finger_started_on_the_button() {
    let mut button = tracker();
    button.handle_press(GestureState::Began, press(inside())).unwrap();
    button
        .handle_press(GestureState::Changed, press(Point::new(150.0, 680.0)))
        .unwrap();
    // Отпустил снаружи (подсветка уже снята): без срабатывания, без изменений.
    assert_eq!(
        button
            .handle_press(GestureState::Ended, press(Point::new(150.0, 680.0)))
            .unwrap(),
        PressOutcome::Unchanged
    );
    assert_eq!(button.state(), ButtonState::Normal);
    // Отпускание снаружи, когда подсветка ещё стояла (последнего `Changed` не было): снять, не сработать.
    let mut button = tracker();
    button.handle_press(GestureState::Began, press(inside())).unwrap();
    assert_eq!(
        button
            .handle_press(GestureState::Ended, press(Point::new(150.0, 700.0)))
            .unwrap(),
        PressOutcome::StateChanged(ButtonState::Normal)
    );
}

#[test]
fn tracking_zone_is_the_frame_plus_the_token_slop() {
    let mut button = tracker();
    // Нижний край 650; 7 pt ниже — в зоне (8 pt), 9 pt ниже — вне.
    button.handle_press(GestureState::Began, press(inside())).unwrap();
    assert_eq!(
        button
            .handle_press(GestureState::Changed, press(Point::new(150.0, 657.0)))
            .unwrap(),
        PressOutcome::Unchanged,
        "7 pt за краем — ещё на кнопке"
    );
    assert_eq!(
        button
            .handle_press(GestureState::Changed, press(Point::new(150.0, 659.0)))
            .unwrap(),
        PressOutcome::StateChanged(ButtonState::Normal),
        "9 pt за краем — ушёл"
    );
    // Отступ — токен: при 40 pt зона шире на 40 pt с каждой стороны.
    let mut tokens = UiKitTokens::default();
    tokens.density.press_tracking_slop_points = 40.0;
    let wide = PressTracker::new(BUTTON, &tokens).unwrap();
    assert_eq!(wide.tracking_zone(), Rect::from_xywh(60.0, 560.0, 180.0, 130.0));
}

#[test]
fn system_cancel_clears_the_highlight_without_activation() {
    let mut button = tracker();
    button.handle_press(GestureState::Began, press(inside())).unwrap();
    assert_eq!(
        button.handle_press(GestureState::Cancelled, GestureData::None).unwrap(),
        PressOutcome::StateChanged(ButtonState::Normal)
    );
    assert_eq!(button.state(), ButtonState::Normal);
    // Отмена нажатой уже снятой кнопки — не событие.
    assert_eq!(
        button.handle_press(GestureState::Cancelled, GestureData::None).unwrap(),
        PressOutcome::Unchanged
    );
}

#[test]
fn bad_input_is_an_error_not_a_silent_press() {
    let mut button = tracker();
    let tap_data = GestureData::Tap {
        position: inside(),
        tap_count: 1,
    };
    assert_eq!(
        button.handle_press(GestureState::Ended, tap_data),
        Err(ButtonError::NotPressData {
            state: GestureState::Ended
        })
    );
    assert_eq!(button.state(), ButtonState::Normal);
    let degenerate = Rect::from_xywh(0.0, 0.0, 0.0, 10.0);
    assert_eq!(
        PressTracker::new(degenerate, &UiKitTokens::default()),
        Err(ButtonError::InvalidFrame { frame: degenerate })
    );
    let mut tokens = UiKitTokens::default();
    tokens.density.press_tracking_slop_points = -1.0;
    assert!(matches!(
        PressTracker::new(BUTTON, &tokens),
        Err(ButtonError::Tokens(_))
    ));
    // `set_frame` с вырожденной рамкой не меняет зону.
    let before = button.tracking_zone();
    assert!(button.set_frame(degenerate).is_err());
    assert_eq!(button.tracking_zone(), before);
    button.set_frame(Rect::from_xywh(0.0, 0.0, 10.0, 10.0)).unwrap();
    assert_eq!(button.tracking_zone(), Rect::from_xywh(-8.0, -8.0, 26.0, 26.0));
}

// ---------- на настоящей сцене: кнопка внутри видоискателя с Tap ----------

/// Начало в (0, 0): координаты потомка совпадают с экранными, рамка кнопки годится и трекеру.
const VIEWFINDER: Rect = Rect::from_xywh(0.0, 0.0, 440.0, 900.0);

struct Screen {
    scene: UiScene<&'static str>,
    button: PressTracker,
    /// Сколько раз кнопка сработала / сколько раз сработал фокус.
    activations: usize,
    focus_taps: usize,
    /// Состояния подсветки по порядку изменений.
    highlight_log: Vec<ButtonState>,
}

/// `with_button_tap` — ставить ли на кнопку `Tap` (рецепт) или только `Press` (ловушка).
fn screen(with_button_tap: bool) -> Screen {
    let mut scene: UiScene<&'static str> = empty_scene();
    let viewfinder = add_rect(&mut scene, "viewfinder", VIEWFINDER);
    scene
        .add_recognizer(viewfinder, RecognizerSpec::Tap(TapSpec::default()), "focus_tap")
        .unwrap();
    let button_node: NodeId = {
        let node = scene.insert_child(viewfinder, "button").unwrap();
        scene.set_placement(node, anchored_rect(BUTTON)).unwrap();
        scene.layout().unwrap();
        node
    };
    scene
        .add_recognizer(button_node, RecognizerSpec::Press(PressSpec::default()), "button_press")
        .unwrap();
    if with_button_tap {
        scene
            .add_recognizer(button_node, RecognizerSpec::Tap(TapSpec::default()), "button_tap")
            .unwrap();
    }
    Screen {
        scene,
        button: tracker(),
        activations: 0,
        focus_taps: 0,
        highlight_log: Vec::new(),
    }
}

impl Screen {
    /// Касание в pt (уходит в пикселях), события арены разбираются так же, как будет делать экран.
    fn touch(&mut self, phase: TouchPhase, x_pt: f64, y_pt: f64, millis: i64) -> Vec<GestureEvent<&'static str>> {
        self.scene.handle_touch(&touch(0, phase, x_pt, y_pt, millis));
        let events = self.scene.drain_gesture_events();
        for event in &events {
            match event.intent {
                "button_press" => match self.button.handle_press(event.state, event.data).unwrap() {
                    PressOutcome::Activated => {
                        self.activations += 1;
                        self.highlight_log.push(ButtonState::Normal);
                    }
                    PressOutcome::StateChanged(state) => self.highlight_log.push(state),
                    PressOutcome::Unchanged => {}
                },
                "focus_tap" if event.state == GestureState::Ended => self.focus_taps += 1,
                _ => {}
            }
        }
        events
    }
}

/// Тап по кнопке внутри видоискателя: кнопка сработала, фокус провален.
#[test]
fn tapping_the_button_inside_the_viewfinder_presses_it_and_does_not_set_focus() {
    let mut screen = screen(true);
    let down = screen.touch(TouchPhase::Began, 150.0, 625.0, 0);
    assert_eq!(
        screen.button.state(),
        ButtonState::Pressed,
        "подсветка при касании: {down:?}"
    );
    assert_eq!(screen.activations, 0, "срабатывание только по отпусканию");
    let up = screen.touch(TouchPhase::Ended, 150.0, 625.0, 80);
    assert_eq!(screen.activations, 1);
    assert_eq!(screen.focus_taps, 0, "фокус не ставится");
    assert_eq!(screen.highlight_log, [ButtonState::Pressed, ButtonState::Normal]);
    let names: Vec<String> = up
        .iter()
        .map(|event| format!("{}:{:?}", event.intent, event.state))
        .collect();
    assert!(names.contains(&"focus_tap:Failed".to_string()), "{names:?}");
    assert!(names.contains(&"button_tap:Ended".to_string()), "{names:?}");
}

/// Ловушка: кнопка только с `Press` (без `Tap`) срабатывает ВМЕСТЕ с фокусом видоискателя.
#[test]
fn trap_a_press_only_button_inside_a_tap_viewfinder_fires_together_with_the_focus() {
    let mut screen = screen(false);
    screen.touch(TouchPhase::Began, 150.0, 625.0, 0);
    screen.touch(TouchPhase::Ended, 150.0, 625.0, 80);
    assert_eq!(screen.activations, 1);
    assert_eq!(screen.focus_taps, 1, "фокус тоже сработал: поэтому на кнопке нужен Tap");
}

/// Увёл палец за зону, вернул, отпустил на кнопке: подсветка Pressed → Normal → Pressed → Normal,
/// кнопка сработала один раз; фокус не ставится (сдвиг больше 10 pt провалил и `Tap` кнопки, и `Tap` видоискателя).
#[test]
fn leaving_and_coming_back_through_the_scene_still_activates_once_and_sets_no_focus() {
    let mut screen = screen(true);
    screen.touch(TouchPhase::Began, 150.0, 625.0, 0);
    screen.touch(TouchPhase::Moved, 150.0, 700.0, 16);
    assert_eq!(screen.button.state(), ButtonState::Normal);
    screen.touch(TouchPhase::Moved, 150.0, 625.0, 32);
    assert_eq!(screen.button.state(), ButtonState::Pressed);
    screen.touch(TouchPhase::Ended, 150.0, 625.0, 48);
    assert_eq!(screen.activations, 1);
    assert_eq!(screen.focus_taps, 0);
    assert_eq!(
        screen.highlight_log,
        [
            ButtonState::Pressed,
            ButtonState::Normal,
            ButtonState::Pressed,
            ButtonState::Normal
        ]
    );
}

/// Отпустил снаружи — ни кнопки, ни фокуса; системная отмена касания — подсветка снята без срабатывания.
#[test]
fn releasing_outside_and_cancelling_through_the_scene_activate_nothing() {
    let mut screen = screen(true);
    screen.touch(TouchPhase::Began, 150.0, 625.0, 0);
    screen.touch(TouchPhase::Moved, 150.0, 700.0, 16);
    screen.touch(TouchPhase::Ended, 150.0, 700.0, 32);
    assert_eq!((screen.activations, screen.focus_taps), (0, 0));

    let mut screen = self::screen(true);
    screen.touch(TouchPhase::Began, 150.0, 625.0, 0);
    assert_eq!(screen.button.state(), ButtonState::Pressed);
    screen.touch(TouchPhase::Cancelled, 150.0, 625.0, 16);
    assert_eq!(screen.button.state(), ButtonState::Normal);
    assert_eq!((screen.activations, screen.focus_taps), (0, 0));
}

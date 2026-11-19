#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Сцена: вход в пикселях → pt → хит-тест → арена → события; кадры и таймеры; дыры в потоке;
//! смена размера surface.

mod common;

use common::*;
use shell_contract::touch::TouchPhase;
use ui_core::geometry::Rect;
use ui_core::gesture::{
    GestureData, GestureState, LongPressSpec, PanSpec, PointerDisposition, RecognizerSpec, TapSpec,
};
use ui_core::id::NodeId;
use ui_core::input::UiInput;
use ui_core::scene::{InputOutcome, SceneError, UiScene};
use ui_core::time::ClockAdvance;
use ui_core::tree::TreeError;

/// Видоискатель на весь экран с кнопкой затвора внутри: кнопка 100x80 pt, левый верхний угол (170, 800).
struct Viewfinder {
    scene: UiScene<&'static str>,
    viewfinder: NodeId,
    button: NodeId,
}

fn viewfinder_scene() -> Viewfinder {
    let mut scene = empty_scene();
    let root = scene.tree().root();
    let viewfinder = add_rect(&mut scene, root, "viewfinder", Rect::from_xywh(0.0, 0.0, 440.0, 982.0));
    let button = add_rect(
        &mut scene,
        viewfinder,
        "shutter",
        Rect::from_xywh(170.0, 800.0, 100.0, 80.0),
    );
    scene
        .add_recognizer(viewfinder, RecognizerSpec::Tap(TapSpec::default()), "viewfinder_tap")
        .unwrap();
    scene
        .add_recognizer(button, RecognizerSpec::Tap(TapSpec::default()), "shutter_tap")
        .unwrap();
    Viewfinder {
        scene,
        viewfinder,
        button,
    }
}

/// Тап: касание и отрыв в одной точке, пиксели.
fn tap_px(scene: &mut UiScene<&'static str>, x_px: f32, y_px: f32, start_ms: i64) {
    let down = scene.handle_touch(&touch_px(0, TouchPhase::Began, x_px, y_px, start_ms));
    let up = scene.handle_touch(&touch_px(0, TouchPhase::Ended, x_px, y_px, start_ms + 40));
    assert_eq!((down, up), (PointerDisposition::Applied, PointerDisposition::Applied));
}

fn ended_intents(scene: &mut UiScene<&'static str>) -> Vec<&'static str> {
    scene
        .drain_gesture_events()
        .into_iter()
        .filter(|event| event.state == GestureState::Ended)
        .map(|event| event.intent)
        .collect()
}

/// Тап по кнопке в пикселях: побеждает глубже (кнопка), видоискатель молчит; мимо кнопки —
/// наоборот. Граница кнопки (x = 170 pt = 417,27 px) проверяет сам перевод пикселей: при
/// «пиксели = pt» оба касания попали бы в видоискатель.
#[test]
fn tap_in_pixels_goes_to_deepest_element_under_finger() {
    let mut stand = viewfinder_scene();

    // Центр кнопки: (220, 840) pt = (540, 2061,8) px.
    tap_px(&mut stand.scene, 540.0, 2061.8, 0);
    let events = stand.scene.drain_gesture_events();
    let ended: Vec<_> = events.iter().filter(|e| e.state == GestureState::Ended).collect();
    assert_eq!(ended.len(), 1);
    assert_eq!(ended[0].intent, "shutter_tap");
    assert_eq!(ended[0].node, stand.button);
    match ended[0].data {
        GestureData::Tap { position, tap_count } => {
            assert_eq!(tap_count, 1);
            assert!(
                (position.x - 220.0).abs() < 0.01 && (position.y - 840.0).abs() < 0.01,
                "{position:?}"
            );
        }
        other => panic!("ожидали данные тапа, получили {other:?}"),
    }
    assert!(
        events
            .iter()
            .any(|e| e.intent == "viewfinder_tap" && e.state == GestureState::Failed),
        "проигравший видоискатель получает Failed"
    );

    // Мимо кнопки, но в видоискателе.
    tap_px(&mut stand.scene, 200.0, 800.0, 1000);
    assert_eq!(ended_intents(&mut stand.scene), ["viewfinder_tap"]);

    // Чуть левее края кнопки (416 px = 169,5 pt) — видоискатель; чуть правее (419 px = 170,7 pt) — кнопка.
    tap_px(&mut stand.scene, 416.0, 2061.8, 2000);
    assert_eq!(ended_intents(&mut stand.scene), ["viewfinder_tap"]);
    tap_px(&mut stand.scene, 419.0, 2061.8, 3000);
    assert_eq!(ended_intents(&mut stand.scene), ["shutter_tap"]);

    // Вне рамки видоискателя (982 pt = 2410,9 px), но ещё в корне (982,7 pt) — путь только до корня: никто не отвечает.
    tap_px(&mut stand.scene, 540.0, 2411.5, 4000);
    assert!(ended_intents(&mut stand.scene).is_empty());
    assert!(stand.scene.tree().contains(stand.viewfinder));
}

/// Долгий тап срабатывает от кадров при неподвижном пальце: касаний между `Began` и порогом нет.
#[test]
fn long_press_fires_from_frames_alone() {
    let mut stand = viewfinder_scene();
    stand
        .scene
        .add_recognizer(
            stand.viewfinder,
            RecognizerSpec::LongPress(LongPressSpec::default()),
            "focus_lock",
        )
        .unwrap();

    stand
        .scene
        .handle_touch(&touch_px(0, TouchPhase::Began, 200.0, 800.0, 0));
    for frame_ms in [100, 250, 499] {
        let outcome = stand.scene.handle_input(UiInput::Frame(at_ms(frame_ms)));
        assert!(matches!(outcome, InputOutcome::Frame(_)));
        assert!(
            stand
                .scene
                .drain_gesture_events()
                .iter()
                .all(|e| e.intent != "focus_lock"),
            "до 500 мс долгий тап не начинается ({frame_ms} мс)"
        );
    }
    let outcome = stand.scene.handle_input(UiInput::Frame(at_ms(517)));
    let InputOutcome::Frame(report) = outcome else {
        panic!("кадр должен вернуть отчёт кадра");
    };
    assert!(matches!(report.clock, ClockAdvance::Advanced { .. }));
    let events = stand.scene.drain_gesture_events();
    let began: Vec<_> = events.iter().filter(|e| e.intent == "focus_lock").collect();
    assert_eq!(began.len(), 1, "{events:?}");
    assert_eq!(began[0].state, GestureState::Began);
    assert_eq!(began[0].time, at_ms(517), "метка события — время кадра, а не касания");
    assert_eq!(stand.scene.frame_time(), Some(at_ms(517)));
}

/// Дыра в потоке касаний посреди пана отменяет жест с его намерением.
#[test]
fn stream_gap_cancels_running_pan() {
    let mut stand = viewfinder_scene();
    stand
        .scene
        .add_recognizer(
            stand.viewfinder,
            RecognizerSpec::Pan(PanSpec::default()),
            "pan_exposure",
        )
        .unwrap();
    stand
        .scene
        .handle_touch(&touch_px(0, TouchPhase::Began, 200.0, 800.0, 0));
    stand
        .scene
        .handle_touch(&touch_px(0, TouchPhase::Moved, 260.0, 800.0, 16));
    stand
        .scene
        .handle_touch(&touch_px(0, TouchPhase::Moved, 320.0, 800.0, 32));
    let before = stand.scene.drain_gesture_events();
    assert!(
        before
            .iter()
            .any(|e| e.intent == "pan_exposure" && e.state == GestureState::Began)
    );

    assert_eq!(
        stand.scene.handle_input(UiInput::PointerStreamGap),
        InputOutcome::StreamGapHandled
    );
    let after = stand.scene.drain_gesture_events();
    assert!(
        after
            .iter()
            .any(|e| e.intent == "pan_exposure" && e.state == GestureState::Cancelled),
        "{after:?}"
    );
    // Отрыва уже не будет; позднее `Ended` игнорируется как неизвестный палец.
    let late = stand
        .scene
        .handle_touch(&touch_px(0, TouchPhase::Ended, 320.0, 800.0, 48));
    assert_eq!(late, PointerDisposition::IgnoredUnknownPointer);
}

/// Другая ширина surface: касания переводятся по НОВОМУ масштабу, идущий жест отменён,
/// размер экрана в pt остаётся эталонным.
#[test]
fn resize_surface_changes_scale_and_cancels_running_gesture() {
    let mut stand = viewfinder_scene();
    stand
        .scene
        .add_recognizer(
            stand.viewfinder,
            RecognizerSpec::Pan(PanSpec::default()),
            "pan_exposure",
        )
        .unwrap();
    stand
        .scene
        .handle_touch(&touch_px(0, TouchPhase::Began, 200.0, 800.0, 0));
    stand
        .scene
        .handle_touch(&touch_px(0, TouchPhase::Moved, 300.0, 800.0, 16));
    stand.scene.drain_gesture_events();

    // Экран шире при тех же пропорциях: 1440 x 3216 px.
    stand.scene.resize_surface(surface_sized(1440, 3216)).unwrap();
    assert!((stand.scene.point_scale().pixels_per_point() - 1440.0 / 440.0).abs() < 1e-12);
    let cancelled = stand.scene.drain_gesture_events();
    assert!(
        cancelled
            .iter()
            .any(|e| e.intent == "pan_exposure" && e.state == GestureState::Cancelled)
    );
    let root_frame = stand.scene.tree().frame(stand.scene.tree().root()).unwrap();
    assert!((root_frame.size.width - 440.0).abs() < 1e-9);
    assert!((root_frame.size.height - 3216.0 / (1440.0 / 440.0)).abs() < 1e-9);

    // Центр кнопки (220, 840) pt при 1440 px — это (720, 2749,09) px. При старом масштабе
    // 720 px были бы 293,3 pt — мимо кнопки.
    tap_px(&mut stand.scene, 720.0, 2749.09, 5000);
    assert_eq!(ended_intents(&mut stand.scene), ["shutter_tap"]);
    // А пиксель (540, 2061,8), центр кнопки при 1080, теперь — (160,6; 612,7) pt, не кнопка.
    tap_px(&mut stand.scene, 540.0, 2061.8, 6000);
    assert_eq!(ended_intents(&mut stand.scene), ["viewfinder_tap"]);
}

/// Ошибки API: распознаватель мёртвому элементу, удаление корня, неверная ширина surface.
#[test]
fn scene_api_reports_errors_instead_of_hiding_them() {
    let mut stand = viewfinder_scene();
    let removal = stand.scene.remove_subtree(stand.button).unwrap();
    assert_eq!(removal.removed, vec![stand.button]);
    let error = stand
        .scene
        .add_recognizer(stand.button, RecognizerSpec::Tap(TapSpec::default()), "ghost")
        .unwrap_err();
    assert_eq!(error, SceneError::Tree(TreeError::UnknownNode(stand.button)));

    let root = stand.scene.tree().root();
    assert_eq!(
        stand.scene.remove_subtree(root).unwrap_err(),
        SceneError::Tree(TreeError::CannotRemoveRoot)
    );
    assert!(matches!(
        stand.scene.resize_surface(surface(0)),
        Err(SceneError::PointScale(_))
    ));
    // Неудачная смена размера масштаб не трогает.
    assert!((stand.scene.point_scale().pixels_per_point() - PIXELS_PER_POINT).abs() < 1e-12);
}

/// Кадр «назад» не двигает часы; повтор — тоже; отчёт различает случаи.
#[test]
fn frame_report_distinguishes_clock_cases() {
    let mut stand = viewfinder_scene();
    assert!(matches!(
        stand.scene.advance_frame(at_ms(100)).clock,
        ClockAdvance::Advanced { .. }
    ));
    assert_eq!(stand.scene.advance_frame(at_ms(100)).clock, ClockAdvance::Repeated);
    assert!(matches!(
        stand.scene.advance_frame(at_ms(90)).clock,
        ClockAdvance::IgnoredBackwards { .. }
    ));
    assert_eq!(stand.scene.frame_time(), Some(at_ms(100)));
}

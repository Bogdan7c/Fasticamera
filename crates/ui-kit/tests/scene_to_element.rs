#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Сквозной тест шва `ui-core` → `ui-kit`: касания оболочки в ПИКСЕЛЯХ → сцена с распознавателем
//! `Pan` → данные жеста → намерения элемента ([`PanUpdate`]) → смещение и события элемента.
//! Так будет работать экран камеры (сессия 12): элемент ничего не знает об арене, а экран — о
//! знаках и осях элемента.

use shell_contract::touch::TouchPhase;
use ui_core::geometry::Rect;
use ui_core::gesture::{GestureData, GestureEvent, GestureState, PanDirection, PanSpec, RecognizerSpec};
use ui_core::scene::UiScene;
use ui_core::time::MotionPreference;
use ui_kit::carousel::{CarouselEntry, CarouselEvent, CarouselSpec, SelectionCause, SnapCarousel};
use ui_kit::drive::{DragOrigin, GrowthDirection, MotionPhase};
use ui_kit::pan::PanUpdate;
use ui_kit::scale::{ScaleEvent, ScaleSpec, SnapPolicy, SnapScale, TickVisibility, TravelDirection};
use ui_kit::tokens::{ScaleDensity, UiKitTokens};

mod common;
use common::*;

/// Длительность кадра 60 Гц, мс.
const FRAME_MS: i64 = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Video,
    Photo,
    Portrait,
}

/// Смещения точек прилипания для ширин 61,1 / 50 / 70 и зазора 35,4 (токен), посчитаны руками.
const OFFSET_PHOTO: f64 = 61.1 / 2.0 + 35.4 + 50.0 / 2.0;

fn mode_carousel(selected: Mode, tokens: &UiKitTokens) -> SnapCarousel<Mode> {
    let entries = vec![
        CarouselEntry {
            item: Mode::Video,
            width_points: 61.1,
        },
        CarouselEntry {
            item: Mode::Photo,
            width_points: 50.0,
        },
        CarouselEntry {
            item: Mode::Portrait,
            width_points: 70.0,
        },
    ];
    SnapCarousel::new(
        entries,
        &selected,
        CarouselSpec::default(),
        &env(tokens, MotionPreference::Full),
        ms(0),
    )
    .unwrap()
}

/// Что делает экран с событиями арены: превращает их в намерения элемента. Это и есть шов.
fn deliver_to_carousel(carousel: &mut SnapCarousel<Mode>, events: Vec<GestureEvent<&'static str>>) {
    for event in events {
        if let Some(update) = PanUpdate::from_gesture(event.state, event.data).unwrap() {
            carousel.apply_pan(event.time, update).unwrap();
        }
    }
}

/// Сцена с одной горизонтальной областью-каруселью на всю ширину; на ней — `Pan` по горизонтали.
fn carousel_scene() -> UiScene<&'static str> {
    let mut scene = empty_scene();
    let area = add_rect(&mut scene, "carousel", Rect::from_xywh(0.0, 600.0, 440.0, 60.0));
    scene
        .add_recognizer(
            area,
            RecognizerSpec::Pan(PanSpec {
                direction: PanDirection::Horizontal,
                ..PanSpec::default()
            }),
            "carousel_pan",
        )
        .unwrap();
    scene
}

const FINGER_Y_PT: f64 = 630.0;

/// Палец ведёт по горизонтали: `step_pt` за кадр (отрицательное — влево) `steps` раз.
/// Каждый кадр: касание → события арены → элемент → кадр сцены. Возвращает время последнего кадра.
struct HorizontalDrag {
    next_ms: i64,
    x_pt: f64,
}

impl HorizontalDrag {
    fn press(scene: &mut UiScene<&'static str>, start_x_pt: f64) -> Self {
        scene.handle_touch(&touch(0, TouchPhase::Began, start_x_pt, FINGER_Y_PT, 0));
        scene.drain_gesture_events();
        Self {
            next_ms: FRAME_MS,
            x_pt: start_x_pt,
        }
    }

    /// Один кадр движения. Возвращает события арены, уже полученные от сцены.
    fn step(
        &mut self,
        scene: &mut UiScene<&'static str>,
        phase: TouchPhase,
        step_pt: f64,
    ) -> Vec<GestureEvent<&'static str>> {
        self.x_pt += step_pt;
        scene.handle_touch(&touch(0, phase, self.x_pt, FINGER_Y_PT, self.next_ms));
        let events = scene.drain_gesture_events();
        self.next_ms += FRAME_MS;
        events
    }
}

/// Результат прогона: что видел элемент на каждом кадре.
struct Run {
    /// Положение пальца (то, что реально ушло в пикселях), pt.
    finger_x_pt: f64,
    /// Положение пальца на момент `Began` пана, pt.
    began_x_pt: Option<f64>,
    /// Время и событие `Ended`, если оно было.
    ended: Option<GestureEvent<&'static str>>,
}

fn run_horizontal(
    scene: &mut UiScene<&'static str>,
    carousel: &mut SnapCarousel<Mode>,
    drag: &mut HorizontalDrag,
    moves: usize,
    step_pt: f64,
    mut on_frame: impl FnMut(&SnapCarousel<Mode>, &Run, i64),
) -> Run {
    let mut run = Run {
        finger_x_pt: drag.x_pt,
        began_x_pt: None,
        ended: None,
    };
    for _ in 0..moves {
        let events = drag.step(scene, TouchPhase::Moved, step_pt);
        run.finger_x_pt = seen_in_points(to_pixels(drag.x_pt));
        for event in &events {
            if event.state == GestureState::Began {
                run.began_x_pt = Some(run.finger_x_pt);
            }
        }
        let frame_ms = drag.next_ms - FRAME_MS;
        deliver_to_carousel(carousel, events);
        scene.advance_frame(ms(frame_ms));
        on_frame(carousel, &run, frame_ms);
    }
    run
}

fn release_horizontal(
    scene: &mut UiScene<&'static str>,
    carousel: &mut SnapCarousel<Mode>,
    drag: &mut HorizontalDrag,
    step_pt: f64,
    run: &mut Run,
) {
    // Палец отрывается на ходу: последняя выборка продолжает движение.
    let events = drag.step(scene, TouchPhase::Ended, step_pt);
    run.ended = events.iter().find(|event| event.state == GestureState::Ended).cloned();
    deliver_to_carousel(carousel, events);
}

/// (а), (б): смещение карусели = смещению пальца в pt 1:1 с правильным знаком, на `Began` скачка нет.
#[test]
fn carousel_follows_the_pixel_finger_one_to_one_with_the_right_sign_and_no_hysteresis_jump() {
    let tokens = UiKitTokens::default();
    let mut scene = carousel_scene();
    let mut carousel = mode_carousel(Mode::Photo, &tokens);
    let mut drag = HorizontalDrag::press(&mut scene, 300.0);
    let mut began_checked = false;

    // Палец влево по 3 pt за кадр (медленно): Began придёт, когда путь превысит 10 pt.
    let run = run_horizontal(
        &mut scene,
        &mut carousel,
        &mut drag,
        14,
        -3.0,
        |carousel, run, frame_ms| {
            let now = ms(frame_ms);
            match run.began_x_pt {
                // До распознавания элемент не трогали: стоит на выбранном.
                None => assert_eq!(carousel.offset_points(now), OFFSET_PHOTO, "t = {frame_ms}"),
                Some(began_x) => {
                    // (а) Палец ушёл ВЛЕВО на d pt от точки распознавания — контент сместился ВПРАВО
                    // (смещение выросло) ровно на d. Эталон — разность пикселей, переведённая в pt.
                    let finger_travel = began_x - run.finger_x_pt;
                    assert!(
                        (carousel.offset_points(now) - (OFFSET_PHOTO + finger_travel)).abs() < 1e-6,
                        "t = {frame_ms}: {} против {}",
                        carousel.offset_points(now),
                        OFFSET_PHOTO + finger_travel
                    );
                    began_checked |= finger_travel == 0.0;
                }
            }
        },
    );
    // (б) На самом кадре `Began` смещение пальца от распознавания равно 0 и карусель не прыгнула
    // на путь гистерезиса (он был больше 10 pt). Если бы брали `from_touch_down`, прыжок был бы ≥ 10.
    assert!(began_checked, "кадр Began должен быть проверен");
    let began_x = run.began_x_pt.expect("пан распознан");
    assert!(300.0 - began_x > 10.0, "гистерезис пройден до Began: {began_x}");
    assert!(
        carousel.drain_events().is_empty(),
        "во время перетаскивания выбор не меняется"
    );
    assert_eq!(carousel.phase(ms(drag.next_ms - FRAME_MS)), MotionPhase::Touched);
}

/// (в), (г), (д): скорость после отпускания = скорости пальца (знак и величина), бросок листает
/// ровно на один элемент в обе стороны, ровно одно событие выбора.
#[test]
fn fling_hands_over_the_finger_velocity_and_pages_exactly_one_item_with_one_event() {
    let tokens = UiKitTokens::default();
    // Скорость пальца 3000 pt/с: 48 pt за кадр 16 мс. Проекция такого броска улетела бы за все элементы.
    let step_pt = 48.0;
    let finger_speed = step_pt / (FRAME_MS as f64 / 1000.0);
    for (start, leftward, expected_index) in [(Mode::Video, true, 1), (Mode::Portrait, false, 1)] {
        let direction = if leftward { -1.0 } else { 1.0 };
        let mut scene = carousel_scene();
        let mut carousel = mode_carousel(start, &tokens);
        let start_x = if leftward { 380.0 } else { 60.0 };
        let mut drag = HorizontalDrag::press(&mut scene, start_x);
        let mut run = run_horizontal(
            &mut scene,
            &mut carousel,
            &mut drag,
            3,
            direction * step_pt,
            |_, _, _| {},
        );
        release_horizontal(&mut scene, &mut carousel, &mut drag, direction * step_pt, &mut run);

        let ended = run.ended.clone().expect("пан закончился отрывом");
        // Время события = метка касания отрыва; элемент получил его как `now` отпускания.
        assert_eq!(ended.time, ms(drag.next_ms - FRAME_MS));
        let GestureData::Pan { velocity, .. } = ended.data else {
            panic!("данные пана ожидались: {:?}", ended.data);
        };
        // (в) Палец влево — скорость контента положительна (карусель «следующий» при FingerLeft),
        // величина равна скорости пальца. Допуск 1 pt/с (0,03 %): координаты ушли в `f32`-пикселях
        // (ошибка округления ≈ 1e-4 px на отсчёт), оценщик скорости делит её на окно ≈ 50 мс.
        let expected_content_velocity = -direction * finger_speed;
        assert!(
            (velocity.dx - direction * finger_speed).abs() < 1.0,
            "скорость пальца в данных пана: {}",
            velocity.dx
        );
        let first_sample = carousel.velocity_points_per_second(ended.time);
        assert!(
            (first_sample - expected_content_velocity).abs() < 1.0,
            "скорость карусели {first_sample}, ожидалась {expected_content_velocity} (палец {})",
            velocity.dx
        );
        // (г) Листание ровно на один в нужную сторону (не 0 и не 2).
        assert_eq!(carousel.selected_index(), expected_index, "старт {start:?}");
        // (д) Ровно одно SelectionChanged от пальца, за ним одна хаптика.
        let events = carousel.drain_events();
        let selections: Vec<_> = events
            .iter()
            .filter(|event| matches!(event, CarouselEvent::SelectionChanged { .. }))
            .collect();
        assert_eq!(selections.len(), 1, "{events:?}");
        assert!(matches!(
            selections[0],
            CarouselEvent::SelectionChanged {
                index: 1,
                cause: SelectionCause::Swipe,
                ..
            }
        ));
        assert_eq!(events.len(), 2, "выбор и одна хаптика: {events:?}");
        // Дальше кадры сцены довозят карусель до Photo пружиной; повторных событий нет.
        for frame in 1..=300 {
            scene.advance_frame(ms(drag.next_ms + frame * FRAME_MS));
        }
        let settled_at = ms(drag.next_ms + 300 * FRAME_MS);
        assert!((carousel.offset_points(settled_at) - OFFSET_PHOTO).abs() < 1e-9);
        assert_eq!(carousel.phase(settled_at), MotionPhase::Resting);
        assert!(carousel.drain_events().is_empty());
    }
}

// ---------- вертикальная шкала без рисок («солнце» экспозиции) ----------

/// Модель «EV ±2 по 1/3»: шаг 12 pt при плотности `Dense`.
fn sun_scale(initial_value: f64, tokens: &UiKitTokens) -> SnapScale<ui_kit::value::QuantizedLinear> {
    let spec = ScaleSpec {
        direction: GrowthDirection::FingerUp,
        snap: SnapPolicy::EveryStep,
        density: ScaleDensity::Dense,
        ticks: TickVisibility::Hidden,
        drag_origin: DragOrigin::FromRecognition,
    };
    SnapScale::new(
        exposure_model(),
        initial_value,
        spec,
        &env(tokens, MotionPreference::Full),
        ms(0),
    )
    .unwrap()
}

/// Медленный вертикальный проход пальцем вверх: ровно столько событий тиков, сколько рубежей
/// пройдено; значение растёт (палец вверх — значение вверх); хаптика — по одной на кадр с рубежом.
#[test]
fn vertical_sun_slow_pass_emits_exactly_one_tick_event_per_crossed_detent() {
    let tokens = UiKitTokens::default();
    let mut scene: UiScene<&'static str> = empty_scene();
    let area = add_rect(&mut scene, "sun", Rect::from_xywh(200.0, 300.0, 60.0, 300.0));
    scene
        .add_recognizer(
            area,
            RecognizerSpec::Pan(PanSpec {
                direction: PanDirection::Vertical,
                ..PanSpec::default()
            }),
            "sun_pan",
        )
        .unwrap();
    // Старт на -1 EV (−36 pt от нуля); шаг 12 pt, рубежи ступенчатой модели — на полушагах: -30, -18,
    // -6, 6, 18, 30, 42 (pt). Палец идёт вверх 2 pt за кадр.
    let mut scale = sun_scale(-1.0, &tokens);
    let (x_pt, start_y_pt) = (230.0, 550.0);
    scene.handle_touch(&touch(0, TouchPhase::Began, x_pt, start_y_pt, 0));
    scene.drain_gesture_events();

    let mut y_pt = start_y_pt;
    let mut began_y_pt = None;
    let mut crossings = Vec::new();
    let mut haptics = 0;
    let mut frame_ms = 0;
    // Путь: 6 pt на гистерезис Began не хватит, пан начнётся после > 10 pt; дальше ведём палец
    // до тех пор, пока контент не пройдёт 78 pt от точки распознавания (6,5 шага).
    for _ in 0..60 {
        frame_ms += FRAME_MS;
        y_pt -= 2.0;
        scene.handle_touch(&touch(0, TouchPhase::Moved, x_pt, y_pt, frame_ms));
        let events = scene.drain_gesture_events();
        let seen_y = seen_in_points(to_pixels(y_pt));
        for event in &events {
            if event.state == GestureState::Began {
                began_y_pt = Some(seen_y);
            }
        }
        for event in events {
            if let Some(update) = PanUpdate::from_gesture(event.state, event.data).unwrap() {
                scale.apply_pan(event.time, update).unwrap();
            }
        }
        scene.advance_frame(ms(frame_ms));
        scale.advance(ms(frame_ms));
        for event in scale.drain_events() {
            match event {
                ScaleEvent::TickCrossed { direction } | ScaleEvent::ReferenceCrossed { direction } => {
                    crossings.push((frame_ms, direction));
                }
                ScaleEvent::Haptic { .. } => haptics += 1,
                ScaleEvent::ValueChanged { .. } | ScaleEvent::Settled { .. } => {}
            }
        }
        if let Some(began_y) = began_y_pt
            && began_y - seen_y >= 78.0
        {
            break;
        }
    }
    let began_y = began_y_pt.expect("пан распознан");
    let travelled = began_y - seen_in_points(to_pixels(y_pt));
    // Контент идёт 1:1 за пальцем, палец вверх — значение вверх (знак `FingerUp`).
    let snapshot = scale.snapshot(ms(frame_ms));
    assert!(
        (snapshot.offset_points - (-36.0 + travelled)).abs() < 1e-6,
        "{} против {}",
        snapshot.offset_points,
        -36.0 + travelled
    );
    // Рубежи на полушагах от −36: -30, -18, -6, 6, 18, 30, 42; от −36 до −36+travelled(≥78) пройдено 7.
    assert!((78.0..=80.5).contains(&travelled), "{travelled}");
    assert_eq!(crossings.len(), 7, "{crossings:?}");
    assert!(
        crossings
            .iter()
            .all(|(_, direction)| *direction == TravelDirection::Increasing)
    );
    assert_eq!(
        haptics, 7,
        "по одной хаптике на кадр с рубежом (кадры не совпадают: 2 pt за кадр)"
    );
    // Каждый рубеж — на своём кадре: в событиях нет пропусков и дублей.
    let mut frames: Vec<i64> = crossings.iter().map(|(frame, _)| *frame).collect();
    frames.dedup();
    assert_eq!(frames.len(), 7);
}

/// Системная отмена касания (`TouchPhase::Cancelled`) через шов: карусель возвращается к элементу,
/// выбранному до жеста, без события выбора и хаптики — режим камеры не меняется.
#[test]
fn system_cancel_through_the_seam_returns_the_carousel_without_selecting_anything() {
    let tokens = UiKitTokens::default();
    let mut scene = carousel_scene();
    let mut carousel = mode_carousel(Mode::Video, &tokens);
    let mut drag = HorizontalDrag::press(&mut scene, 380.0);
    // Тянем дальше половины пути до Photo (60 pt из 90,95): отпускание выбрало бы Photo.
    run_horizontal(&mut scene, &mut carousel, &mut drag, 9, -8.0, |_, _, _| {});
    assert!(carousel.offset_points(ms(drag.next_ms - FRAME_MS)) > 50.0);
    let events = drag.step(&mut scene, TouchPhase::Cancelled, 0.0);
    assert!(events.iter().any(|event| event.state == GestureState::Cancelled));
    deliver_to_carousel(&mut carousel, events);
    assert_eq!(carousel.selected(), &Mode::Video);
    assert!(carousel.drain_events().is_empty());
    let after = ms(drag.next_ms + 10_000);
    scene.advance_frame(after);
    assert!(carousel.offset_points(after).abs() < 1e-9, "вернулась к Video");
    assert_eq!(carousel.phase(after), MotionPhase::Resting);
}

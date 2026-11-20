//! Тесты зума в редьюсере (сессия 13): усыновление значения, переход по тапу, колесо, щипок, переворот,
//! откат после неудачи смены объектива. Проверяются итоги: значение в состоянии, намерения и кадрирование в
//! эффектах, отказы — не вызовы функций.

use camera_contract::ids::{LensFacing, LensRole};
use camera_contract::zoom::{LensIntent, ZoomGesture, ZoomRatio};

use crate::action::ScreenAction;
use crate::effect::Effect;
use crate::outcome::{ActionOutcome, NoChangeReason, Rejection};
use crate::reduce::ScreenTransition;
use crate::test_support::{Screen, at_ms, live_facts, ratio, zoom_catalog, zoom_facts, zoom_facts_with_failure};

/// Намерения объектива из эффектов, по порядку.
fn lens_intents(effects: &[Effect]) -> Vec<LensIntent> {
    effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::Lens(intent) => Some(*intent),
            _ => None,
        })
        .collect()
}

/// Коэффициент в последней команде кадрирования рендеру.
fn framing_ratio(effects: &[Effect]) -> Option<f32> {
    effects.iter().rev().find_map(|effect| match effect {
        Effect::Viewfinder(intent) => Some(intent.zoom.ratio.value()),
        _ => None,
    })
}

fn near(left: f32, right: f32) -> bool {
    (left - right).abs() < 1.0e-3
}

/// Колесо открыто пальцем, чтобы его двигать.
fn open_wheel(screen: &mut Screen, at: i64) {
    let opened = screen.step(ScreenAction::ZoomWheelTouched { at: at_ms(at) });
    assert_eq!(opened.outcome, ActionOutcome::Applied);
    assert!(screen.state.zoom().is_wheel_open());
}

fn ratio_of(screen: &Screen) -> f32 {
    screen.state.zoom().ratio().value()
}

// ---- усыновление значения из фактов (переустановка экрана) ----

#[test]
fn a_fresh_screen_adopts_the_active_lens_without_telling_the_camera() {
    // Экран создан, когда активен ширик: значение 0,593, а не условная единица; намерение не публикуется.
    let mut screen = Screen::unprepared(crate::ScreenConfig::default());
    let resized = screen.step(ScreenAction::SurfaceResized(crate::test_support::OUR_SURFACE));
    let adopted = screen.step(ScreenAction::CameraFactsChanged(zoom_facts(LensRole::UltraWide)));

    assert!(near(ratio_of(&screen), 0.593), "{}", ratio_of(&screen));
    assert_eq!(screen.state.zoom().facing(), LensFacing::Back);
    assert!(screen.state.zoom().is_adopted());
    assert!(lens_intents(&resized.effects).is_empty() && lens_intents(&adopted.effects).is_empty());
    // Рендер получает кадрирование с адоптированным значением и каталогом.
    assert!(near(framing_ratio(&adopted.effects).expect("кадрирование"), 0.593));
    let Some(Effect::Viewfinder(framing)) = adopted.effects.last() else {
        panic!("{:?}", adopted.effects)
    };
    assert_eq!(framing.zoom.catalog, zoom_catalog());
}

#[test]
fn a_fresh_screen_with_front_camera_active_adopts_the_front_direction() {
    let screen = Screen::zoomed(LensRole::Front);
    assert_eq!(screen.state.zoom().facing(), LensFacing::Front);
    assert!(near(ratio_of(&screen), 1.0));
    assert!(screen.state.zoom_group().is_some());
}

#[test]
fn adoption_happens_once_and_later_active_lens_changes_do_not_override_the_user() {
    let mut screen = Screen::zoomed(LensRole::Wide);
    screen.step(ScreenAction::ZoomWheelTouched { at: at_ms(10) });
    screen.step(ScreenAction::ZoomWheelDragged {
        requested: ratio(1.7),
        at: at_ms(20),
    });
    // Движок с гистерезисом переключился на ширик: экран свой коэффициент не меняет.
    let echo = screen.step(ScreenAction::CameraFactsChanged(zoom_facts(LensRole::UltraWide)));
    assert!(near(ratio_of(&screen), 1.7));
    assert!(lens_intents(&echo.effects).is_empty());
}

// ---- тап по кнопке ленты ----

#[test]
fn tapping_the_ultra_wide_stop_glides_to_it_and_publishes_intents_ending_at_the_stop_ratio() {
    let mut screen = Screen::zoomed(LensRole::Wide);
    let started = screen.step(ScreenAction::ZoomStopTapped {
        stop: 0,
        at: at_ms(1000),
    });
    assert_eq!(started.outcome, ActionOutcome::Applied);
    assert!(screen.state.zoom().is_gliding());

    let mut intents = lens_intents(&started.effects);
    let mut last_framing = framing_ratio(&started.effects);
    let mut previous = ratio_of(&screen);
    // Кадры по 16 мс до конца перехода (0,27 с): значение монотонно падает к упору.
    for step in 1..=25 {
        let tick = screen.step(ScreenAction::Tick(at_ms(1000 + step * 16)));
        let now = ratio_of(&screen);
        assert!(
            now <= previous + 1.0e-6,
            "значение должно только убывать: {previous} → {now}"
        );
        previous = now;
        intents.extend(lens_intents(&tick.effects));
        last_framing = framing_ratio(&tick.effects).or(last_framing);
    }
    assert!(!screen.state.zoom().is_gliding());
    assert!(near(ratio_of(&screen), 0.593));
    assert!(intents.len() > 3, "намерения идут по ходу перехода: {}", intents.len());
    assert!(intents.iter().all(|intent| intent.facing == LensFacing::Back));
    let final_intent = intents.last().expect("намерения");
    assert!(near(final_intent.ratio.value(), 0.593), "{final_intent:?}");
    assert!(near(last_framing.expect("кадрирование"), 0.593));
}

#[test]
fn the_glide_follows_a_smooth_curve_that_starts_and_ends_slowly() {
    // Кривая прогресса ленты `[measured: R00 M:124]`: 0,04 (33 мс) / 0,28 (99 мс) / 0,81 (198 мс) при 0,27 с.
    let mut screen = Screen::zoomed(LensRole::Wide);
    screen.step(ScreenAction::ZoomStopTapped { stop: 2, at: at_ms(0) });
    // Прогресс в логарифме: log(r)/log(2).
    let progress_at = |screen: &mut Screen, ms: i64| {
        screen.step(ScreenAction::Tick(at_ms(ms)));
        f64::from(ratio_of(screen)).log2()
    };
    let p33 = progress_at(&mut screen, 33);
    let p99 = progress_at(&mut screen, 99);
    let p198 = progress_at(&mut screen, 198);
    assert!((p33 - 0.04).abs() < 0.02, "{p33}");
    assert!((p99 - 0.28).abs() < 0.03, "{p99}");
    assert!((p198 - 0.81).abs() < 0.03, "{p198}");
}

#[test]
fn tapping_the_stop_that_is_already_shown_changes_nothing_and_publishes_nothing() {
    let mut screen = Screen::zoomed(LensRole::Wide);
    let again = screen.step(ScreenAction::ZoomStopTapped {
        stop: 1,
        at: at_ms(100),
    });
    assert_eq!(again.outcome, ActionOutcome::NoChange(NoChangeReason::ValueUnchanged));
    assert!(again.effects.is_empty());
}

#[test]
fn tapping_a_missing_stop_or_having_no_catalog_is_an_explicit_rejection() {
    let mut zoomed = Screen::zoomed(LensRole::Wide);
    let missing = zoomed.step(ScreenAction::ZoomStopTapped {
        stop: 9,
        at: at_ms(100),
    });
    assert_eq!(missing.outcome, ActionOutcome::Rejected(Rejection::ZoomStopMissing));
    assert!(missing.effects.is_empty());
    assert!(near(ratio_of(&zoomed), 1.0));

    let mut without_catalog = Screen::ready();
    for action in [
        ScreenAction::ZoomStopTapped { stop: 0, at: at_ms(1) },
        ScreenAction::ZoomWheelTouched { at: at_ms(2) },
        ScreenAction::ZoomPinchBegan { at: at_ms(3) },
        ScreenAction::FlipCameraRequested { at: at_ms(4) },
    ] {
        let rejected = without_catalog.step(action);
        assert_eq!(rejected.outcome, ActionOutcome::Rejected(Rejection::ZoomUnavailable));
        assert!(rejected.effects.is_empty());
    }
    assert_eq!(without_catalog.state.rejections().get(Rejection::ZoomUnavailable), 4);
}

// ---- колесо ----

#[test]
fn dragging_the_wheel_sets_the_ratio_clamps_to_the_range_and_publishes_each_value() {
    let mut screen = Screen::zoomed(LensRole::Wide);
    open_wheel(&mut screen, 100);

    let moved = screen.step(ScreenAction::ZoomWheelDragged {
        requested: ratio(1.7),
        at: at_ms(120),
    });
    assert!(near(ratio_of(&screen), 1.7));
    assert_eq!(
        lens_intents(&moved.effects),
        [LensIntent {
            facing: LensFacing::Back,
            ratio: ratio(1.7),
            gesture: ZoomGesture::Continuous,
        }]
    );
    assert!(near(framing_ratio(&moved.effects).expect("кадрирование"), 1.7));

    let beyond = screen.step(ScreenAction::ZoomWheelDragged {
        requested: ratio(50.0),
        at: at_ms(140),
    });
    assert!(near(ratio_of(&screen), 10.0), "верхний предел — из каталога");
    assert!(near(lens_intents(&beyond.effects)[0].ratio.value(), 10.0));

    let below = screen.step(ScreenAction::ZoomWheelDragged {
        requested: ratio(0.1),
        at: at_ms(160),
    });
    assert!(near(ratio_of(&screen), 0.593), "нижний предел — ширик");
    assert!(near(lens_intents(&below.effects)[0].ratio.value(), 0.593));
}

#[test]
fn a_late_wheel_event_after_the_wheel_left_does_not_move_the_value() {
    let mut screen = Screen::zoomed(LensRole::Wide);
    let late = screen.step(ScreenAction::ZoomWheelDragged {
        requested: ratio(3.0),
        at: at_ms(50),
    });
    assert_eq!(late.outcome, ActionOutcome::NoChange(NoChangeReason::NothingToShow));
    assert!(near(ratio_of(&screen), 1.0));
    assert!(late.effects.is_empty());
}

#[test]
fn the_wheel_leaves_2150_ms_after_the_finger_and_the_wheel_stop_but_not_under_a_resting_finger() {
    let mut screen = Screen::zoomed(LensRole::Wide);
    open_wheel(&mut screen, 1000);
    // Палец лежит: ни через 10 секунд колесо не уходит.
    screen.step(ScreenAction::Tick(at_ms(11_000)));
    assert!(screen.state.zoom().is_wheel_open());
    assert!(screen.state.zoom().is_wheel_touched());

    screen.step(ScreenAction::ZoomWheelDragged {
        requested: ratio(1.5),
        at: at_ms(11_100),
    });
    screen.step(ScreenAction::ZoomWheelReleased { at: at_ms(11_200) });
    screen.step(ScreenAction::Tick(at_ms(11_200 + 2149)));
    assert!(screen.state.zoom().is_wheel_open(), "ещё 1 мс до срока");
    screen.step(ScreenAction::Tick(at_ms(11_200 + 2150)));
    assert!(!screen.state.zoom().is_wheel_open());
    assert!(near(ratio_of(&screen), 1.5), "значение после ухода колеса сохраняется");
}

#[test]
fn wheel_movement_postpones_the_dismissal() {
    let mut screen = Screen::zoomed(LensRole::Wide);
    open_wheel(&mut screen, 0);
    screen.step(ScreenAction::ZoomWheelReleased { at: at_ms(100) });
    // Бросок колеса продолжает менять значение: срок отсчитывается от последнего изменения.
    screen.step(ScreenAction::ZoomWheelDragged {
        requested: ratio(1.2),
        at: at_ms(2000),
    });
    screen.step(ScreenAction::Tick(at_ms(2100 + 2000)));
    assert!(screen.state.zoom().is_wheel_open());
    screen.step(ScreenAction::Tick(at_ms(2000 + 2150)));
    assert!(!screen.state.zoom().is_wheel_open());
}

// ---- щипок ----

fn pinch(screen: &mut Screen, scale: f64, at: i64) -> ScreenTransition {
    screen.step(ScreenAction::ZoomPinchChanged { scale, at: at_ms(at) })
}

#[test]
fn pinch_multiplies_the_start_ratio_by_the_gesture_scale_and_clamps_to_the_range() {
    let mut screen = Screen::zoomed(LensRole::Wide);
    screen.step(ScreenAction::ZoomPinchBegan { at: at_ms(10) });
    assert!(screen.state.zoom().is_pinching());

    let doubled = pinch(&mut screen, 2.0, 20);
    assert!(near(ratio_of(&screen), 2.0));
    assert!(near(lens_intents(&doubled.effects)[0].ratio.value(), 2.0));

    // От начала жеста, а не от прошлого шага: масштаб 1,5 даёт 1,5, а не 3.
    pinch(&mut screen, 1.5, 30);
    assert!(near(ratio_of(&screen), 1.5));

    pinch(&mut screen, 100.0, 40);
    assert!(near(ratio_of(&screen), 10.0));
    pinch(&mut screen, 0.01, 50);
    assert!(near(ratio_of(&screen), 0.593));

    let ended = screen.step(ScreenAction::ZoomPinchEnded { at: at_ms(60) });
    assert_eq!(ended.outcome, ActionOutcome::Applied);
    assert!(!screen.state.zoom().is_pinching());
    assert!(near(ratio_of(&screen), 0.593), "значение остаётся там, куда дошло");
}

#[test]
fn front_camera_pinch_stops_at_three_times_from_the_catalog() {
    let mut screen = Screen::zoomed(LensRole::Front);
    screen.step(ScreenAction::ZoomPinchBegan { at: at_ms(10) });
    let clamped = pinch(&mut screen, 5.0, 20);
    assert!(near(ratio_of(&screen), 3.0));
    assert!(near(lens_intents(&clamped.effects)[0].ratio.value(), 3.0));
    assert_eq!(lens_intents(&clamped.effects)[0].facing, LensFacing::Front);
    // Отвести пальцы назад: ниже единицы фронталка не уходит.
    pinch(&mut screen, 0.2, 30);
    assert!(near(ratio_of(&screen), 1.0));
}

#[test]
fn pinch_with_a_nonsense_scale_is_rejected_and_keeps_the_value() {
    let mut screen = Screen::zoomed(LensRole::Wide);
    screen.step(ScreenAction::ZoomPinchBegan { at: at_ms(10) });
    for bad in [f64::NAN, 0.0, -1.0, f64::INFINITY] {
        let rejected = pinch(&mut screen, bad, 20);
        assert_eq!(
            rejected.outcome,
            ActionOutcome::Rejected(Rejection::InvalidZoomInput),
            "{bad}"
        );
        assert!(rejected.effects.is_empty());
    }
    assert!(near(ratio_of(&screen), 1.0));
    // Изменение без начала жеста — не угадывается.
    let mut idle = Screen::zoomed(LensRole::Wide);
    let stray = pinch(&mut idle, 2.0, 20);
    assert_eq!(stray.outcome, ActionOutcome::NoChange(NoChangeReason::NothingToShow));
    assert!(near(ratio_of(&idle), 1.0));
}

// ---- переворот ----

#[test]
fn flipping_goes_to_front_at_one_x_and_returns_to_the_last_back_ratio() {
    let mut screen = Screen::zoomed(LensRole::Wide);
    open_wheel(&mut screen, 10);
    screen.step(ScreenAction::ZoomWheelDragged {
        requested: ratio(1.4),
        at: at_ms(20),
    });

    let to_front = screen.step(ScreenAction::FlipCameraRequested { at: at_ms(1000) });
    assert_eq!(screen.state.zoom().facing(), LensFacing::Front);
    assert!(near(ratio_of(&screen), 1.0));
    assert_eq!(
        lens_intents(&to_front.effects),
        [LensIntent {
            facing: LensFacing::Front,
            ratio: ratio(1.0),
            gesture: ZoomGesture::Discrete,
        }]
    );
    assert!(!screen.state.zoom().is_wheel_open(), "переворот закрывает колесо");
    assert_eq!(screen.state.zoom().last_flip().expect("метка").sequence, 1);

    let back = screen.step(ScreenAction::FlipCameraRequested { at: at_ms(2000) });
    assert_eq!(screen.state.zoom().facing(), LensFacing::Back);
    assert!(near(ratio_of(&screen), 1.4), "возврат на последний задний коэффициент");
    assert!(near(lens_intents(&back.effects)[0].ratio.value(), 1.4));
    assert_eq!(lens_intents(&back.effects)[0].facing, LensFacing::Back);
    assert_eq!(screen.state.zoom().last_flip().expect("метка").sequence, 2);
}

#[test]
fn flipping_without_a_front_group_is_rejected_and_keeps_the_direction() {
    let mut screen = Screen::unprepared(crate::ScreenConfig::default());
    screen.step(ScreenAction::SurfaceResized(crate::test_support::OUR_SURFACE));
    let mut facts = zoom_facts(LensRole::Wide);
    let mut catalog = (*zoom_catalog()).clone();
    catalog.front = None;
    facts.lens.catalog = Some(std::sync::Arc::new(catalog));
    screen.step(ScreenAction::CameraFactsChanged(facts));

    let rejected = screen.step(ScreenAction::FlipCameraRequested { at: at_ms(5) });
    assert_eq!(rejected.outcome, ActionOutcome::Rejected(Rejection::ZoomUnavailable));
    assert_eq!(screen.state.zoom().facing(), LensFacing::Back);
    assert!(screen.state.zoom().last_flip().is_none());
}

// ---- неудача смены объектива ----

#[test]
fn a_new_switch_failure_snaps_the_ratio_into_the_active_lens_without_an_echo_intent() {
    let mut screen = Screen::zoomed(LensRole::UltraWide);
    screen.step(ScreenAction::ZoomPinchBegan { at: at_ms(10) });
    pinch(&mut screen, 20.0, 20);
    assert!(near(ratio_of(&screen), 10.0));

    // Переход на основную не удался, активен всё ещё ширик: его предел 0,593 × 10 = 5,93.
    let failed = screen.step(ScreenAction::CameraFactsChanged(zoom_facts_with_failure(
        LensRole::UltraWide,
        1,
        "2",
    )));
    assert!(near(ratio_of(&screen), 5.93), "{}", ratio_of(&screen));
    assert!(
        lens_intents(&failed.effects).is_empty(),
        "откат — синхронизация, не новое намерение"
    );
    assert!(near(framing_ratio(&failed.effects).expect("кадрирование"), 5.93));
    assert!(!screen.state.zoom().is_pinching(), "откат гасит идущий щипок");
}

#[test]
fn an_already_seen_failure_does_not_snap_again_and_a_fresh_screen_ignores_an_old_one() {
    // Экран создан, когда в фактах уже лежит неудача №7: она «уже учтена».
    let mut screen = Screen::unprepared(crate::ScreenConfig::default());
    screen.step(ScreenAction::SurfaceResized(crate::test_support::OUR_SURFACE));
    screen.step(ScreenAction::CameraFactsChanged(zoom_facts_with_failure(
        LensRole::UltraWide,
        7,
        "2",
    )));
    assert!(near(ratio_of(&screen), 0.593));
    screen.step(ScreenAction::ZoomPinchBegan { at: at_ms(10) });
    pinch(&mut screen, 20.0, 20);
    assert!(near(ratio_of(&screen), 10.0));

    // Тот же номер приходит снова (повторная доставка снимка): отката нет.
    let repeat = screen.step(ScreenAction::CameraFactsChanged(zoom_facts_with_failure(
        LensRole::UltraWide,
        7,
        "2",
    )));
    assert!(near(ratio_of(&screen), 10.0));
    assert!(lens_intents(&repeat.effects).is_empty());

    // Новый номер — откат.
    screen.step(ScreenAction::CameraFactsChanged(zoom_facts_with_failure(
        LensRole::UltraWide,
        8,
        "2",
    )));
    assert!(near(ratio_of(&screen), 5.93));
}

#[test]
fn facts_without_lens_information_leave_the_zoom_untouched() {
    let mut screen = Screen::zoomed(LensRole::Wide);
    let before = screen.state.zoom().clone();
    screen.step(ScreenAction::CameraFactsChanged(live_facts()));
    // Каталог пропал (движок перезапускается): значение пользователя не сбрасывается.
    assert_eq!(screen.state.zoom().ratio(), before.ratio());
    assert_eq!(screen.state.zoom().facing(), before.facing());
    let _ = ZoomRatio::ONE;
}

// ---- перевод «тап ↔ сенсор» берёт кадрирование и зеркало из формулы зума ----

mod mapping {
    use camera_contract::geometry::SensorOrientation;
    use ui_core::geometry::Point;

    use super::*;
    use crate::test_support::main_camera_stream;
    use crate::viewfinder_map::{MirrorMode, StreamGeometry};

    fn upright_stream(mirror_from_app: MirrorMode) -> StreamGeometry {
        StreamGeometry {
            sensor_orientation: SensorOrientation::Degrees0,
            mirror: mirror_from_app,
            ..main_camera_stream()
        }
    }

    fn screen_with(role: LensRole, mirror_from_app: MirrorMode) -> Screen {
        let mut screen = Screen::unprepared(crate::ScreenConfig::default());
        screen.step(ScreenAction::SurfaceResized(crate::test_support::OUR_SURFACE));
        let mut facts = zoom_facts(role);
        facts.stream = Some(upright_stream(mirror_from_app));
        screen.step(ScreenAction::CameraFactsChanged(facts));
        screen
    }

    /// Точка на `fraction` ширины кадра слева, по центру высоты.
    fn point_in_frame(screen: &Screen, fraction: f64) -> Point {
        let frame = screen
            .state
            .layout()
            .expect("раскладка")
            .frame_rect(screen.state.settings().aspect);
        Point::new(frame.min_x() + frame.size.width * fraction, frame.center().y)
    }

    fn sensor_x(screen: &Screen, fraction: f64) -> f32 {
        screen
            .state
            .viewfinder_mapping()
            .expect("перевод")
            .screen_to_sensor(point_in_frame(screen, fraction))
            .expect("точка в кадре")
            .x()
    }

    #[test]
    fn tap_on_the_left_of_the_front_preview_maps_to_the_right_of_the_sensor() {
        let back = screen_with(LensRole::Wide, MirrorMode::None);
        let front = screen_with(LensRole::Front, MirrorMode::None);
        assert!(sensor_x(&back, 0.1) < 0.2, "задняя: слева направо без зеркала");
        assert!(
            sensor_x(&front, 0.1) > 0.8,
            "фронтальная зеркалится: слева на экране — справа на сенсоре"
        );
    }

    #[test]
    fn mirror_comes_only_from_the_zoom_formula_never_doubled_by_the_stream_flag() {
        // Приложение пометило поток зеркальным, но камера задняя: формула зума решает, что зеркала нет.
        let back_with_stale_flag = screen_with(LensRole::Wide, MirrorMode::Horizontal);
        assert!(sensor_x(&back_with_stale_flag, 0.1) < 0.2);
        // Фронтальная с тем же флагом: зеркало одно, а не два (иначе вышло бы 0,1 снова).
        let front_with_flag = screen_with(LensRole::Front, MirrorMode::Horizontal);
        assert!(sensor_x(&front_with_flag, 0.1) > 0.8);
    }

    #[test]
    fn zoom_narrows_the_sensor_area_the_tap_maps_into() {
        let mut screen = screen_with(LensRole::Wide, MirrorMode::None);
        // 1×: левый край кадра — край сенсора.
        assert!(sensor_x(&screen, 0.0) < 0.01);
        screen.step(ScreenAction::ZoomWheelTouched { at: at_ms(5) });
        screen.step(ScreenAction::ZoomWheelDragged {
            requested: ratio(2.0),
            at: at_ms(10),
        });
        // 2× на основной: кроп вдвое уже по центру, левый край кадра — четверть сенсора.
        let edge = sensor_x(&screen, 0.0);
        assert!((edge - 0.25).abs() < 0.01, "{edge}");
        let right = sensor_x(&screen, 1.0);
        assert!((right - 0.75).abs() < 0.01, "{right}");
    }

    #[test]
    fn without_an_active_lens_the_mapping_uses_the_whole_frame() {
        let mut screen = Screen::unprepared(crate::ScreenConfig::default());
        screen.step(ScreenAction::SurfaceResized(crate::test_support::OUR_SURFACE));
        let mut facts = zoom_facts(LensRole::Wide);
        facts.stream = Some(upright_stream(MirrorMode::None));
        facts.lens.active = None;
        screen.step(ScreenAction::CameraFactsChanged(facts));
        assert!(sensor_x(&screen, 0.0) < 0.01);
    }
}

#[test]
fn a_panel_opening_over_the_wheel_closes_the_wheel_and_keeps_the_value() {
    let mut screen = Screen::zoomed(LensRole::Wide);
    open_wheel(&mut screen, 10);
    screen.step(ScreenAction::ZoomWheelDragged {
        requested: ratio(1.6),
        at: at_ms(20),
    });
    screen.step(ScreenAction::ControlsMenuOpenRequested);
    screen.step(ScreenAction::Tick(at_ms(30)));
    assert!(!screen.state.zoom().is_wheel_open());
    assert!(near(ratio_of(&screen), 1.6));
}

#[test]
fn lens_switch_facts_are_visible_to_the_state() {
    use crate::facts::LensSwitch;
    let mut screen = Screen::zoomed(LensRole::Wide);
    assert!(!screen.state.is_lens_switching());
    let mut facts = zoom_facts(LensRole::Wide);
    facts.lens.switch = LensSwitch::Switching {
        from: crate::test_support::camera_id("2"),
        to: crate::test_support::camera_id("3"),
    };
    screen.step(ScreenAction::CameraFactsChanged(facts));
    assert!(screen.state.is_lens_switching());
    screen.step(ScreenAction::CameraFactsChanged(zoom_facts(LensRole::UltraWide)));
    assert!(!screen.state.is_lens_switching());
}

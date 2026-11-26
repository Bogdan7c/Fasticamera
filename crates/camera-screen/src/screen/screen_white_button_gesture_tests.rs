//! Кнопка белого у затвора пальцем через настоящие касания и кадры экрана (white-balance 02, D4b):
//! тап открывает карточку, долгое нажатие берёт белое телефона ровно один раз и панель не открывает, число влезает
//! в круг, мерцание и тряска идут и кончаются.
//!
//! Каждая проверка смотрит результат: эффекты камеры, состояние панели, нарисованное число.

use camera_contract::white::{WhiteOrigin, WhiteSourceId};
use shell_contract::haptics::HapticPattern;
use shell_contract::orientation::DeviceOrientation;
use shell_contract::system_gesture::{RawEdgeInsetsPx, SystemGestureZones};
use ui_core::geometry::{Point, Rect};
use ui_core::time::MotionPreference;
use ui_draw::draw_list::TextItem;
use ui_draw::geometry::PxPoint;
use ui_kit::tokens::TextRoleKind;

use super::test_rig::*;
use super::white_test_support::*;
use crate::action::ScreenAction;
use crate::config::ScreenConfig;
use crate::effect::{Effect, ScreenCameraIntent};
use crate::state::PanelState;
use crate::tools::ToolId;
use crate::white_facts::{WhiteFacts, WhiteTakeFacts, WhiteTakeState};

const WHITE_CARD: PanelState = PanelState::Submenu(ToolId::White);

fn take_phone() -> ScreenCameraIntent {
    ScreenCameraIntent::TakeWhite(WhiteSourceId::PHONE)
}

fn count_take_phone(effects: &[Effect]) -> usize {
    camera_intents(effects)
        .iter()
        .filter(|intent| **intent == take_phone())
        .count()
}

fn count_haptic(effects: &[Effect], pattern: HapticPattern) -> usize {
    effects
        .iter()
        .filter(|effect| **effect == Effect::Haptic(pattern))
        .count()
}

// ---- тап и долгое нажатие ----

#[test]
fn a_short_tap_opens_the_card_and_takes_nothing() {
    let mut rig = Rig::new();
    let button = white_button(&rig);
    rig.tap(button);
    assert_eq!(rig.screen.state().panel(), WHITE_CARD);
    assert!(camera_intents(&rig.effects()).is_empty(), "тап белое не берёт");
}

#[test]
fn a_long_press_takes_the_phone_white_once_and_does_not_open_the_panel() {
    let mut rig = Rig::new();
    let button = white_button(&rig);
    long_press(&mut rig, button, 700);
    let effects = rig.effects();
    assert_eq!(count_take_phone(&effects), 1, "{effects:?}");
    assert_eq!(count_haptic(&effects, HapticPattern::MediumImpact), 1);
    // Отпускание после долгого нажатия панель не открывает.
    assert_eq!(rig.screen.state().panel(), PanelState::Closed);
    rig.run(300);
    assert_eq!(rig.screen.state().panel(), PanelState::Closed);
    assert!(rig.effects().is_empty(), "после отпускания ничего не добавилось");
}

#[test]
fn holding_for_seconds_still_takes_the_white_exactly_once() {
    let mut rig = Rig::new();
    let button = white_button(&rig);
    long_press(&mut rig, button, 4_000);
    assert_eq!(count_take_phone(&rig.effects()), 1);
}

#[test]
fn a_tap_after_a_long_press_opens_the_card_without_taking_again() {
    let mut rig = Rig::new();
    let button = white_button(&rig);
    long_press(&mut rig, button, 700);
    rig.effects();
    rig.tap(button);
    assert_eq!(rig.screen.state().panel(), WHITE_CARD);
    assert!(camera_intents(&rig.effects()).is_empty());
}

#[test]
fn a_finger_that_slides_off_the_icon_before_the_long_press_does_nothing() {
    let mut rig = Rig::new();
    let button = white_button(&rig);
    let start = rig.now_ms;
    rig.touch(shell_contract::touch::TouchPhase::Began, button, start);
    rig.touch(
        shell_contract::touch::TouchPhase::Moved,
        Point::new(button.x, button.y + 80.0),
        start + 100,
    );
    rig.now_ms = start + 900;
    rig.frame();
    rig.touch(
        shell_contract::touch::TouchPhase::Ended,
        Point::new(button.x, button.y + 80.0),
        rig.now_ms,
    );
    rig.run(100);
    assert_eq!(rig.screen.state().panel(), PanelState::Closed);
    assert!(camera_intents(&rig.effects()).is_empty());
}

#[test]
fn a_long_press_with_the_white_card_open_takes_the_white_and_keeps_the_card_open() {
    let mut rig = Rig::new();
    open_card(&mut rig);
    let button = white_button(&rig);
    long_press(&mut rig, button, 700);
    assert_eq!(count_take_phone(&rig.effects()), 1);
    assert_eq!(rig.screen.state().panel(), WHITE_CARD);
}

#[test]
fn a_long_press_with_another_card_open_takes_the_white_and_leaves_that_card_open() {
    let mut rig = Rig::new();
    rig.act(ScreenAction::StripToolTapped(ToolId::Exposure));
    rig.run(500);
    rig.effects();
    let button = white_button(&rig);
    long_press(&mut rig, button, 700);
    assert_eq!(count_take_phone(&rig.effects()), 1);
    assert_eq!(rig.screen.state().panel(), PanelState::ExposurePanel);
}

#[test]
fn a_long_press_under_the_controls_drawer_takes_nothing() {
    let mut rig = Rig::new();
    rig.act(ScreenAction::ControlsMenuOpenRequested);
    rig.run(600);
    rig.effects();
    let button = white_button(&rig);
    long_press(&mut rig, button, 700);
    assert_eq!(count_take_phone(&rig.effects()), 0);
}

#[test]
fn the_long_press_and_the_tap_work_in_both_landscape_orientations_too() {
    for orientation in [DeviceOrientation::LandscapeLeft, DeviceOrientation::LandscapeRight] {
        let mut rig = Rig::new();
        rig.act(ScreenAction::DeviceOrientationChanged(orientation));
        rig.run(600);
        let button = white_button(&rig);
        long_press(&mut rig, button, 700);
        assert_eq!(count_take_phone(&rig.effects()), 1, "{orientation:?}");
        assert_eq!(rig.screen.state().panel(), PanelState::Closed, "{orientation:?}");
        rig.tap(button);
        assert_eq!(rig.screen.state().panel(), WHITE_CARD, "{orientation:?}");
    }
}

#[test]
fn nothing_under_the_button_fires_the_viewfinder_focus_or_lock() {
    let mut rig = Rig::new();
    let button = white_button(&rig);
    rig.tap(button);
    // Карточка открылась и подняла кнопку вместе с затвором: долгое нажатие — уже по новому месту.
    rig.run(700);
    let raised_button = white_button(&rig);
    assert!(raised_button.y < button.y, "кнопка поднята");
    long_press(&mut rig, raised_button, 700);
    let intents = camera_intents(&rig.effects());
    assert!(
        intents.iter().all(|intent| !matches!(
            intent,
            ScreenCameraIntent::FocusAndExposeAt(_) | ScreenCameraIntent::LockFocusAndExposureAt(_)
        )),
        "{intents:?}"
    );
    // Счётчик считает долгое нажатие по кнопке белого (одно), а видоискатель под кнопкой молчит: фокусных
    // намерений выше нет.
    assert_eq!(
        rig.screen.diagnostics().gestures.long_presses,
        1,
        "одно долгое нажатие: по кнопке белого"
    );
}

#[test]
fn a_touch_that_began_in_the_system_gesture_zone_is_silent() {
    let mut rig = Rig::new();
    // Зона снизу, накрывающая кнопку: касание принадлежит системе, как бы долго его ни держали.
    let button = white_button(&rig);
    let screen_height_pt = rig.screen.state().layout().expect("раскладка").screen_size().height;
    let zone_px = ((screen_height_pt - button.y) * f64::from(PX_PER_PT)).ceil() as i32 + 40;
    rig.screen.set_system_gesture_zones(
        SystemGestureZones::try_from_insets(RawEdgeInsetsPx {
            left: 0,
            top: 0,
            right: 0,
            bottom: zone_px,
        })
        .expect("зоны"),
    );
    long_press(&mut rig, button, 700);
    rig.tap(button);
    assert!(rig.effects().is_empty());
    assert_eq!(rig.screen.state().panel(), PanelState::Closed);
    assert_eq!(rig.screen.diagnostics().touches_ignored_in_system_gesture_zone, 2);
}

// ---- значки верхнего ряда не изменились ----

#[test]
fn holding_another_icon_for_a_long_time_still_activates_it_on_release() {
    let mut rig = Rig::new();
    // Ряд прежний: вспышка, RAW, EV (таймер скрыт, пока выключен).
    let flash = rig.center_with(|g| g.strip_icon(0, 3));
    long_press(&mut rig, flash, 700);
    assert_eq!(
        rig.screen.state().settings().flash,
        crate::settings::FlashMode::Auto,
        "у вспышки долгого нажатия нет: удержание не отменяет срабатывание"
    );
    assert_eq!(count_take_phone(&rig.effects()), 0);
}

// ---- число на кнопке: влезает в круг и крутится ----

/// Прямоугольник нарисованного текста в px (ось-выровненный охват углов).
fn text_bounds_px(item: &TextItem) -> (f32, f32, f32, f32) {
    let corners = [
        (0.0, -item.run.cap_height_px),
        (item.run.advance_px, -item.run.cap_height_px),
        (item.run.advance_px, 0.0),
        (0.0, 0.0),
    ]
    .map(|(x, y)| item.transform.apply_point(PxPoint::new(x, y)));
    let min_x = corners.iter().map(|p| p.x).fold(f32::INFINITY, f32::min);
    let max_x = corners.iter().map(|p| p.x).fold(f32::NEG_INFINITY, f32::max);
    let min_y = corners.iter().map(|p| p.y).fold(f32::INFINITY, f32::min);
    let max_y = corners.iter().map(|p| p.y).fold(f32::NEG_INFINITY, f32::max);
    (min_x, min_y, max_x, max_y)
}

fn rect_px(rect: Rect) -> (f32, f32, f32, f32) {
    (px(rect.min_x()), px(rect.min_y()), px(rect.max_x()), px(rect.max_y()))
}

#[test]
fn a_five_digit_number_stays_inside_the_glass_circle_in_every_orientation() {
    for orientation in [
        DeviceOrientation::Portrait,
        DeviceOrientation::LandscapeLeft,
        DeviceOrientation::LandscapeRight,
    ] {
        let mut rig = Rig::new();
        rig.act(ScreenAction::DeviceOrientationChanged(orientation));
        set_facts(&mut rig, facts_held(12_000.0, 0.0, WhiteOrigin::Manual));
        rig.run(700);
        let number = texts_of(&rig.last, "12000", TextRoleKind::StripValue);
        assert_eq!(number.len(), 1, "{orientation:?}");
        let bounds = text_bounds_px(&number[0]);
        let circle = rect_px(targets(&rig).button);
        let inset = px(2.0);
        assert!(
            bounds.0 >= circle.0 + inset
                && bounds.1 >= circle.1 + inset
                && bounds.2 <= circle.2 - inset
                && bounds.3 <= circle.3 - inset,
            "{orientation:?}: число {bounds:?} вылезает за круг {circle:?}"
        );
    }
}

#[test]
fn a_short_number_keeps_its_natural_size_and_a_long_one_is_compressed_evenly() {
    let mut rig = Rig::new();
    set_facts(&mut rig, facts_held(5_600.0, 0.0, WhiteOrigin::Manual));
    rig.run(300);
    let short = texts_of(&rig.last, "5600", TextRoleKind::StripValue);
    let short_scale = short[0].transform.a;
    set_facts(&mut rig, facts_held(12_000.0, 0.0, WhiteOrigin::Manual));
    rig.run(300);
    let long = texts_of(&rig.last, "12000", TextRoleKind::StripValue);
    assert!(
        long[0].transform.a < short_scale * 0.99,
        "пять цифр сжаты: {} против {short_scale}",
        long[0].transform.a
    );
    // Равномерно: высота сжимается так же, как ширина.
    assert!((long[0].transform.a - long[0].transform.d).abs() < 1e-3);
}

#[test]
fn the_number_turns_with_the_device() {
    let mut rig = Rig::new();
    rig.run(300);
    let portrait = texts_of(&rig.last, "5600", TextRoleKind::StripValue);
    assert!(
        portrait[0].transform.a.abs() > 0.9 && portrait[0].transform.b.abs() < 0.1,
        "{:?}",
        portrait[0].transform
    );
    rig.act(ScreenAction::DeviceOrientationChanged(DeviceOrientation::LandscapeLeft));
    rig.run(1_500);
    let landscape = texts_of(&rig.last, "5600", TextRoleKind::StripValue);
    assert!(
        landscape[0].transform.a.abs() < 0.1 && landscape[0].transform.b.abs() > 0.9,
        "число повёрнуто: {:?}",
        landscape[0].transform
    );
}

// ---- мерцание и тряска ----

fn number_x(rig: &Rig) -> f32 {
    texts_of(&rig.last, "5600", TextRoleKind::StripValue)[0].transform.tx
}

fn number_opacity(rig: &Rig) -> f32 {
    let drawn: Vec<TextItem> = rig
        .last
        .leaf_items()
        .filter_map(|item| match item {
            ui_draw::draw_list::DrawItem::Text(text) if text.run == run_of("5600", TextRoleKind::StripValue) => {
                Some(text.clone())
            }
            _ => None,
        })
        .collect();
    drawn[0].opacity.get()
}

/// Прозрачность круга-поверхности кнопки белого: поверхность с центром в центре кнопки (тряска сдвигает её
/// лишь на несколько pt, поэтому допуск — радиус кнопки).
fn button_glass_opacity(rig: &Rig) -> f32 {
    let center = white_button(rig);
    let (center_x, center_y) = (px(center.x), px(center.y));
    surface_shapes(&rig.last)
        .into_iter()
        .find_map(|shape| match shape.geometry {
            ui_draw::shape::ShapeGeometry::Circle { diameter }
                if (shape.transform.tx - center_x).abs() < diameter / 2.0
                    && (shape.transform.ty - center_y).abs() < diameter / 2.0 =>
            {
                Some(shape.opacity.get())
            }
            _ => None,
        })
        .expect("круг кнопки белого нарисован")
}

fn waiting_facts(take: Option<WhiteTakeState>) -> crate::facts::CameraFacts {
    facts_with(WhiteFacts {
        take: take.map(|state| WhiteTakeFacts { serial: 1, state }),
        ..WhiteFacts::STARTING
    })
}

#[test]
fn while_the_engine_waits_the_number_flickers_and_frames_are_requested_then_it_calms_down() {
    let mut rig = Rig::new();
    rig.run(300);
    assert!(!rig.screen.white_attention_animating(), "в покое кадры не нужны");
    assert!((number_opacity(&rig) - 1.0).abs() < 1e-6);
    set_facts(&mut rig, waiting_facts(Some(WhiteTakeState::Waiting)));
    let mut dimmest: f32 = 1.0;
    for _ in 0..80 {
        rig.run(FRAME_MS);
        assert!(rig.screen.white_attention_animating(), "пока ждём, кадры нужны");
        dimmest = dimmest.min(number_opacity(&rig));
        assert_eq!(button_glass_opacity(&rig), 1.0, "мерцает только число, стекло стоит");
    }
    assert!(dimmest < 0.6, "число мерцает: самая тусклая {dimmest}");
    set_facts(&mut rig, waiting_facts(Some(WhiteTakeState::Taken)));
    rig.run(600);
    assert!(
        !rig.screen.white_attention_animating(),
        "ожидание кончилось — кадры не нужны"
    );
    assert!(
        (number_opacity(&rig) - 1.0).abs() < 1e-6,
        "число вернулось к полной яркости"
    );
}

#[test]
fn a_refusal_shakes_the_number_sideways_and_then_it_comes_back() {
    let mut rig = Rig::new();
    rig.run(300);
    let rest = number_x(&rig);
    rig.act(ScreenAction::WhiteTakeNotDelivered);
    let mut widest: f32 = 0.0;
    for _ in 0..20 {
        rig.run(FRAME_MS);
        widest = widest.max((number_x(&rig) - rest).abs());
    }
    assert!(widest > px(2.0), "число тряслось: размах {widest} px");
    assert!(rig.screen.white_attention_animating() || widest > 0.0);
    rig.run(800);
    assert!((number_x(&rig) - rest).abs() < 0.01, "число вернулось на место");
    assert!(
        !rig.screen.white_attention_animating(),
        "тряска кончилась — кадры не нужны"
    );
}

#[test]
fn with_reduce_motion_a_refusal_does_not_move_the_number_but_dims_it() {
    let mut rig = Rig::with(ScreenConfig::default(), MotionPreference::Reduced);
    rig.run(300);
    let rest = number_x(&rig);
    rig.act(ScreenAction::WhiteTakeNotDelivered);
    let mut dimmest: f32 = 1.0;
    for _ in 0..12 {
        rig.run(FRAME_MS);
        assert!((number_x(&rig) - rest).abs() < 0.01, "при Reduce Motion число не едет");
        dimmest = dimmest.min(number_opacity(&rig));
    }
    assert!(dimmest < 0.9, "вместо тряски — затемнение: {dimmest}");
    rig.run(600);
    assert!((number_opacity(&rig) - 1.0).abs() < 1e-6);
}

#[test]
fn a_request_without_our_own_take_command_never_flickers_the_number() {
    // Заявка открытия камеры в фактах своей команды не оставляет (`take` пуст): без кадров камеры она висит,
    // и мерцало бы бесконечно.
    let mut rig = Rig::new();
    set_facts(&mut rig, waiting_facts(None));
    for _ in 0..80 {
        rig.run(FRAME_MS);
        assert!(!rig.screen.white_attention_animating());
        assert!((number_opacity(&rig) - 1.0).abs() < 1e-6);
    }
}

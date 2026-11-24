//! Раздел калибровки внутри безопасной зоны (14k, замечания 2 и 7 владельца, решение 226): пиксели surface → фильтр
//! системных жестов → жесты раздела → намерения библиотеке и меню. Числа полос — телефон владельца (полосы
//! обязательных жестов 156 px сверху и 132 px снизу; вырез 120 px внутри верхней).
//!
//! Палец здесь всегда проходит через фильтр: у каждого стенда стоят зоны системных жестов, поэтому орган управления,
//! оказавшийся в полосе, не «работает в тесте», а молчит, как на телефоне.

use look_contract::{CleanView, LookIntent, ToneBaseModel};
use shell_contract::edge_insets::{RawEdgeInsetsPx, ScreenEdge};
use shell_contract::orientation::DeviceOrientation;
use shell_contract::safe_area::SafeAreaInsets;
use shell_contract::system_gesture::SystemGestureZones;
use shell_contract::touch::TouchPhase;
use ui_core::geometry::{Point, Rect, Vector};

use super::screen_calibration_tests::{calibrating, center, drag_eye};
use super::test_rig::*;
use crate::action::ScreenAction;
use crate::calibration::{CalibrationLayout, CalibrationMenu, ControlViewKind};
use crate::config::ScreenConfig;
use crate::effect::Effect;
use crate::look_test_support::{look_facts, look_intents};
use crate::outcome::Rejection;

/// Полосы телефона владельца (`dumpsys window`, 24.11.2026).
const OWNER_INSETS: RawEdgeInsetsPx = RawEdgeInsetsPx {
    left: 0,
    top: 156,
    right: 0,
    bottom: 132,
};

/// Положения, в которых владелец держит раздел.
const HELD: [DeviceOrientation; 3] = [
    DeviceOrientation::Portrait,
    DeviceOrientation::LandscapeLeft,
    DeviceOrientation::LandscapeRight,
];

fn owner_zones() -> SystemGestureZones {
    SystemGestureZones::try_from_insets(OWNER_INSETS).expect("зоны владельца")
}

fn owner_safe_area() -> SafeAreaInsets {
    SafeAreaInsets::try_from_insets(OWNER_INSETS).expect("безопасная зона владельца")
}

/// Стенд телефона владельца: фильтр касаний есть, безопасная зона ещё НЕ сообщена (как до 14k).
fn phone_without_safe_area(orientation: DeviceOrientation) -> Rig {
    let mut rig = calibrating(look_facts(), orientation);
    rig.screen.set_system_gesture_zones(owner_zones());
    rig
}

/// Стенд телефона владельца целиком: фильтр касаний и безопасная зона.
fn phone(orientation: DeviceOrientation) -> Rig {
    let mut rig = phone_without_safe_area(orientation);
    rig.act(ScreenAction::SafeAreaChanged(owner_safe_area()));
    rig.run(400);
    rig.effects();
    rig
}

fn layout_of(rig: &Rig) -> CalibrationLayout {
    *rig.screen.state().calibration().layout().expect("раскладка раздела")
}

/// Безопасный прямоугольник поверхности, pt.
fn safe_surface_rect(rig: &Rig) -> Rect {
    let scale = rig.screen.state().pixels_per_point().expect("масштаб");
    let size = rig.screen.state().layout().expect("раскладка").screen_size();
    let inset = |edge| f64::from(owner_safe_area().thickness_px(edge)) / scale;
    Rect::from_xywh(
        inset(ScreenEdge::Left),
        inset(ScreenEdge::Top),
        size.width - inset(ScreenEdge::Left) - inset(ScreenEdge::Right),
        size.height - inset(ScreenEdge::Top) - inset(ScreenEdge::Bottom),
    )
}

fn assert_inside(outer: Rect, inner: Rect, what: &str) {
    const EPSILON: f64 = 1e-6;
    assert!(
        inner.min_x() >= outer.min_x() - EPSILON
            && inner.min_y() >= outer.min_y() - EPSILON
            && inner.max_x() <= outer.max_x() + EPSILON
            && inner.max_y() <= outer.max_y() + EPSILON,
        "{what}: {inner:?} вылезает за безопасную зону {outer:?}"
    );
}

/// Свайп по карусели основы на одну модель влево (глазами).
fn swipe_tone_base(rig: &mut Rig) -> Vec<LookIntent> {
    let carousel = center(rig, "calib_base_carousel", 0);
    drag_eye(rig, carousel, Vector::new(-60.0, 0.0));
    look_intents(&rig.effects()).into_iter().cloned().collect()
}

#[test]
fn without_the_safe_area_the_gesture_band_swallows_the_tone_base_swipe() {
    // Причина замечания 2: раздел с y = 0, палец по AGX начинается в полосе шторки и принадлежит системе.
    let mut rig = phone_without_safe_area(DeviceOrientation::Portrait);
    let carousel = center(&rig, "calib_base_carousel", 0);
    assert!(
        carousel.y * f64::from(px(1.0)) < 156.0,
        "карусель стояла в полосе: {carousel:?}"
    );
    assert_eq!(swipe_tone_base(&mut rig), Vec::new());
    assert_eq!(rig.screen.diagnostics().touches_ignored_in_system_gesture_zone, 1);
}

#[test]
fn the_tone_base_is_switched_by_a_finger_on_the_owner_phone_in_every_held_position() {
    for orientation in HELD {
        let mut rig = phone(orientation);
        assert_eq!(
            swipe_tone_base(&mut rig),
            vec![LookIntent::SetToneBase(ToneBaseModel::Aces)],
            "{orientation:?}"
        );
        assert_eq!(
            rig.screen.diagnostics().touches_ignored_in_system_gesture_zone,
            0,
            "{orientation:?}: палец по карусели не в полосе системы"
        );
    }
}

#[test]
fn a_finger_that_starts_in_the_band_stays_with_the_system_and_changes_nothing() {
    let mut rig = phone(DeviceOrientation::Portrait);
    let carousel = center(&rig, "calib_base_carousel", 0);
    let before = rig.screen.state().clone();
    drag_eye(&mut rig, Point::new(carousel.x, 20.0), Vector::new(-60.0, 0.0));
    assert_eq!(look_intents(&rig.effects()), Vec::<&LookIntent>::new());
    assert_eq!(rig.screen.diagnostics().touches_ignored_in_system_gesture_zone, 1);
    assert_eq!(rig.screen.state().mode(), before.mode());
    assert_eq!(rig.screen.state().calibration().menu(), CalibrationMenu::Closed);
}

#[test]
fn the_column_the_scope_and_the_menu_button_stay_inside_the_safe_area_in_every_held_position() {
    for orientation in HELD {
        let rig = phone(orientation);
        let safe = safe_surface_rect(&rig);
        let layout = layout_of(&rig);
        assert_inside(safe, layout.eye_rect_on_surface(layout.zones().knobs), "колонка ручек");
        assert_inside(safe, layout.eye_rect_on_surface(layout.zones().scope), "прибор");
        let diameter = ScreenConfig::default().widgets.bottom.menu_button.diameter_pt;
        let button = layout.eye_point_on_surface(layout.menu_button_center());
        assert_inside(
            safe,
            Rect::centered_at(button, ui_core::geometry::Size::new(diameter, diameter)),
            "кнопка меню",
        );
    }
}

#[test]
fn every_control_of_both_views_is_reachable_by_a_finger_in_every_held_position() {
    // Узлы, которые нажимают или ведут пальцем (контейнеры и капсула режимов на месте PHOTO, решение 82, — не здесь).
    const SCALES: [&str; 6] = [
        "calib_base_carousel",
        "calib_base_reset",
        "calib_clean",
        "calib_view_segment",
        "calib_scope",
        "menu_button",
    ];
    const WHEELS: [&str; 4] = [
        "calib_wheel_tab",
        "calib_wheel_pad",
        "calib_wheel_reset_whole",
        "calib_wheel_reset_push",
    ];
    for orientation in HELD {
        let mut rig = phone(orientation);
        let safe = safe_surface_rect(&rig);
        let check = |rig: &Rig, labels: &[&str]| {
            for label in labels {
                let centers = rig.screen.node_centers(label);
                assert!(!centers.is_empty(), "{orientation:?}: нет узла {label}");
                for point in centers {
                    assert!(
                        safe.contains(point),
                        "{orientation:?}: {label} в полосе системы: {point:?}"
                    );
                }
            }
        };
        check(&rig, &SCALES);
        rig.act(ScreenAction::ControlViewSelected(ControlViewKind::Wheels));
        rig.run(400);
        check(&rig, &WHEELS);
    }
}

#[test]
fn the_clean_button_and_the_menu_button_answer_a_finger_in_every_held_position() {
    for orientation in HELD {
        let mut rig = phone(orientation);
        let clean = center(&rig, "calib_clean", 0);
        let start = rig.now_ms;
        rig.touch(TouchPhase::Began, clean, start);
        rig.run(300);
        assert!(
            rig.effects()
                .iter()
                .any(|effect| matches!(effect, Effect::CleanView(CleanView::ShowBase))),
            "{orientation:?}: CLEAN под пальцем"
        );
        let end = rig.now_ms;
        rig.touch(TouchPhase::Ended, clean, end);
        rig.run(300);

        let button = center(&rig, "menu_button", 0);
        rig.tap(button);
        rig.run(700);
        assert_eq!(
            rig.screen.state().calibration().menu(),
            CalibrationMenu::Root,
            "{orientation:?}: «шесть точек» открывает меню"
        );
    }
}

#[test]
fn in_landscape_the_menu_button_used_to_sit_in_the_system_band() {
    // Без безопасной зоны (как до 14k) кнопка в альбоме правым боком лежит в полосе шторки и не отвечает.
    let mut rig = phone_without_safe_area(DeviceOrientation::LandscapeRight);
    let button = center(&rig, "menu_button", 0);
    rig.tap(button);
    rig.run(700);
    assert_eq!(rig.screen.state().calibration().menu(), CalibrationMenu::Closed);
    assert_eq!(rig.screen.diagnostics().touches_ignored_in_system_gesture_zone, 1);
}

#[test]
fn a_late_safe_area_rebuilds_the_section_and_reorders_the_scope_picture() {
    let mut rig = phone_without_safe_area(DeviceOrientation::LandscapeLeft);
    rig.effects();
    let before = layout_of(&rig);
    rig.act(ScreenAction::SafeAreaChanged(owner_safe_area()));
    rig.run(200);
    let after = layout_of(&rig);
    assert_ne!(before, after, "раскладка пересчитана");
    // Прибор стал уже: рендеру ушёл новый запрос ровно на новый размер зоны.
    let sizes: Vec<_> = rig
        .effects()
        .into_iter()
        .filter_map(|effect| match effect {
            Effect::Scopes(scope_contract::ScopeIntent::Show(request)) => Some(request.image_size),
            _ => None,
        })
        .collect();
    assert_eq!(sizes, vec![after.scope_image_size()]);
    assert!(after.scope_image_size().width() < before.scope_image_size().width());
    // Узлы на новых местах: палец по карусели доходит.
    assert_eq!(
        swipe_tone_base(&mut rig),
        vec![LookIntent::SetToneBase(ToneBaseModel::Aces)]
    );
}

#[test]
fn photo_does_not_depend_on_the_safe_area() {
    let mut plain = Rig::new();
    let expected = plain.frame().clone();
    let mut rig = Rig::new();
    rig.frame();
    rig.act(ScreenAction::SafeAreaChanged(owner_safe_area()));
    assert_eq!(*rig.frame(), expected, "экран PHOTO побайтно тот же");
    assert!(rig.effects().is_empty(), "ни камере, ни рендеру ничего не ушло");
}

#[test]
fn insets_larger_than_the_screen_are_one_counted_refusal_and_a_good_area_brings_the_section_back() {
    let mut rig = calibrating(look_facts(), DeviceOrientation::Portrait);
    let refused = |rig: &Rig| rig.screen.state().rejections().get(Rejection::InvalidSurface);
    let before = refused(&rig);
    let garbage = SafeAreaInsets::try_from_insets(RawEdgeInsetsPx {
        left: 0,
        top: 5000,
        right: 0,
        bottom: 0,
    })
    .expect("число неотрицательное");
    rig.act(ScreenAction::SafeAreaChanged(garbage));
    rig.run(100);
    assert!(rig.screen.state().calibration().layout().is_none());
    assert_eq!(refused(&rig), before + 1, "отказ посчитан один раз, а не каждый тик");

    rig.act(ScreenAction::SafeAreaChanged(owner_safe_area()));
    rig.run(200);
    assert!(rig.screen.state().calibration().layout().is_some());
    assert!(!rig.screen.node_centers("calib_base_carousel").is_empty());
}

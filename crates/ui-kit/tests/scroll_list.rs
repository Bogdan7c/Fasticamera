#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Свободный список: палец вверх увозит содержимое, остановка — на месте, за краем — «резинка», бросок катится.

use ui_core::geometry::Vector;
use ui_core::gesture::PanTranslation;
use ui_core::time::MotionPreference;
use ui_kit::pan::PanUpdate;
use ui_kit::scroll_list::{ScrollAxis, ScrollList};
use ui_kit::tokens::UiKitTokens;

mod common;
use common::{VIEWPORT_POINTS, env, ms};

fn finger_pan(dy: f64) -> PanTranslation {
    PanTranslation {
        from_touch_down: Vector::new(0.0, dy),
        from_recognition: Vector::new(0.0, dy),
    }
}

fn list(content: f64, tokens: &UiKitTokens) -> ScrollList {
    ScrollList::new(content, &env(tokens, MotionPreference::Full), ms(0)).unwrap()
}

#[test]
fn finger_up_scrolls_the_content_by_the_finger_distance() {
    let tokens = UiKitTokens::default();
    let mut list = list(VIEWPORT_POINTS * 3.0, &tokens);
    list.apply_pan(
        ms(0),
        PanUpdate::Began {
            translation: finger_pan(0.0),
        },
    )
    .unwrap();
    list.apply_pan(
        ms(16),
        PanUpdate::Changed {
            translation: finger_pan(-120.0),
        },
    )
    .unwrap();
    assert!(
        (list.offset(ms(16)) - 120.0).abs() < 1e-6,
        "палец вверх на 120 pt = смещение +120"
    );
}

#[test]
fn list_that_fits_has_no_scroll_range() {
    let tokens = UiKitTokens::default();
    let list = list(VIEWPORT_POINTS * 0.5, &tokens);
    assert_eq!(list.max_offset(), 0.0);
}

#[test]
fn release_with_speed_coasts_further_and_stays_inside_the_range() {
    let tokens = UiKitTokens::default();
    let mut list = list(VIEWPORT_POINTS * 3.0, &tokens);
    list.apply_pan(
        ms(0),
        PanUpdate::Began {
            translation: finger_pan(0.0),
        },
    )
    .unwrap();
    list.apply_pan(
        ms(16),
        PanUpdate::Changed {
            translation: finger_pan(-50.0),
        },
    )
    .unwrap();
    let at_release = list.offset(ms(16));
    list.apply_pan(
        ms(32),
        PanUpdate::Ended {
            velocity: Vector::new(0.0, -1500.0),
        },
    )
    .unwrap();
    let later = list.offset(ms(2000));
    assert!(
        later > at_release + 50.0,
        "бросок прокатил дальше: {at_release} -> {later}"
    );
    assert!(
        later <= list.max_offset() + 1e-6,
        "бросок не выходит за предел: {later}"
    );
}

#[test]
fn pull_past_the_edge_returns_to_the_edge() {
    let tokens = UiKitTokens::default();
    let mut list = list(VIEWPORT_POINTS * 2.0, &tokens);
    list.apply_pan(
        ms(0),
        PanUpdate::Began {
            translation: finger_pan(0.0),
        },
    )
    .unwrap();
    list.apply_pan(
        ms(16),
        PanUpdate::Changed {
            translation: finger_pan(80.0),
        },
    )
    .unwrap();
    assert!(list.offset(ms(16)) < 0.0, "за верхним краем — «резинка»");
    list.apply_pan(ms(32), PanUpdate::Ended { velocity: Vector::ZERO })
        .unwrap();
    assert!(list.offset(ms(3000)).abs() < 1e-3, "вернулся к верху");
}

#[test]
fn content_growth_extends_the_scroll_range() {
    let tokens = UiKitTokens::default();
    let mut list = list(VIEWPORT_POINTS * 2.0, &tokens);
    assert!((list.max_offset() - VIEWPORT_POINTS).abs() < 1e-9);
    list.set_content_extent(VIEWPORT_POINTS * 3.0, ms(0)).unwrap();
    assert!((list.max_offset() - VIEWPORT_POINTS * 2.0).abs() < 1e-9);
}

fn finger_pan_horizontal(dx: f64) -> PanTranslation {
    PanTranslation {
        from_touch_down: Vector::new(dx, 0.0),
        from_recognition: Vector::new(dx, 0.0),
    }
}

#[test]
fn horizontal_list_follows_a_finger_moving_left_and_ignores_vertical_motion() {
    let tokens = UiKitTokens::default();
    let mut list = ScrollList::along(
        ScrollAxis::Horizontal,
        VIEWPORT_POINTS * 3.0,
        0.0,
        &env(&tokens, MotionPreference::Full),
        ms(0),
    )
    .unwrap();
    list.apply_pan(
        ms(0),
        PanUpdate::Began {
            translation: finger_pan_horizontal(0.0),
        },
    )
    .unwrap();
    list.apply_pan(
        ms(16),
        PanUpdate::Changed {
            translation: finger_pan_horizontal(-80.0),
        },
    )
    .unwrap();
    assert!(
        (list.offset(ms(16)) - 80.0).abs() < 1e-6,
        "палец влево на 80 pt = смещение +80"
    );
    // Вертикальная составляющая смещения горизонтальный список не двигает.
    list.apply_pan(
        ms(32),
        PanUpdate::Changed {
            translation: PanTranslation {
                from_touch_down: Vector::new(-80.0, 200.0),
                from_recognition: Vector::new(-80.0, 200.0),
            },
        },
    )
    .unwrap();
    assert!(
        (list.offset(ms(32)) - 80.0).abs() < 1e-6,
        "вертикаль не влияет: {}",
        list.offset(ms(32))
    );
}

#[test]
fn list_starts_at_the_requested_offset_clamped_to_the_range() {
    let tokens = UiKitTokens::default();
    let environment = env(&tokens, MotionPreference::Full);
    let content = VIEWPORT_POINTS * 2.0;
    let inside = ScrollList::along(ScrollAxis::Horizontal, content, 120.0, &environment, ms(0)).unwrap();
    assert!((inside.offset(ms(0)) - 120.0).abs() < 1e-6);
    let beyond = ScrollList::along(ScrollAxis::Horizontal, content, 10_000.0, &environment, ms(0)).unwrap();
    assert!(
        (beyond.offset(ms(0)) - VIEWPORT_POINTS).abs() < 1e-6,
        "зажато в максимум: {}",
        beyond.offset(ms(0))
    );
    let negative = ScrollList::along(ScrollAxis::Vertical, content, -50.0, &environment, ms(0)).unwrap();
    assert_eq!(negative.offset(ms(0)), 0.0);
}

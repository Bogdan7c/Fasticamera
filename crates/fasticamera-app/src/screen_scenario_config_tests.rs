//! Проверка секции `camera.scenarios.screen`: умолчания верны, каждое неверное значение отвергается со своей
//! причиной, план касаний геометрически осмыслен.

use super::*;

#[test]
fn default_config_is_valid_and_budget_is_half_of_the_frame_period() {
    let config = ScreenScenarioConfig::default();
    assert_eq!(config.validate(), Ok(()));
    let budget = config.ui_gpu_budget_ms(60.0);
    // Решение 90: половина кадра при 60 Гц — ловушка грубых ошибок, а не мера цены.
    assert!((budget - 8.3333).abs() < 1e-3, "{budget}");
}

#[test]
fn every_default_touch_point_is_a_fraction_of_the_screen() {
    for (name, point) in ScreenTouchPlan::default().named_points() {
        assert!(
            (0.0..=1.0).contains(&point.x) && (0.0..=1.0).contains(&point.y),
            "{name}: {point:?}"
        );
    }
}

#[test]
fn a_point_outside_the_screen_is_refused_naming_the_gesture() {
    let mut config = ScreenScenarioConfig::default();
    config.touch.ev_icon = ScreenPoint::new(1.2, 0.1);
    assert_eq!(
        config.validate(),
        Err(ScreenScenarioConfigError::PointOutOfRange {
            gesture: "ev_icon",
            x: 1.2,
            y: 0.1
        })
    );
    let mut nan = ScreenScenarioConfig::default();
    nan.touch.menu_open_swipe.to = ScreenPoint::new(f64::NAN, 0.5);
    assert!(matches!(
        nan.validate(),
        Err(ScreenScenarioConfigError::PointOutOfRange {
            gesture: "menu_open_swipe",
            ..
        })
    ));
}

#[test]
fn zero_durations_and_budget_out_of_range_are_refused() {
    let mut config = ScreenScenarioConfig::default();
    config.touch.lock_press.hold_ms = 0;
    assert_eq!(
        config.validate(),
        Err(ScreenScenarioConfigError::ZeroGestureDuration { gesture: "lock_press" })
    );
    let mut swipe = ScreenScenarioConfig::default();
    swipe.touch.modes_swipe_left.duration_ms = 0;
    assert_eq!(
        swipe.validate(),
        Err(ScreenScenarioConfigError::ZeroGestureDuration {
            gesture: "modes_swipe_left"
        })
    );
    let zero = ScreenScenarioConfig {
        step_timeout_ms: 0,
        ..ScreenScenarioConfig::default()
    };
    assert_eq!(
        zero.validate(),
        Err(ScreenScenarioConfigError::Zero {
            field: "step_timeout_ms"
        })
    );
    for value in [0.0, -0.1, 1.5, f64::NAN] {
        let config = ScreenScenarioConfig {
            ui_gpu_budget_fraction: value,
            ..ScreenScenarioConfig::default()
        };
        assert!(
            matches!(
                config.validate(),
                Err(ScreenScenarioConfigError::BudgetFractionOutOfRange { .. })
            ),
            "{value}"
        );
    }
}

#[test]
fn unknown_fields_are_refused_and_partial_json_keeps_defaults() {
    assert!(serde_json::from_str::<ScreenScenarioConfig>(r#"{"typo": 1}"#).is_err());
    assert!(serde_json::from_str::<ScreenScenarioConfig>(r#"{"touch": {"typo": 1}}"#).is_err());
    let parsed: ScreenScenarioConfig = serde_json::from_str(r#"{"step_timeout_ms": 900}"#).expect("разбор");
    assert_eq!(parsed.step_timeout_ms, 900);
    assert_eq!(parsed.touch, ScreenTouchPlan::default());
    assert_eq!(parsed.warmup_ui_frames, 30);
}

#[test]
fn focus_targets_are_consistent_with_the_screen_points_by_the_real_layout() {
    use camera_screen::settings::AspectRatio;
    use camera_screen::{LayoutConfig, ScreenLayout};
    // Экран OnePlus 11 из доки плана касаний: 1440 × 3216 px.
    let layout = ScreenLayout::new(
        &LayoutConfig::default(),
        shell_contract::surface::SurfaceSize {
            width_px: 1440,
            height_px: 3216,
        },
    )
    .expect("раскладка");
    let frame = layout.frame_rect(AspectRatio::FourByThree);
    let screen = layout.screen_size();
    let plan = ScreenTouchPlan::default();
    for (name, tap, target) in [
        ("center", plan.focus_center, plan.focus_targets.center),
        ("top_left", plan.focus_top_left, plan.focus_targets.top_left),
        ("bottom_right", plan.focus_bottom_right, plan.focus_targets.bottom_right),
    ] {
        let u = (tap.x * screen.width - frame.min_x()) / frame.size.width;
        let v = (tap.y * screen.height - frame.min_y()) / frame.size.height;
        assert!(
            (u - target.x).abs() < 0.01 && (v - target.y).abs() < 0.01,
            "{name}: тап в кадре ({u}, {v}), цель {target:?}"
        );
    }
}

#[test]
fn a_focus_target_outside_the_frame_or_a_bad_tolerance_is_refused() {
    let mut config = ScreenScenarioConfig::default();
    config.touch.focus_targets.top_left = ScreenPoint::new(-0.1, 0.5);
    assert!(matches!(
        config.validate(),
        Err(ScreenScenarioConfigError::PointOutOfRange {
            gesture: "focus_targets.top_left",
            ..
        })
    ));
    for bad in [0.0, -0.1, 0.5, 1.0, f64::NAN] {
        let mut config = ScreenScenarioConfig::default();
        config.touch.focus_point_tolerance = bad;
        assert!(
            matches!(
                config.validate(),
                Err(ScreenScenarioConfigError::FocusToleranceOutOfRange { .. })
            ),
            "{bad}"
        );
    }
}

#[test]
fn the_ev_icon_point_follows_the_strip_layout_formula_for_a_row_of_three() {
    // Ряд по умолчанию со вспышкой: Flash, Raw, Exposure (таймер выключен). Центр значка = центр ряда +
    // (индекс − (число − 1)/2) · шаг.
    let strip = camera_screen::widgets_config::StripLayout::default();
    let offset_from_middle = 2.0 - 2.0 / 2.0;
    let ev_x_fraction = (strip.center_x_pt + offset_from_middle * strip.pitch_pt) / 440.0;
    let plan = ScreenTouchPlan::default();
    assert!((plan.ev_icon.x - ev_x_fraction).abs() < 5e-4, "{:?}", plan.ev_icon);
}

#[test]
fn the_white_button_point_follows_the_layout_offset_from_the_shutter() {
    let widgets = camera_screen::widgets_config::WidgetLayoutConfig::default();
    let expected_x = (widgets.bottom.shutter_center_x_pt + widgets.white_button.offset_from_shutter_pt) / 440.0;
    let plan = ScreenTouchPlan::default();
    assert!(
        (plan.white_button.x - expected_x).abs() < 5e-4,
        "{:?}",
        plan.white_button
    );
    assert_eq!(
        plan.white_hold.at, plan.white_button,
        "долгое нажатие целится в ту же кнопку"
    );
}

#[test]
fn a_white_hold_without_duration_is_refused_naming_the_gesture() {
    let mut config = ScreenScenarioConfig::default();
    config.touch.white_hold.hold_ms = 0;
    assert_eq!(
        config.validate(),
        Err(ScreenScenarioConfigError::ZeroGestureDuration { gesture: "white_hold" })
    );
}

#[test]
fn white_points_outside_the_screen_are_refused_naming_the_gesture() {
    let mut config = ScreenScenarioConfig::default();
    config.touch.white_light = ScreenPoint::new(0.3, 1.5);
    assert_eq!(
        config.validate(),
        Err(ScreenScenarioConfigError::PointOutOfRange {
            gesture: "white_light",
            x: 0.3,
            y: 1.5
        })
    );
}

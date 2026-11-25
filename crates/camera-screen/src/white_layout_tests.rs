//! Раскладка карточки белого: числа по умолчанию годны, а любая ошибка конфига ловится при проверке, а не на экране.

use super::*;
use crate::config::ScreenConfig;
use crate::widgets_config::CardLayout;

fn card() -> CardLayout {
    CardLayout::default()
}

#[test]
fn the_default_layout_is_valid_and_the_card_is_exactly_24_pt_taller() {
    let layout = WhiteCardLayout::default();
    assert_eq!(layout.validate(&card()), Ok(()));
    let height = layout.card_height_pt(&card());
    assert!((height - 207.1).abs() < 1e-9, "высота {height}");
    assert!((layout.raised_shutter_extra_pt + card().raised_shutter_offset_pt - 124.0).abs() < 1e-9);
}

#[test]
fn the_whole_screen_config_with_the_default_white_layout_validates() {
    assert_eq!(ScreenConfig::default().validate(), Ok(()));
}

#[test]
fn the_ruler_zone_ends_above_the_system_gesture_band_with_every_row_at_least_44_pt() {
    let layout = WhiteCardLayout::default();
    let card = card();
    let ruler_bottom = layout.ruler_center_from_top_pt + layout.ruler_touch_height_pt * 0.5;
    // Расстояние от низа зоны шкалы до низа экрана.
    let from_screen_bottom = layout.card_height_pt(&card) + card.bottom_inset_pt - ruler_bottom;
    assert!(
        from_screen_bottom >= layout.system_gesture_band_pt,
        "{from_screen_bottom} < {}",
        layout.system_gesture_band_pt
    );
    for height in [
        layout.axis_touch_height_pt,
        layout.lights_touch_height_pt,
        layout.ruler_touch_height_pt,
    ] {
        assert!(height >= 44.0, "{height}");
    }
}

#[test]
fn a_ruler_that_reaches_into_the_gesture_band_is_refused() {
    let layout = WhiteCardLayout {
        ruler_center_from_top_pt: 150.0,
        ..WhiteCardLayout::default()
    };
    assert!(matches!(
        layout.validate(&card()),
        Err(WhiteLayoutError::RulerInGestureBand { .. })
    ));
}

#[test]
fn overlapping_short_or_off_card_rows_are_refused() {
    let overlap = WhiteCardLayout {
        lights_row_center_from_top_pt: 60.0,
        ..WhiteCardLayout::default()
    };
    assert!(matches!(
        overlap.validate(&card()),
        Err(WhiteLayoutError::RowsOverlap {
            upper: "числа осей",
            ..
        })
    ));
    let short = WhiteCardLayout {
        axis_touch_height_pt: 30.0,
        ..WhiteCardLayout::default()
    };
    assert!(matches!(
        short.validate(&card()),
        Err(WhiteLayoutError::TouchTooShort {
            row: "числа осей", ..
        })
    ));
    let above = WhiteCardLayout {
        axis_row_center_from_top_pt: 10.0,
        ..WhiteCardLayout::default()
    };
    assert!(matches!(
        above.validate(&card()),
        Err(WhiteLayoutError::RowAboveCard {
            row: "числа осей", ..
        })
    ));
}

#[test]
fn a_shutter_raised_less_than_the_card_grew_is_refused() {
    let layout = WhiteCardLayout {
        raised_shutter_extra_pt: 10.0,
        ..WhiteCardLayout::default()
    };
    assert!(matches!(
        layout.validate(&card()),
        Err(WhiteLayoutError::ShutterCollides { .. })
    ));
}

#[test]
fn axis_numbers_that_do_not_fit_between_the_side_buttons_are_refused() {
    let layout = WhiteCardLayout {
        axis_touch_width_pt: 200.0,
        ..WhiteCardLayout::default()
    };
    assert!(matches!(
        layout.validate(&card()),
        Err(WhiteLayoutError::AxisButtonsTooWide { .. })
    ));
}

#[test]
fn numbers_must_be_finite_and_positive_and_the_echo_memory_non_empty() {
    for broken in [
        WhiteCardLayout {
            light_pitch_pt: f64::NAN,
            ..WhiteCardLayout::default()
        },
        WhiteCardLayout {
            echo_memory_ms: 0.0,
            ..WhiteCardLayout::default()
        },
        WhiteCardLayout {
            ruler_center_from_top_pt: f64::INFINITY,
            ..WhiteCardLayout::default()
        },
    ] {
        assert!(broken.validate(&card()).is_err(), "{broken:?}");
    }
    let empty = WhiteCardLayout {
        echo_memory_len: 0,
        ..WhiteCardLayout::default()
    };
    assert_eq!(empty.validate(&card()), Err(WhiteLayoutError::EmptyEchoMemory));
}

#[test]
fn the_button_row_must_fit_the_card_width_without_scrolling() {
    let layout = WhiteCardLayout::default();
    assert_eq!(layout.check_buttons_fit(6, &card()), Ok(()));
    assert!(matches!(
        layout.check_buttons_fit(7, &card()),
        Err(WhiteLayoutError::LightsTooWide { count: 7, .. })
    ));
}

#[test]
fn a_broken_white_layout_fails_the_whole_screen_config() {
    let mut config = ScreenConfig::default();
    config.widgets.white_card.light_pitch_pt = 90.0;
    assert!(config.validate().is_err());
}

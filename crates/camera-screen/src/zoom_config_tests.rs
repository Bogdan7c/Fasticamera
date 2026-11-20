use super::*;

#[test]
fn default_zoom_config_is_valid_and_carries_the_measured_numbers() {
    let config = ZoomConfig::default();
    config.validate().expect("умолчания верны");
    // Числа ленты, колеса и кнопки переворота — из замера R00 (`layout.md` 2.2, `motion.md`).
    assert_eq!(config.strip.pitch_pt, 39.0);
    assert_eq!(config.strip.selected_x_pt, 220.0);
    assert_eq!(config.strip.center_y_pt, 709.9);
    assert_eq!(config.strip.selected_circle_diameter_pt, 37.0);
    assert_eq!((config.wheel.center_x_pt, config.wheel.center_y_pt), (219.7, 848.9));
    assert_eq!(config.wheel.idle_dismiss_ms, 2150.0);
    assert_eq!(config.wheel.dismiss_drop_pt, 33.0);
    assert_eq!(config.wheel.appear_rise_pt, 10.7);
    assert_eq!(config.wheel.fine_tick_until, 4.4);
    assert_eq!(config.motion.glide_ms, 270.0);
    assert_eq!(
        (
            config.flip_button.center_x_pt,
            config.flip_button.center_y_pt,
            config.flip_button.diameter_pt
        ),
        (382.0, 898.0, 48.0)
    );
}

#[test]
fn zoom_config_round_trips_through_json_and_rejects_unknown_fields() {
    let config = ZoomConfig::default();
    let json = serde_json::to_string(&config).expect("сериализация");
    let back: ZoomConfig = serde_json::from_str(&json).expect("разбор");
    assert_eq!(back, config);
    assert!(serde_json::from_str::<ZoomConfig>(r#"{"unknown_field": 1}"#).is_err());
    assert!(serde_json::from_str::<ZoomConfig>(r#"{"wheel": {"idle_dismiss_ms": 1000, "zzz": 1}}"#).is_err());
    // Частичный конфиг дополняется умолчаниями.
    let partial: ZoomConfig = serde_json::from_str(r#"{"motion": {"glide_ms": 400}}"#).expect("частичный");
    assert_eq!(partial.motion.glide_ms, 400.0);
    assert_eq!(partial.strip.pitch_pt, 39.0);
}

#[test]
fn nonsense_numbers_are_rejected_with_the_field_name() {
    let mut config = ZoomConfig::default();
    config.motion.glide_ms = 0.0;
    assert!(matches!(
        config.validate(),
        Err(ZoomConfigError::NotPositive {
            field: "motion.glide_ms",
            ..
        })
    ));
    let mut config = ZoomConfig::default();
    config.wheel.center_y_pt = f64::NAN;
    assert!(matches!(
        config.validate(),
        Err(ZoomConfigError::NotFinite {
            field: "wheel.center_y_pt",
            ..
        })
    ));
    let mut config = ZoomConfig::default();
    config.strip.max_buttons = 0;
    assert!(matches!(config.validate(), Err(ZoomConfigError::Inconsistent { .. })));
    let mut config = ZoomConfig::default();
    config.transition.min_card_width_fraction = 1.5;
    assert!(config.validate().is_err());
    let config = ZoomConfig {
        flip_button_fill_rgb: [0.1, 2.0, 0.1],
        ..ZoomConfig::default()
    };
    assert!(config.validate().is_err());
    let mut config = ZoomConfig::default();
    config.wheel.bright_tick_values.push(-1.0);
    assert!(config.validate().is_err());
}

#[test]
fn screen_config_validation_covers_the_zoom_section() {
    let mut config = crate::ScreenConfig::default();
    config.zoom.transition.blur_fade_out_ms = -5.0;
    assert!(matches!(config.validate(), Err(crate::ScreenConfigError::Zoom(_))));
}

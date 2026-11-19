#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Токены K7–K10, типографики и цветов: значения по спецификации, проверка, serde.

use ui_kit::tokens::{TextRoleKind, TokenError, Tracking, UiKitTokens};

#[test]
fn defaults_follow_the_measured_numbers_and_validate() {
    let tokens = UiKitTokens::default();
    tokens.validate().unwrap();
    // layout.md 2.2 (видео A).
    assert_eq!(
        (tokens.focus.frame_width_pt, tokens.focus.frame_height_pt),
        (91.0, 92.0)
    );
    assert_eq!((tokens.focus.sun_width_pt, tokens.focus.sun_height_pt), (30.0, 31.0));
    assert_eq!(tokens.focus.sun_offset_right_of_frame_pt, 21.5);
    assert_eq!(tokens.focus.sun_offset_above_center_pt, 5.0);
    assert_eq!((tokens.banner.height_pt, tokens.banner.min_width_pt), (30.0, 117.0));
    assert_eq!(
        (tokens.shutter.outer_diameter_pt, tokens.shutter.disc_diameter_pt),
        (80.1, 67.9)
    );
}

#[test]
fn mode_label_size_is_derived_from_the_measured_cap_height() {
    let tokens = UiKitTokens::default();
    // 14,2 pt заглавных / 0,7275 (высота заглавных Inter, em) ≈ 19,5 pt.
    let derived = 14.2 / (1490.0 / 2048.0);
    assert!((f64::from(tokens.typography.mode_label.size_pt) - derived).abs() < 0.1);
    let style = tokens.typography.role(TextRoleKind::ModeLabel).style(3.0);
    assert!((style.size_px - 19.5 * 3.0).abs() < 1e-3);
    assert_eq!(
        tokens.typography.role(TextRoleKind::ScaleValue).style(3.0).figures,
        ui_draw::text::Figures::Tabular
    );
}

#[test]
fn tracking_comes_from_the_hig_table_or_a_fixed_value() {
    let mut tokens = UiKitTokens::default();
    let from_table = tokens.typography.caption.style(1.0).tracking_em;
    assert_eq!(from_table, ui_draw::text::tracking::sf_pro_tracking_em(12.0));
    tokens.typography.caption.tracking = Tracking::FixedEm { em: 0.05 };
    assert_eq!(tokens.typography.caption.style(1.0).tracking_em, 0.05);
}

#[test]
fn broken_tokens_are_reported_by_name() {
    let mut tokens = UiKitTokens::default();
    tokens.focus.hide_after_s = tokens.focus.dim_after_s;
    assert!(matches!(
        tokens.validate(),
        Err(TokenError::OutOfRange {
            name: "focus.hide_after_s",
            ..
        })
    ));

    let mut tokens = UiKitTokens::default();
    tokens.shutter.disc_diameter_pt = tokens.shutter.outer_diameter_pt;
    assert!(matches!(
        tokens.validate(),
        Err(TokenError::OutOfRange {
            name: "shutter.disc_diameter_pt",
            ..
        })
    ));

    let mut tokens = UiKitTokens::default();
    tokens.typography.menu_item.size_pt = 0.0;
    assert!(matches!(
        tokens.validate(),
        Err(TokenError::NonPositiveSize {
            name: "typography.menu_item",
            ..
        })
    ));

    let mut tokens = UiKitTokens::default();
    tokens.colors.scrim.a = 1.5;
    assert_eq!(tokens.validate(), Err(TokenError::InvalidColor { name: "scrim" }));

    let mut tokens = UiKitTokens::default();
    tokens.panel.content_reveal_start = 1.0;
    assert!(matches!(
        tokens.validate(),
        Err(TokenError::OutOfRange {
            name: "panel.content_reveal_start",
            ..
        })
    ));
}

#[test]
fn tokens_survive_a_json_round_trip_and_reject_unknown_fields() {
    let tokens = UiKitTokens::default();
    let json = serde_json::to_string(&tokens).unwrap();
    let back: UiKitTokens = serde_json::from_str(&json).unwrap();
    assert_eq!(back, tokens);
    let partial: UiKitTokens = serde_json::from_str(r#"{"banner":{"height_pt":34.0}}"#).unwrap();
    assert_eq!(partial.banner.height_pt, 34.0);
    assert_eq!(partial.banner.min_width_pt, 117.0, "остальное — умолчания");
    assert!(serde_json::from_str::<UiKitTokens>(r#"{"banner":{"heigth_pt":34.0}}"#).is_err());
}

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Токены K7–K10, типографики и цветов: значения по спецификации, проверка, serde.

use ui_kit::tokens::{TextRoleKind, TextWeight, TokenError, Tracking, UiKitTokens};

#[test]
fn defaults_follow_the_measured_numbers_and_validate() {
    let tokens = UiKitTokens::default();
    tokens.validate().unwrap();
    // Замеры R00 t=99.05–109.8 (3 px = 1 pt).
    assert_eq!(
        (tokens.focus.frame_width_pt, tokens.focus.frame_height_pt),
        (74.7, 74.7)
    );
    assert_eq!((tokens.focus.stroke_width_pt, tokens.focus.tick_length_pt), (1.0, 5.5));
    assert_eq!(tokens.focus.sun_disc_diameter_pt, 9.0);
    assert_eq!(tokens.focus.sun_ray_count, 8);
    assert_eq!(
        (
            tokens.focus.sun_ray_inner_radius_pt,
            tokens.focus.sun_ray_outer_radius_pt
        ),
        (7.8, 12.5)
    );
    assert_eq!(tokens.focus.sun_offset_right_of_frame_pt, 17.8);
    assert_eq!(tokens.focus.sun_offset_above_center_pt, 0.0);
    assert_eq!(
        (tokens.focus.guide_line_length_pt, tokens.focus.guide_gap_radius_pt),
        (143.0, 15.5)
    );
    assert_eq!((tokens.focus.appear_scale, tokens.focus.appear_duration_s), (1.7, 0.12));
    assert_eq!((tokens.banner.height_pt, tokens.banner.min_width_pt), (30.0, 117.0));
    assert_eq!(
        (tokens.shutter.outer_diameter_pt, tokens.shutter.disc_diameter_pt),
        (80.1, 67.9)
    );
}

#[test]
fn text_role_sizes_follow_measured_cap_heights_over_inter_cap_ratio() {
    let tokens = UiKitTokens::default();
    // Кегль = высота заглавных / 0,728 (решение 104); замеры: R00 t=10.027, C t=18.0/40.0/54.0.
    let inter_cap_ratio = 0.728;
    for (kind, cap_height_pt) in [
        (TextRoleKind::ModeLabel, 10.2),
        (TextRoleKind::CardTitle, 10.3),
        (TextRoleKind::CardValue, 20.7),
        (TextRoleKind::ToolCaption, 10.3),
        (TextRoleKind::StripValue, 10.3),
        (TextRoleKind::StripUnit, 6.6),
    ] {
        let derived = cap_height_pt / inter_cap_ratio;
        let actual = f64::from(tokens.typography.role(kind).size_pt);
        assert!((actual - derived).abs() < 0.3, "{kind:?}: {actual} против {derived}");
    }
    assert_eq!(tokens.typography.mode_label.size_pt, 14.0);
    assert_eq!(tokens.typography.mode_label.weight, TextWeight::Semibold);
    assert_eq!(tokens.typography.card_value.weight, TextWeight::Regular);
    // Подпись варианта карточки: `[estimate]`, как у подписей верхнего ряда (14,1 pt полужирный).
    assert_eq!(
        tokens.typography.card_option.size_pt,
        tokens.typography.strip_value.size_pt
    );
    assert_eq!(tokens.typography.card_option.weight, TextWeight::Semibold);
    let style = tokens.typography.role(TextRoleKind::ModeLabel).style(3.0);
    assert!((style.size_px - 14.0 * 3.0).abs() < 1e-3);
    for kind in [
        TextRoleKind::ScaleValue,
        TextRoleKind::CardValue,
        TextRoleKind::StripValue,
        TextRoleKind::CardOption,
    ] {
        assert_eq!(
            tokens.typography.role(kind).style(3.0).figures,
            ui_draw::text::Figures::Tabular,
            "{kind:?}"
        );
    }
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

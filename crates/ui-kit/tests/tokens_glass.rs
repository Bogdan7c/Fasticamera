#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::field_reassign_with_default
)]
//! Пресеты стекла в токенах: значения по `glass.md`, раздел 6, проверка, serde, итог для формы.

use ui_draw::geometry::PxSize;
use ui_draw::glass::{EdgeProfile, Extent, GlassStyleError};
use ui_kit::tokens::{GlassRole, GlassTokens, TokenError, UiKitTokens};

const PX_PER_PT: f32 = 2.4545;

#[test]
fn button_and_panel_presets_match_the_specification_numbers() {
    let tokens = GlassTokens::default();
    let button = tokens.style_for(GlassRole::Button);
    assert_eq!(button.edge_profile, EdgeProfile::ConvexSuperellipse);
    assert_eq!(button.lens_zone, Extent::FractionOfHeight(0.25));
    assert_eq!(button.refraction_strength, Extent::FractionOfHeight(0.5));
    assert_eq!(button.blur_radius_pt, 2.0);
    assert_eq!(button.saturation, 1.5);
    assert_eq!(button.dispersion, 0.02);
    assert_eq!(button.dimming.max_alpha, 0.35);

    let panel = tokens.style_for(GlassRole::Panel);
    assert_eq!(panel.lens_zone, Extent::Points(24.0));
    assert_eq!(panel.refraction_strength, Extent::Points(28.0));
    assert_eq!(panel.blur_radius_pt, 8.0);
    // Остальное у панели то же, что у кнопки.
    assert_eq!(panel.saturation, button.saturation);
    assert_eq!(panel.specular, button.specular);
}

#[test]
fn presets_resolve_to_sensible_pixels_for_real_shapes() {
    let tokens = GlassTokens::default();
    // Кнопка 44 pt = 108 px: зона 27 px, сила 54 px, размытие 2 pt ≈ 4,9 px.
    let button = tokens.button.resolve(PxSize::new(108.0, 108.0), PX_PER_PT);
    assert!((button.lens_zone_px - 27.0).abs() < 1e-3);
    assert!((button.refraction_strength_px - 54.0).abs() < 1e-3);
    assert!((button.blur_radius_px - 2.0 * PX_PER_PT).abs() < 1e-3);
    // Панель 300×200 pt: зона 24 pt = 58,9 px, размытие 8 pt.
    let panel = tokens
        .panel
        .resolve(PxSize::new(300.0 * PX_PER_PT, 200.0 * PX_PER_PT), PX_PER_PT);
    assert!((panel.lens_zone_px - 24.0 * PX_PER_PT).abs() < 1e-3);
    assert!((panel.blur_radius_px - 8.0 * PX_PER_PT).abs() < 1e-3);
    assert!(
        panel.blur_radius_px > button.blur_radius_px,
        "большие формы мягче (Apple 219, 7:02)"
    );
}

#[test]
fn full_token_set_validates_and_a_bad_preset_names_its_role() {
    UiKitTokens::default().validate().unwrap();
    let mut broken = UiKitTokens::default();
    broken.glass.panel.saturation = f32::NAN;
    match broken.validate() {
        Err(TokenError::InvalidGlass {
            role: "panel",
            source: GlassStyleError::NotFinite {
                field: "saturation", ..
            },
        }) => {}
        other => panic!("ожидали ошибку пресета «panel»: {other:?}"),
    }
    let mut broken_button = UiKitTokens::default();
    broken_button.glass.button.tint_level = 2.0;
    assert!(matches!(
        broken_button.validate(),
        Err(TokenError::InvalidGlass { role: "button", .. })
    ));
}

#[test]
fn glass_tokens_roundtrip_through_json_and_reject_unknown_fields() {
    let tokens = UiKitTokens::default();
    let json = serde_json::to_string(&tokens).unwrap();
    assert_eq!(serde_json::from_str::<UiKitTokens>(&json).unwrap(), tokens);
    assert!(serde_json::from_str::<GlassTokens>(r#"{"button": {}, "dialog": {}}"#).is_err());
    let partial: GlassTokens = serde_json::from_str(r#"{"panel": {"blur_radius_pt": 12.0}}"#).unwrap();
    assert_eq!(partial.panel.blur_radius_pt, 12.0);
    assert_eq!(partial.button, GlassTokens::default().button);
}

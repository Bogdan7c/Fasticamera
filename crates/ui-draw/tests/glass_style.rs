#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::field_reassign_with_default
)]
//! `GlassStyle`: serde, проверка, пересчёт в пиксели.

use ui_draw::geometry::PxSize;
use ui_draw::glass::{EdgeProfile, Extent, GlassStyle, GlassStyleError};

#[test]
fn default_style_is_valid_and_roundtrips_through_json() {
    let style = GlassStyle::default();
    style.validate().unwrap();
    let json = serde_json::to_string(&style).unwrap();
    let restored: GlassStyle = serde_json::from_str(&json).unwrap();
    assert_eq!(restored, style);
    assert_eq!(style.edge_profile, EdgeProfile::ConvexSuperellipse);
    assert_eq!(style.dimming.max_alpha, 0.35);
}

#[test]
fn unknown_field_is_rejected_and_partial_config_keeps_defaults() {
    let error = serde_json::from_str::<GlassStyle>(r#"{"blur_radius_pt": 3.0, "bluur": 1}"#).unwrap_err();
    assert!(error.to_string().contains("bluur"), "{error}");
    let nested = serde_json::from_str::<GlassStyle>(r#"{"specular": {"alpha": 0.5, "typo": 1}}"#);
    assert!(
        nested.is_err(),
        "вложенные структуры тоже закрыты для неизвестных полей"
    );
    let partial: GlassStyle = serde_json::from_str(r#"{"blur_radius_pt": 3.0}"#).unwrap();
    assert_eq!(partial.blur_radius_pt, 3.0);
    assert_eq!(partial.saturation, GlassStyle::default().saturation);
}

#[test]
fn extent_has_a_tagged_json_form() {
    let json = serde_json::to_string(&Extent::FractionOfHeight(0.25)).unwrap();
    assert_eq!(json, r#"{"fraction_of_height":0.25}"#);
    assert_eq!(
        serde_json::from_str::<Extent>(r#"{"points":24.0}"#).unwrap(),
        Extent::Points(24.0)
    );
}

#[test]
fn validate_rejects_each_kind_of_bad_value() {
    let mut negative = GlassStyle::default();
    negative.blur_radius_pt = -1.0;
    assert!(matches!(
        negative.validate(),
        Err(GlassStyleError::Negative {
            field: "blur_radius_pt",
            ..
        })
    ));

    let mut not_a_number = GlassStyle::default();
    not_a_number.saturation = f32::NAN;
    assert!(matches!(
        not_a_number.validate(),
        Err(GlassStyleError::NotFinite {
            field: "saturation",
            ..
        })
    ));

    let mut out_of_range = GlassStyle::default();
    out_of_range.tint_level = 1.5;
    assert!(matches!(
        out_of_range.validate(),
        Err(GlassStyleError::OutOfRange {
            field: "tint_level",
            ..
        })
    ));

    let mut inverted = GlassStyle::default();
    inverted.dimming.luminance_low = 0.9;
    inverted.dimming.luminance_high = 0.2;
    assert!(matches!(
        inverted.validate(),
        Err(GlassStyleError::InvertedRange { .. })
    ));

    let mut bad_extent = GlassStyle::default();
    bad_extent.lens_zone = Extent::FractionOfHeight(-0.1);
    assert!(matches!(
        bad_extent.validate(),
        Err(GlassStyleError::OutOfRange { field: "lens_zone", .. })
    ));
}

#[test]
fn bigger_member_gets_a_thicker_lens_and_stronger_refraction() {
    let style = GlassStyle::default();
    let small = style.resolve(PxSize::new(100.0, 100.0), 2.4545);
    let large = style.resolve(PxSize::new(200.0, 200.0), 2.4545);
    assert!(large.lens_zone_px > small.lens_zone_px);
    assert!(large.refraction_strength_px > small.refraction_strength_px);
    // Доли высоты: 25 % зоны и 50 % силы.
    assert!((small.lens_zone_px - 25.0).abs() < 1e-4);
    assert!((small.refraction_strength_px - 50.0).abs() < 1e-4);
}

#[test]
fn resolve_converts_points_to_pixels_and_clamps_the_zone() {
    let style = GlassStyle {
        lens_zone: Extent::Points(24.0),
        refraction_strength: Extent::Points(28.0),
        blur_radius_pt: 8.0,
        ..GlassStyle::default()
    };
    let panel = style.resolve(PxSize::new(600.0, 400.0), 2.0);
    assert_eq!(panel.lens_zone_px, 48.0);
    assert_eq!(panel.refraction_strength_px, 56.0);
    assert_eq!(panel.blur_radius_px, 16.0);
    // Абсолютная зона не зависит от размера формы, пока влезает.
    let panel_wide = style.resolve(PxSize::new(900.0, 400.0), 2.0);
    assert_eq!(panel_wide.lens_zone_px, 48.0);
    // Форма 40 px: зона 48 px не влезает, ограничена половиной меньшей стороны.
    let tiny = style.resolve(PxSize::new(40.0, 40.0), 2.0);
    assert_eq!(tiny.lens_zone_px, 20.0);
    // Тень: радиус 24 pt, смещение — радиус/6.
    assert_eq!(panel.shadow.radius_px, 48.0);
    assert!((panel.shadow.offset_px - 8.0).abs() < 1e-5);
    // Свечение: 1,5 меньшей стороны.
    assert_eq!(panel.press.glow_radius_px, 600.0);
    assert!((panel.press.scale_for(1.0) - 1.1).abs() < 1e-6);
    assert_eq!(panel.press.scale_for(0.0), 1.0);
}
